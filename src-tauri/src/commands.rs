//! Tauri commands — the IPC surface replacing Electron's ipcMain.handle.
//! Frontend calls these via `invoke()`; async work emits events back.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::breaker::CircuitBreaker;
use crate::config::{Config, ConfigStore};
use crate::control::{ControlRegistry, ControlSnapshot};
use crate::hive::{Hive, Message, RegistryEntry};
use crate::pty::{self, PtyHandle};
use crate::ptyenv::build_pty_env;

pub struct AppState {
    pub hive: std::sync::Arc<Hive>,
    pub config_store: ConfigStore,
    pub config: Mutex<Config>,
    pub ptys: Mutex<HashMap<String, PtyHandle>>,
    pub breaker: Mutex<CircuitBreaker>,
    pub control: Mutex<ControlRegistry>,
    /// Transcript-based usage reader (usage.rs) — the breaker's fallback
    /// sample source when live OTel has nothing yet.
    pub usage: crate::usage::UsageReader,
    /// TCP loopback addr of the HookServer (`127.0.0.1:port`) → env HIVE_SOCK.
    pub sock_addr: Option<String>,
    /// The native hook-shim binary copied into `<harness>/bin/` — the analogue
    /// of upstream writing cth-hook.cjs into the hive's bin dir.
    pub hook_bin: Option<PathBuf>,
    /// Agents with a `/compact` pending — queued when a compact mission fires
    /// mid-work, drained at the next idle boundary (upstream's renderer-side
    /// compact queue, moved backend-side where agent status lives).
    pub compact_queue: Mutex<std::collections::HashSet<String>>,
    /// Inbox-wake watchdog backstop (workerwake.rs) — nudges idle workers
    /// whose mail landed AFTER their last Stop (the stall the renderer-side
    /// idle loop misses). Fed by hooks/spawn/kill.
    pub wake: Mutex<crate::workerwake::WorkerWakeWatchdog>,
    /// Inbound webhook gateway (webhook.rs) — loopback HTTP, secret-gated,
    /// routes verified posts into god's inbox.
    pub webhook: std::sync::Arc<crate::webhook::WebhookServer>,
    /// OTLP/HTTP-JSON collector (telemetry.rs) — live usage, tool spans,
    /// api_error feed for claude agents.
    pub telemetry: std::sync::Arc<crate::telemetry::TelemetryCollector>,
    /// Bound collector URL set at boot (spawn reads it for OTEL_* env).
    pub otel_endpoint: std::sync::OnceLock<String>,
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
        let mut config = config;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if config.seed_missions(now) {
            let _ = config_store.save(&config);
        }
        Ok(Self {
            hive: std::sync::Arc::new(hive),
            config_store,
            config: Mutex::new(config),
            ptys: Mutex::new(HashMap::new()),
            breaker: Mutex::new(CircuitBreaker::default()),
            control: Mutex::new(ControlRegistry::default()),
            usage: crate::usage::UsageReader::default(),
            compact_queue: Mutex::new(std::collections::HashSet::new()),
            wake: Mutex::new(crate::workerwake::WorkerWakeWatchdog::default()),
            webhook: std::sync::Arc::new(crate::webhook::WebhookServer::new()),
            telemetry: std::sync::Arc::new(crate::telemetry::TelemetryCollector::new()),
            otel_endpoint: std::sync::OnceLock::new(),
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
    spawn_inner(app, args, None, None)
}

/// Respawn a registry agent — same id, same hive folder/memory, optionally
/// resuming its recorded session (claude --resume <session_id>). This is the
/// restart/restore-team path: the registry survives, only the PTY died.
#[tauri::command]
pub fn respawn_agent(app: AppHandle, agent_id: String) -> Result<Spawned, String> {
    let st = state(&app);
    let entry = st
        .hive
        .registry()
        .into_iter()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| format!("no agent {agent_id}"))?;
    if st.ptys.lock().unwrap().contains_key(&agent_id) {
        return Err(format!("agent {agent_id} is already running"));
    }
    let args = SpawnArgs {
        provider: entry.provider.clone(),
        name: entry.name.clone(),
        role: Some(entry.role.clone()),
        cwd: entry.cwd.clone(),
        command: entry.command.clone(),
        seed_prompt: None,
        is_god: entry.is_god,
        cols: 120,
        rows: 30,
    };
    // Claude resumes by session id; other providers start fresh (their resume
    // flags differ — PORTING.md notes the preset fields for later).
    let resume = (entry.provider == "claude")
        .then(|| entry.session_id.clone())
        .flatten();
    spawn_inner(app, args, Some(agent_id), resume)
}

fn spawn_inner(
    app: AppHandle,
    args: SpawnArgs,
    forced_id: Option<String>,
    resume_session: Option<String>,
) -> Result<Spawned, String> {
    let st = state(&app);
    let id = forced_id.clone().unwrap_or_else(|| {
        format!(
            "{}-{}",
            args.name.to_lowercase().replace(' ', "-"),
            &uuid::Uuid::new_v4().simple().to_string()[..4]
        )
    });

    let entry = RegistryEntry {
        id: id.clone(),
        name: args.name.clone(),
        provider: args.provider.clone(),
        role: args.role.clone().unwrap_or_default(),
        cwd: args.cwd.clone(),
        is_god: args.is_god,
        status: "starting".into(),
        session_id: None,
        command: args.command.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    // New agents get the full folder+registry bootstrap; a respawn reuses the
    // existing entry and just re-stamps its status.
    if forced_id.is_none() {
        st.hive.register_agent(&entry).map_err(err_str)?;
    } else {
        st.hive.set_status(&id, "starting").map_err(err_str)?;
    }

    // Per-agent hive identity env (hive.ts:803) layered over the inherited env
    // minus parent-session markers (ptyEnv.ts).
    let auto = st.config.lock().unwrap().auto_mode;
    let agent_dir = st.hive.agent_dir(&id);
    let is_claude = args.provider == "claude";
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
    // OTel wiring (hive.ts:983-989 — claude only): metrics/logs pushed as
    // OTLP/HTTP-JSON to the embedded collector, tagged by agent.id.
    // Keychain secrets configured for injection (config.secret_env_keys) —
    // values come out of the OS credential store at spawn, never from disk.
    for key in &st.config.lock().unwrap().secret_env_keys {
        if let Ok(entry) = keyring::Entry::new("office-agent", key) {
            if let Ok(v) = entry.get_password() {
                agent_env.insert(key.clone(), v);
            }
        }
    }
    // Codex lifecycle bridge — a per-agent CODEX_HOME carrying our hook
    // tables + the user's auth, so a codex worker gets the full hive pipeline
    // (status, inbox drain, HITL deny, wake) without touching the user's
    // global config. Same shim; codex's payload is already claude-shaped.
    if args.provider == "codex" {
        if let Some(bin) = &st.hook_bin {
            if let Ok(home) = st.hive.install_codex_hooks(&agent_dir, bin) {
                agent_env.insert("CODEX_HOME".into(), home.display().to_string());
            }
        }
    }
    // Hermes lifecycle bridge — per-agent HERMES_HOME + config.yaml `hooks:`
    // block on the same shim (--hermes mode). Runtime dir is isolated too so
    // multiplex state never collides with the user's own hermes.
    if args.provider == "hermes" {
        if let Some(bin) = &st.hook_bin {
            if let Ok(home) = st.hive.install_hermes_hooks(&agent_dir, bin) {
                agent_env.insert("HERMES_HOME".into(), home.display().to_string());
                agent_env.insert(
                    "HERMES_RUNTIME_DIR".into(),
                    home.join("runtime").display().to_string(),
                );
                agent_env.insert("HERMES_ACCEPT_HOOKS".into(), "1".into());
            }
        }
    }
    // Gemini lifecycle bridge — per-agent system-settings.json bound via
    // GEMINI_CLI_SYSTEM_SETTINGS_PATH; the shim's --gemini mode translates.
    if args.provider == "gemini" {
        if let Some(bin) = &st.hook_bin {
            if let Ok(p) = st.hive.install_gemini_hooks(&agent_dir, bin) {
                agent_env.insert(
                    "GEMINI_CLI_SYSTEM_SETTINGS_PATH".into(),
                    p.display().to_string(),
                );
            }
        }
    }
    // Grok bridge — GLOBAL hook file under ~/.grok/hooks (trusted dir; the
    // shim no-ops outside hive workers via the AGENT_ID/HIVE_SOCK check).
    if args.provider == "grok" {
        if let Some(bin) = &st.hook_bin {
            let _ = st.hive.install_grok_hooks(bin);
        }
    }
    // Antigravity (`agy`) bridge — global hook groups under ~/.gemini,
    // runtime-scoped to hive workers by the shim's AGENT_ID check.
    if args.provider == "antigravity" {
        if let Some(bin) = &st.hook_bin {
            let _ = st.hive.install_agy_hooks(bin);
        }
    }
    // Pi / OpenCode — bundled-file bridges (extensions + plugins posting to
    // HIVE_SOCK directly); per-agent dirs isolate from the user's globals.
    if args.provider == "pi" {
        if let Ok(home) = crate::bridges::install_pi_hooks(&agent_dir) {
            agent_env.insert("PI_CODING_AGENT_DIR".into(), home.display().to_string());
        }
    }
    if args.provider == "opencode" {
        if let Ok(home) = crate::bridges::install_opencode_plugin(&agent_dir) {
            agent_env.insert("OPENCODE_CONFIG_DIR".into(), home.display().to_string());
        }
    }
    if is_claude {
        if let Some(ep) = st.otel_endpoint.get() {
            agent_env.insert("OTEL_METRICS_EXPORTER".into(), "otlp".into());
            agent_env.insert("OTEL_LOGS_EXPORTER".into(), "otlp".into());
            agent_env.insert("OTEL_EXPORTER_OTLP_PROTOCOL".into(), "http/json".into());
            agent_env.insert("OTEL_EXPORTER_OTLP_ENDPOINT".into(), ep.clone());
            agent_env.insert("OTEL_METRIC_EXPORT_INTERVAL".into(), "5000".into());
            agent_env.insert("OTEL_LOGS_EXPORT_INTERVAL".into(), "2000".into());
            agent_env.insert(
                "OTEL_RESOURCE_ATTRIBUTES".into(),
                format!("agent.id={},agent.name={}", entry.id, entry.name),
            );
        }
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
            if let Some(sid) = &resume_session {
                av.push("--resume".into());
                av.push(sid.clone());
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
            if let Some(sid) = &resume_session {
                parts.push("--resume".into());
                parts.push(pty::cmd_quote(sid));
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
            if let Some(sid) = &resume_session {
                command_line = format!("{} --resume {}", command_line, pty::cmd_quote(sid));
            }
            pty::spawn(&id, &command_line, &cwd, &env, args.cols, args.rows, app.clone())
        }
    }
    .map_err(err_str)?;
    st.wake.lock().unwrap().note_spawn(
        &id,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    );
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
    // Dead agents can't leak a tripped breaker level or a pending steer.
    st.breaker.lock().unwrap().forget(&id);
    st.wake.lock().unwrap().forget(&id, Some(&id));
    st.telemetry.forget(&id);
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

// ─── operator control (control.ts IPC equivalents) ──────────────────────────

#[tauri::command]
pub fn control_pause(app: AppHandle, agent_id: String, on: bool) -> Result<ControlSnapshot, String> {
    let st = state(&app);
    let mut c = st.control.lock().unwrap();
    c.pause(&agent_id, on);
    Ok(c.snapshot(&agent_id))
}

#[tauri::command]
pub fn control_resume(app: AppHandle, agent_id: String) -> Result<ControlSnapshot, String> {
    let st = state(&app);
    let mut c = st.control.lock().unwrap();
    c.resume(&agent_id);
    Ok(c.snapshot(&agent_id))
}

#[tauri::command]
pub fn control_halt(app: AppHandle, agent_id: String) -> Result<ControlSnapshot, String> {
    let st = state(&app);
    let mut c = st.control.lock().unwrap();
    c.halt(&agent_id);
    Ok(c.snapshot(&agent_id))
}

#[tauri::command]
pub fn control_steer(
    app: AppHandle,
    agent_id: String,
    text: String,
) -> Result<ControlSnapshot, String> {
    let st = state(&app);
    let mut c = st.control.lock().unwrap();
    c.steer(&agent_id, &text);
    Ok(c.snapshot(&agent_id))
}

#[tauri::command]
pub fn control_gate_tool(
    app: AppHandle,
    agent_id: String,
    tool: String,
    on: bool,
) -> Result<ControlSnapshot, String> {
    let st = state(&app);
    let mut c = st.control.lock().unwrap();
    c.gate_tool(&agent_id, &tool, on);
    Ok(c.snapshot(&agent_id))
}

#[tauri::command]
pub fn control_pause_delivery(
    app: AppHandle,
    agent_id: String,
    on: bool,
) -> Result<ControlSnapshot, String> {
    let st = state(&app);
    let mut c = st.control.lock().unwrap();
    c.pause_auto_delivery(&agent_id, on);
    Ok(c.snapshot(&agent_id))
}

#[tauri::command]
pub fn control_snapshot(app: AppHandle, agent_id: String) -> Result<ControlSnapshot, String> {
    Ok(state(&app).control.lock().unwrap().snapshot(&agent_id))
}

// ─── missions (scheduler) ───────────────────────────────────────────────────

#[tauri::command]
pub fn missions_list(app: AppHandle) -> Result<Vec<crate::missions::ScheduledMission>, String> {
    Ok(state(&app).config.lock().unwrap().missions.clone())
}

#[tauri::command]
pub fn missions_save(
    app: AppHandle,
    missions: Vec<crate::missions::ScheduledMission>,
) -> Result<(), String> {
    let st = state(&app);
    {
        let mut cfg = st.config.lock().unwrap();
        cfg.missions = missions;
        st.config_store.save(&cfg).map_err(err_str)?;
    }
    // The scheduler polls config — a save takes effect on the next tick for
    // free (no re-arm plumbing; that's the poll design's payoff).
    let _ = app.emit("missions-updated", serde_json::json!({}));
    Ok(())
}

// ─── secrets (OS keychain → agent env) ──────────────────────────────────────

const KEYRING_SERVICE: &str = "office-agent";

/// Store a secret in the OS credential store and register its env key for
/// injection. The value never touches config.json or log.jsonl.
#[tauri::command]
pub fn secret_set(app: AppHandle, key: String, value: String) -> Result<(), String> {
    let key = key.trim().to_string();
    if key.is_empty() || value.is_empty() {
        return Err("key and value required".into());
    }
    keyring::Entry::new(KEYRING_SERVICE, &key)
        .and_then(|e| e.set_password(&value))
        .map_err(|e| format!("keychain: {e}"))?;
    let st = state(&app);
    let mut cfg = st.config.lock().unwrap();
    if !cfg.secret_env_keys.contains(&key) {
        cfg.secret_env_keys.push(key);
        st.config_store.save(&cfg).map_err(err_str)?;
    }
    Ok(())
}

/// Remove a secret — from the keychain AND the injection list.
#[tauri::command]
pub fn secret_delete(app: AppHandle, key: String) -> Result<(), String> {
    if let Ok(e) = keyring::Entry::new(KEYRING_SERVICE, &key) {
        let _ = e.delete_credential();
    }
    let st = state(&app);
    let mut cfg = st.config.lock().unwrap();
    cfg.secret_env_keys.retain(|k| k != &key);
    st.config_store.save(&cfg).map_err(err_str)
}

/// The injection-list key NAMES — values are never exposed to the frontend.
#[tauri::command]
pub fn secrets_list(app: AppHandle) -> Result<Vec<String>, String> {
    Ok(state(&app).config.lock().unwrap().secret_env_keys.clone())
}

// ─── webhooks (inbound HTTP → god's inbox) ──────────────────────────────────

#[tauri::command]
pub fn webhooks_list(app: AppHandle) -> Result<Vec<crate::webhook::WebhookEndpoint>, String> {
    Ok(state(&app).config.lock().unwrap().webhooks.clone())
}

/// Persist the endpoint list and hot-swap it on the running server — a
/// removed id stops resolving on the very next request (upstream
/// setEndpoints semantics; the port never rebinds).
#[tauri::command]
pub fn webhooks_save(
    app: AppHandle,
    webhooks: Vec<crate::webhook::WebhookEndpoint>,
) -> Result<u16, String> {
    let st = state(&app);
    {
        let mut cfg = st.config.lock().unwrap();
        cfg.webhooks = webhooks.clone();
        st.config_store.save(&cfg).map_err(err_str)?;
    }
    let port = st.config.lock().unwrap().webhook_port;
    ensure_webhook_server(&app, &webhooks, port);
    Ok(port)
}

/// Start the loopback server if endpoints exist and it isn't bound yet.
/// Returns the port actually serving (None = nothing to serve).
fn ensure_webhook_server(
    app: &AppHandle,
    endpoints: &[crate::webhook::WebhookEndpoint],
    port: u16,
) -> Option<u16> {
    let st = state(app);
    if st.webhook.set_endpoints(endpoints) == 0 {
        return None;
    }
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Some(port); // already bound; set_endpoints already applied
    }
    let wh = st.webhook.clone();
    let hive = st.hive.clone();
    let app2 = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = wh.run(port, hive, app2) {
            eprintln!("[webhook] server stopped: {e:#}");
            STARTED.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    });
    Some(port)
}

#[tauri::command]
pub fn webhook_approve(app: AppHandle, token: String, endpoint_id: String) -> Result<String, String> {
    let st = state(&app);
    st.webhook
        .approve(&token, &st.hive, &endpoint_id)
        .ok_or_else(|| "unknown or non-pending token".to_string())
}

#[tauri::command]
pub fn webhook_reject(app: AppHandle, token: String) -> Result<bool, String> {
    Ok(state(&app).webhook.reject(&token))
}

#[tauri::command]
pub fn webhook_pending(app: AppHandle) -> Result<Vec<(String, serde_json::Value)>, String> {
    Ok(state(&app).webhook.pending())
}

// ─── breaker (the beat's data seam + level read) ────────────────────────────

#[tauri::command]
pub fn breaker_level(app: AppHandle, agent_id: String) -> Result<String, String> {
    let lvl = state(&app).breaker.lock().unwrap().level_for(&agent_id);
    Ok(format!("{lvl:?}").to_lowercase())
}

fn err_str(e: anyhow::Error) -> String {
    format!("{e:#}")
}
