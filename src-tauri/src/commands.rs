//! Tauri commands — the IPC surface replacing Electron's ipcMain.handle.
//! Frontend calls these via `invoke()`; async work emits events back.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::config::{Config, ConfigStore};
use crate::hive::{Hive, Message, RegistryEntry};
use crate::pty::{self, PtyHandle};
use crate::ptyenv::build_pty_env;

pub struct AppState {
    pub hive: Hive,
    pub config_store: ConfigStore,
    pub config: Mutex<Config>,
    pub ptys: Mutex<HashMap<String, PtyHandle>>,
    /// TCP loopback addr of the HookServer (`127.0.0.1:port`) → env HIVE_SOCK.
    pub sock_addr: Option<String>,
    /// The native hook-shim binary copied into `<harness>/bin/` — the analogue
    /// of upstream writing cth-hook.cjs into the hive's bin dir.
    pub hook_bin: Option<PathBuf>,
}

impl AppState {
    pub fn new(harness_home: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&harness_home)?;
        let hive = Hive::ensure(&harness_home.join("hive"))?;
        let config_store = ConfigStore::new(harness_home.clone());
        let config = config_store.load();
        // Stage the hive-hook binary next to the hive, like upstream's
        // bin/cth-hook.cjs. Resolved from beside the running exe (dev:
        // target/debug/, bundle: install dir). Missing → hooks degrade off.
        let hook_bin = stage_hook_bin(&harness_home);
        Ok(Self {
            hive,
            config_store,
            config: Mutex::new(config),
            ptys: Mutex::new(HashMap::new()),
            sock_addr: None,
            hook_bin,
        })
    }
}

fn stage_hook_bin(harness_home: &PathBuf) -> Option<PathBuf> {
    let name = if cfg!(windows) { "hive-hook.exe" } else { "hive-hook" };
    let src = std::env::current_exe()
        .ok()?
        .parent()?
        .join(name);
    if !src.is_file() {
        return None;
    }
    let dest = harness_home.join("bin").join(name);
    if let Some(p) = dest.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    std::fs::copy(&src, &dest).ok().map(|_| dest)
}

pub fn state(app: &AppHandle) -> State<'_, AppState> {
    app.state::<AppState>()
}

// ─── agent lifecycle ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SpawnArgs {
    pub provider: String,
    pub name: String,
    pub role: Option<String>,
    pub cwd: String,
    /// Full command line (preset default + flags assembled by the frontend, or
    /// a raw custom command). Kept as one string — see pty::spawn's shell wrap.
    pub command: String,
    /// Typed-into-the-TUI seed prompt ('type-into-tui' delivery). When unset,
    /// non-hiveAware providers still get the hive prompt typed automatically.
    #[serde(default)]
    pub seed_prompt: Option<String>,
    #[serde(default)]
    pub is_god: bool,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
}

fn default_cols() -> u16 {
    120
}
fn default_rows() -> u16 {
    30
}

#[derive(Serialize)]
pub struct Spawned {
    pub id: String,
}

#[tauri::command]
pub fn spawn_agent(app: AppHandle, args: SpawnArgs) -> Result<Spawned, String> {
    let st = state(&app);
    let id = format!(
        "{}-{}",
        args.name.to_lowercase().replace(' ', "-"),
        &uuid::Uuid::new_v4().simple().to_string()[..4]
    );

    let entry = RegistryEntry {
        id: id.clone(),
        name: args.name.clone(),
        provider: args.provider.clone(),
        role: args.role.clone().unwrap_or_default(),
        cwd: args.cwd.clone(),
        is_god: args.is_god,
        status: "starting".into(),
        session_id: None,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    st.hive.register_agent(&entry).map_err(err_str)?;

    // Per-agent hive identity env (hive.ts:803) layered over the inherited env
    // minus parent-session markers (ptyEnv.ts).
    let auto = st.config.lock().unwrap().auto_mode;
    let agent_dir = st.hive.agent_dir(&id);
    let mut agent_env = HashMap::from([
        ("AGENT_ID".to_string(), id.clone()),
        ("AGENT_NAME".to_string(), args.name.clone()),
        ("HIVE_ROOT".to_string(), st.hive.root.display().to_string()),
        ("AGENT_DIR".to_string(), agent_dir.display().to_string()),
    ]);
    if auto {
        agent_env.insert("HIVE_AUTO_APPROVE".into(), "1".into());
    }
    if let Some(sock) = &st.sock_addr {
        agent_env.insert("HIVE_SOCK".into(), sock.clone());
    }

    let parent: HashMap<String, String> = std::env::vars().collect();
    let path = parent.get("PATH").cloned().unwrap_or_default();
    let env = build_pty_env(&parent, &path, Some(&agent_env), cfg!(windows));

    let cwd = if args.cwd.is_empty() {
        agent_dir.display().to_string()
    } else {
        args.cwd.clone()
    };

    // ── hive wiring per provider class (hive.ts spawnAgentCore, condensed) ──
    // hiveAware (claude): per-session settings.json + --append-system-prompt.
    // Others: the same prompt rides in as the typed first turn — the uniform
    // 'type-into-tui' delivery, which also covers flag/positional providers.
    let is_claude = args.provider == "claude";
    let injected = st.hive.injected_prompt(&entry);
    let mut command_line = args.command.clone();
    let mut seed = args.seed_prompt.clone();

    if is_claude {
        if let (Some(_sock), Some(bin)) = (&st.sock_addr, &st.hook_bin) {
            let shim_cmd = format!("\"{}\"", bin.display());
            let settings = st.hive.hook_settings(&shim_cmd);
            let settings_path = agent_dir.join("settings.json");
            let _ = std::fs::write(
                &settings_path,
                serde_json::to_string_pretty(&settings).unwrap_or_default(),
            );
            command_line = format!(
                "{} --settings {}",
                command_line,
                pty::cmd_quote(&settings_path.display().to_string())
            );
        }
    } else if seed.is_none() {
        seed = Some(injected.clone());
    }

    // Resolve the program for a direct-argv spawn (exe) vs cmd-wrap (scripts).
    let first_tok = command_line
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    let handle = match pty::resolve_program(&first_tok, &path) {
        Some(pty::Resolved::Exe(prog)) => {
            let mut av: Vec<String> = command_line
                .split_whitespace()
                .skip(1)
                .map(String::from)
                .collect();
            if is_claude {
                av.push("--append-system-prompt".into());
                av.push(injected.clone());
            }
            pty::spawn_argv(&id, &prog, &av, &cwd, &env, args.cols, args.rows, app.clone())
        }
        Some(pty::Resolved::Script(prog)) => {
            let mut parts = vec![pty::cmd_quote(&prog.display().to_string())];
            for a in command_line.split_whitespace().skip(1) {
                parts.push(pty::cmd_quote(a));
            }
            if is_claude {
                parts.push("--append-system-prompt".into());
                parts.push(pty::cmd_quote(&pty::cmd_sanitize(&injected)));
            }
            command_line = parts.join(" ");
            pty::spawn(&id, &command_line, &cwd, &env, args.cols, args.rows, app.clone())
        }
        None => {
            if is_claude {
                command_line = format!(
                    "{} --append-system-prompt {}",
                    command_line,
                    pty::cmd_quote(&pty::cmd_sanitize(&injected))
                );
            }
            pty::spawn(&id, &command_line, &cwd, &env, args.cols, args.rows, app.clone())
        }
    }
    .map_err(err_str)?;
    st.ptys.lock().unwrap().insert(id.clone(), handle);
    st.hive.set_status(&id, "idle").map_err(err_str)?;

    // 'type-into-tui' seed delivery: the injected hive prompt (or an explicit
    // seed) typed into the session once the TUI has booted — upstream's
    // seedPrompt path for crush/cursor, used here for every non-claude agent.
    if let Some(text) = seed {
        let app2 = app.clone();
        let id2 = id.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            let Some(st) = app2.try_state::<AppState>() else { return };
            let mut ptys = st.ptys.lock().unwrap();
            if let Some(h) = ptys.get_mut(&id2) {
                let _ = h.write(&format!("{}\r", text.split_whitespace().collect::<Vec<_>>().join(" ")));
            }
        });
    }

    let _ = app.emit(
        "agent-status",
        serde_json::json!({ "id": id, "status": "idle" }),
    );
    Ok(Spawned { id })
}

#[tauri::command]
pub fn pty_write(app: AppHandle, id: String, data: String) -> Result<(), String> {
    let st = state(&app);
    let mut ptys = st.ptys.lock().unwrap();
    match ptys.get_mut(&id) {
        Some(h) => h.write(&data).map_err(err_str),
        None => Err(format!("no pty {id}")),
    }
}

#[tauri::command]
pub fn pty_resize(app: AppHandle, id: String, cols: u16, rows: u16) -> Result<(), String> {
    let st = state(&app);
    let ptys = st.ptys.lock().unwrap();
    match ptys.get(&id) {
        Some(h) => h.resize(cols, rows).map_err(err_str),
        None => Err(format!("no pty {id}")),
    }
}

#[tauri::command]
pub fn kill_agent(app: AppHandle, id: String) -> Result<(), String> {
    let st = state(&app);
    if let Some(mut h) = st.ptys.lock().unwrap().remove(&id) {
        let _ = h.kill();
    }
    st.hive.set_status(&id, "stopped").map_err(err_str)?;
    let _ = app.emit(
        "agent-status",
        serde_json::json!({ "id": id, "status": "stopped" }),
    );
    Ok(())
}

// ─── hive surface ───────────────────────────────────────────────────────────

#[tauri::command]
pub fn hive_registry(app: AppHandle) -> Result<Vec<RegistryEntry>, String> {
    Ok(state(&app).hive.registry())
}

#[tauri::command]
pub fn hive_send(
    app: AppHandle,
    from: String,
    to: String,
    act: String,
    subject: String,
    body: String,
) -> Result<Message, String> {
    let msg = Message::new(&from, &to, &act, &subject, &body);
    state(&app).hive.send(&msg).map_err(err_str)?;
    Ok(msg)
}

#[tauri::command]
pub fn hive_inbox(app: AppHandle, agent_id: String) -> Result<Vec<Message>, String> {
    state(&app).hive.inbox(&agent_id).map_err(err_str)
}

#[tauri::command]
pub fn hive_mark_done(app: AppHandle, agent_id: String, msg_id: String) -> Result<(), String> {
    state(&app)
        .hive
        .mark_done(&agent_id, &msg_id)
        .map_err(err_str)
}

#[tauri::command]
pub fn hive_board(app: AppHandle) -> Result<String, String> {
    state(&app).hive.read_board().map_err(err_str)
}

#[tauri::command]
pub fn hive_tasks(app: AppHandle) -> Result<serde_json::Value, String> {
    state(&app).hive.read_tasks().map_err(err_str)
}

// ─── memory ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn read_memory(app: AppHandle, agent_id: String) -> Result<String, String> {
    state(&app).hive.read_memory(&agent_id).map_err(err_str)
}

#[tauri::command]
pub fn append_memory(app: AppHandle, agent_id: String, entry: String) -> Result<(), String> {
    state(&app)
        .hive
        .append_memory(&agent_id, &entry)
        .map_err(err_str)
}

// ─── config ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_config(app: AppHandle) -> Result<Config, String> {
    Ok(state(&app).config.lock().unwrap().clone())
}

#[tauri::command]
pub fn set_auto_mode(app: AppHandle, enabled: bool) -> Result<(), String> {
    let st = state(&app);
    {
        let mut cfg = st.config.lock().unwrap();
        cfg.auto_mode = enabled;
        st.config_store.save(&cfg).map_err(err_str)?;
    }
    let _ = app.emit("config-changed", serde_json::json!({ "auto_mode": enabled }));
    Ok(())
}

fn err_str(e: anyhow::Error) -> String {
    format!("{e:#}")
}
