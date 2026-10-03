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
}

impl AppState {
    pub fn new(harness_home: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&harness_home)?;
        let hive = Hive::ensure(&harness_home.join("hive"))?;
        let config_store = ConfigStore::new(harness_home.clone());
        let config = config_store.load();
        Ok(Self {
            hive,
            config_store,
            config: Mutex::new(config),
            ptys: Mutex::new(HashMap::new()),
        })
    }
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

    let parent: HashMap<String, String> = std::env::vars().collect();
    let path = parent.get("PATH").cloned().unwrap_or_default();
    let env = build_pty_env(&parent, &path, Some(&agent_env), cfg!(windows));

    let cwd = if args.cwd.is_empty() {
        agent_dir.display().to_string()
    } else {
        args.cwd.clone()
    };

    let handle = pty::spawn(&id, &args.command, &cwd, &env, args.cols, args.rows, app.clone())
        .map_err(err_str)?;
    st.ptys.lock().unwrap().insert(id.clone(), handle);
    st.hive.set_status(&id, "idle").map_err(err_str)?;

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
