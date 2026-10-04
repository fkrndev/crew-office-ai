//! Tauri commands — the miscellaneous IPC surface replacing the remaining
//! `ipcMain.handle` entries in upstream `src/main/index.ts`: shell/clipboard/
//! dialog bridges, the setup probe, fs helpers, hero + model-catalog data,
//! provider BYOK keys, webhook server lifecycle, slack config, telemetry
//! reads, control extras, and the harness-home config trio.
//!
//! Frontend calls these via `invoke()`; names mirror upstream channels
//! (app:openExternal → app_open_external, …). Errors are `Result<_, String>`
//! throughout; handlers that upstream shaped as `{ok, ...}` objects keep that
//! shape inside an Ok.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_shell::ShellExt;

use crate::commands::state;
use crate::misc::{
    err_str, expand_tilde, harness_home, is_filesystem_root, load_hero, mime_for, os_open,
    random_hex32, secret_get, write_slack_reply_script, KEYRING_SERVICE,
};

/// fs_read_binary's byte cap — same 2MB guard the confined reader uses.
const MAX_BINARY_READ: u64 = 2 * 1024 * 1024;

// ─── shell / clipboard ──────────────────────────────────────────────────────

/// app:openExternal — open a URL in the system handler. Scheme-gated to
/// https/http/mailto so the renderer can never shell an arbitrary scheme
/// (upstream allowed https + the two OS settings schemes; this port keeps
/// mailto for the contact links and drops the settings schemes).
#[tauri::command]
pub fn app_open_external(app: AppHandle, url: String) -> Result<bool, String> {
    let url = url.trim().to_string();
    let lower = url.to_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:"))
    {
        return Err("blocked url".into());
    }
    if let Err(e) = app.shell().open(url.clone(), None) {
        // Fallback: the OS's own opener. A renderer-provided URL reaching this
        // path is already scheme-gated above.
        os_open(&url).map_err(|e2| format!("{e}; fallback failed: {e2}"))?;
    }
    Ok(true)
}

/// app:copyToClipboard.
#[tauri::command]
pub fn app_clipboard_write(app: AppHandle, text: String) -> Result<bool, String> {
    app.clipboard().write_text(text).map_err(|e| e.to_string())?;
    Ok(true)
}

/// app:readClipboard — upstream swallows read errors to ''; keep that.
#[tauri::command]
pub fn app_clipboard_read(app: AppHandle) -> Result<String, String> {
    Ok(app.clipboard().read_text().unwrap_or_default())
}

/// Persist a pasted image to the OS clipboard — the paste-flow companion of
/// the file dialog: the renderer hands PNG bytes (base64, optionally a
/// `data:` URL) and the OS clipboard gets the decoded image.
#[tauri::command]
pub fn clipboard_save_image(app: AppHandle, base64: String) -> Result<bool, String> {
    let s = base64.trim();
    // Strip a `data:image/png;base64,` prefix if the caller sent a data URL.
    let s = match s.find("base64,") {
        Some(i) => &s[i + 7..],
        None => s,
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|e| format!("base64: {e}"))?;
    let img = tauri::image::Image::from_bytes(&bytes).map_err(|e| format!("decode image: {e}"))?;
    app.clipboard().write_image(&img).map_err(|e| e.to_string())?;
    Ok(true)
}

// ─── app lifecycle / desktop integration ────────────────────────────────────

/// app:resetAll — stop everything, then wipe the harness-owned state under
/// the configured home: `hive/`, `roster.json` (archived first, never
/// destroyed outright), `harness.db`. The home dir itself and anything else
/// in it is untouched. Refuses outright when home resolves to a filesystem
/// root — a missing/misconfigured home must never escalate into a volume
/// wipe. (Upstream also reset config.json and relaunched; this port returns a
/// summary + `requiresRestart` instead — the caller restarts the app.)
#[tauri::command]
pub fn app_reset_all(app: AppHandle) -> Result<Value, String> {
    let st = state(&app);
    let home = harness_home(&app);
    if is_filesystem_root(&home) {
        return Err(format!(
            "refusing to reset: harness home {} is a filesystem root",
            home.display()
        ));
    }

    // 1. Stop the inbound adapters so nothing writes while we wipe.
    st.slack.stop();
    st.webhook.stop();

    // 2. Close every live PTY (wholesale shutdown, not per-agent lifecycle —
    //    no status/archival fanfare).
    let killed = {
        let mut ptys = st.ptys.lock().unwrap();
        let drained: Vec<crate::pty::PtyHandle> = ptys.drain().map(|(_, h)| h).collect();
        let n = drained.len();
        drop(ptys);
        for mut h in drained {
            let _ = h.kill();
        }
        n
    };

    // 3. The roster retires into roster-backups/ — never deleted outright.
    st.roster.archive();

    // 4. Remove the harness-owned files. Each step is independent and
    //    reported, not fatal — harness.db may still be held open by the
    //    persist connection on Windows, and a half-reset that reports is
    //    better than an all-or-nothing that hangs.
    let mut removed: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let hive_dir = home.join("hive");
    if hive_dir.exists() {
        match std::fs::remove_dir_all(&hive_dir) {
            Ok(()) => removed.push(hive_dir.display().to_string()),
            Err(e) => errors.push(format!("{}: {e}", hive_dir.display())),
        }
    }
    for name in ["roster.json", "harness.db", "harness.db-wal", "harness.db-shm"] {
        let p = home.join(name);
        if p.exists() {
            match std::fs::remove_file(&p) {
                Ok(()) => removed.push(p.display().to_string()),
                Err(e) => errors.push(format!("{}: {e}", p.display())),
            }
        }
    }
    Ok(json!({
        "ok": errors.is_empty(),
        "stoppedSlack": true,
        "stoppedWebhook": true,
        "killedPtys": killed,
        "rosterArchived": true,
        "removed": removed,
        "errors": errors,
        "requiresRestart": true,
    }))
}

/// app:setLoginItem — Windows autostart via the Run key. Non-Windows has no
/// equivalent here yet → "not supported" (upstream used Electron's
/// setLoginItemSettings, which was the macOS path).
#[tauri::command]
pub fn app_set_login_item(app: AppHandle, enabled: bool) -> Result<bool, String> {
    let _ = app;
    #[cfg(windows)]
    {
        let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
        let (key, _) = hkcu
            .create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")
            .map_err(|e| e.to_string())?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        if enabled {
            key.set_value("AgentOffice", &exe.display().to_string())
                .map_err(|e| e.to_string())?;
        } else {
            // An absent value is not an error — the toggle's "off" state is
            // idempotent.
            let _ = key.delete_value("AgentOffice");
        }
        Ok(enabled)
    }
    #[cfg(not(windows))]
    {
        let _ = enabled;
        Err("not supported".into())
    }
}

/// app:setNotifications — persist the desktop-notification toggle. The slimmed
/// Config has no field for it, so it lives in the persist kv (readable by the
/// hooks layer when a native-notification path lands).
#[tauri::command]
pub fn app_set_notifications(app: AppHandle, enabled: bool) -> Result<bool, String> {
    state(&app)
        .persist
        .lock()
        .unwrap()
        .set_kv("notifications_enabled", &json!(enabled));
    Ok(true)
}

/// terminal:openAtFolder — spawn a system terminal at a folder.
#[tauri::command]
pub fn terminal_open_at_folder(app: AppHandle, path: String) -> Result<bool, String> {
    let _ = app;
    let p = expand_tilde(&path);
    if p.is_empty() {
        return Err("invalid cwd".into());
    }
    if !Path::new(&p).is_dir() {
        return Err(format!("not a directory: {p}"));
    }
    #[cfg(windows)]
    {
        let path_env = std::env::var("PATH").unwrap_or_default();
        let spawned = if crate::pty::resolve_program("wt", &path_env).is_some() {
            std::process::Command::new("cmd")
                .args(["/c", "start", "", "wt", "-d", p.as_str()])
                .spawn()
        } else {
            std::process::Command::new("cmd")
                .args(["/c", "start", "", "cmd", "/k", "cd", "/d", p.as_str()])
                .spawn()
        };
        spawned.map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .args(["-a", "Terminal", &p])
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let path_env = std::env::var("PATH").unwrap_or_default();
        let mut launched = false;
        for term in ["x-terminal-emulator", "gnome-terminal", "konsole"] {
            if crate::pty::resolve_program(term, &path_env).is_some() {
                let arg = format!("--working-directory={p}");
                if std::process::Command::new(term).arg(&arg).spawn().is_ok() {
                    launched = true;
                    break;
                }
            }
        }
        if !launched {
            return Err("no supported terminal found".into());
        }
    }
    Ok(true)
}

/// tools:status — probe each agent-engine binary on PATH. Presence is a PATH
/// resolution, never a spawn (upstream's argument verbatim: several of these
/// boot a TUI when invoked bare).
#[tauri::command]
pub fn tools_status(app: AppHandle) -> Result<Vec<Value>, String> {
    let _ = app;
    const PROVIDERS: [&str; 10] = [
        "claude", "codex", "gemini", "grok", "agy", "hermes", "pi", "opencode", "qwen", "crush",
    ];
    let path_env = std::env::var("PATH").unwrap_or_default();
    Ok(PROVIDERS
        .iter()
        .map(|id| {
            let resolved = crate::pty::resolve_program(id, &path_env);
            let path = match resolved {
                Some(crate::pty::Resolved::Exe(p)) | Some(crate::pty::Resolved::Script(p)) => {
                    Value::String(p.display().to_string())
                }
                None => Value::Null,
            };
            json!({ "id": id, "found": path.is_string(), "path": path })
        })
        .collect())
}

// ─── fs helpers ─────────────────────────────────────────────────────────────

/// fs:statAbs — existence/metadata check for an ABSOLUTE path (the terminal
/// ⌘-click flow). Metadata only, never contents; `~/` is expanded here since
/// the renderer doesn't know the home dir.
#[tauri::command]
pub fn fs_stat_abs(app: AppHandle, path: String) -> Result<Value, String> {
    let _ = app;
    if path.len() > 4096 || path.contains('\0') {
        return Ok(json!({ "exists": false, "isFile": false, "path": "" }));
    }
    let abs = expand_tilde(&path);
    if !Path::new(&abs).is_absolute() {
        return Ok(json!({ "exists": false, "isFile": false, "path": path }));
    }
    match std::fs::metadata(&abs) {
        Ok(m) => Ok(json!({ "exists": true, "isFile": m.is_file(), "path": abs })),
        Err(_) => Ok(json!({ "exists": false, "isFile": false, "path": abs })),
    }
}

/// fs:readBinary — raw bytes for an absolute path the text reader refuses
/// (images et al.), base64 on the wire. Same pre-stat + post-read cap as
/// upstream so a file that grows mid-read can't slip past the memory
/// guarantee.
#[tauri::command]
pub fn fs_read_binary(app: AppHandle, path: String) -> Result<Value, String> {
    let _ = app;
    if path.is_empty() || path.len() > 4096 || path.contains('\0') {
        return Err("bad request".into());
    }
    let abs = expand_tilde(&path);
    let meta = std::fs::metadata(&abs).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    if meta.len() > MAX_BINARY_READ {
        return Err(format!(
            "file too large ({:.1} MB)",
            meta.len() as f64 / 1024.0 / 1024.0
        ));
    }
    let bytes = std::fs::read(&abs).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BINARY_READ {
        return Err("file grew past the size limit while reading".into());
    }
    Ok(json!({
        "ok": true,
        "base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
        "mime": mime_for(Path::new(&abs)),
        "path": abs,
        "size": bytes.len(),
    }))
}

/// fs:writeFile — confined write through fs_ops::safe_resolve (the one
/// path-escape policy); parent dirs are created so a new nested file lands.
#[tauri::command]
pub fn fs_write_file(
    app: AppHandle,
    root: String,
    rel: String,
    content: String,
) -> Result<Value, String> {
    let _ = app;
    let abs = crate::fs_ops::safe_resolve(Path::new(&root), &rel)
        .ok_or_else(|| "path escapes the workspace".to_string())?;
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&abs, content).map_err(|e| e.to_string())?;
    Ok(json!({ "ok": true, "path": abs.display().to_string() }))
}

/// fs:revealPath — reveal in the OS file browser. A FILE is only ever
/// revealed (explorer /select, open -R, xdg-open on the parent) — opening the
/// file itself would hand an arbitrary path from agent output to its default
/// application. A DIRECTORY is opened (a folder has no default app to
/// exploit).
#[tauri::command]
pub fn fs_reveal_path(app: AppHandle, path: String) -> Result<bool, String> {
    let _ = app;
    if path.is_empty() || path.len() > 4096 || path.contains('\0') {
        return Err("bad request".into());
    }
    let abs = expand_tilde(&path);
    let meta = std::fs::metadata(&abs).map_err(|_| "not found".to_string())?;
    if meta.is_file() {
        #[cfg(windows)]
        {
            std::process::Command::new("explorer")
                .arg(format!("/select,{abs}"))
                .spawn()
                .map_err(|e| e.to_string())?;
        }
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .args(["-R", abs.as_str()])
                .spawn()
                .map_err(|e| e.to_string())?;
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let parent = Path::new(&abs)
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| abs.clone());
            std::process::Command::new("xdg-open")
                .arg(parent)
                .spawn()
                .map_err(|e| e.to_string())?;
        }
    } else {
        os_open(&abs)?;
    }
    Ok(true)
}

// ─── dialogs ────────────────────────────────────────────────────────────────

/// dialog:chooseFolder — single-directory picker; upstream {ok,path} shape.
#[tauri::command]
pub fn dialog_choose_folder(app: AppHandle) -> Result<Value, String> {
    match app
        .dialog()
        .file()
        .set_title("Pick a folder")
        .blocking_pick_folder()
    {
        Some(fp) => Ok(json!({ "ok": true, "path": fp.to_string() })),
        None => Ok(json!({ "ok": false, "error": "cancelled" })),
    }
}

/// dialog:attachFiles — multi-select file picker (Images group + All Files);
/// upstream {ok, files:[{path,name}]} shape.
#[tauri::command]
pub fn dialog_attach_files(app: AppHandle) -> Result<Value, String> {
    match app
        .dialog()
        .file()
        .set_title("Attach images or files")
        .add_filter(
            "Images",
            &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "heic", "tiff", "avif"],
        )
        .add_filter("All Files", &["*"])
        .blocking_pick_files()
    {
        Some(files) => {
            let files: Vec<Value> = files
                .into_iter()
                .map(|fp| {
                    let s = fp.to_string();
                    let name = Path::new(&s)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| s.clone());
                    json!({ "path": s, "name": name })
                })
                .collect();
            Ok(json!({ "ok": true, "files": files }))
        }
        None => Ok(json!({ "ok": false, "error": "cancelled" })),
    }
}

// ─── catalogs + hero payload ────────────────────────────────────────────────

/// models:catalog — the agent model presets. Upstream fetched a remote
/// catalog with a baked-in fallback; this port serves the baked copy
/// (embedded at build time from upstream's shared/modelCatalog.json).
#[tauri::command]
pub fn models_catalog(app: AppHandle) -> Result<Value, String> {
    let _ = app;
    serde_json::from_str::<Value>(include_str!("modelCatalog.json"))
        .map_err(|e| format!("embedded catalog invalid: {e}"))
}

/// hero:payload — the Settings hero card, fetched + cached with a 6h TTL and
/// a never-fail contract ({hero, fetchedAt, stale}). Off the UI thread since
/// a cold cache means a bounded network fetch.
#[tauri::command]
pub async fn hero_payload(app: AppHandle, force: Option<bool>) -> Result<Value, String> {
    let cache = harness_home(&app).join("hero.json");
    let force = force.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || load_hero(&cache, force))
        .await
        .map_err(|e| e.to_string())
}

// ─── roster ─────────────────────────────────────────────────────────────────

/// roster:readSync — same read as roster_read (upstream kept a sync channel
/// for the renderer's first paint; Tauri IPC is async either way, but the
/// name is the contract).
#[tauri::command]
pub fn roster_read_sync(app: AppHandle) -> Result<Option<Value>, String> {
    Ok(state(&app).roster.read())
}

// ─── provider BYOK keys (write-only keychain entries) ──────────────────────

/// providerKey:* — per-provider API keys stored WRITE-ONLY in the OS
/// credential store under `PROVIDER_KEY_<UPPER>` (upstream's `apikey:<backend>`
/// broker refs). `set` also registers the name in config.secret_env_keys so
/// spawned agents receive it as an env var — the same injection path
/// commands::secret_set arms. The renderer can SET and ASK-has; plaintext
/// never crosses back.
fn provider_key_name(provider: &str) -> Result<String, String> {
    let p = provider.trim();
    if p.is_empty() || !p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err("invalid provider".into());
    }
    Ok(format!("PROVIDER_KEY_{}", p.to_uppercase().replace('-', "_")))
}

#[tauri::command]
pub fn provider_key_set(app: AppHandle, provider: String, value: String) -> Result<bool, String> {
    let key = provider_key_name(&provider)?;
    if value.is_empty() {
        return Err("key required".into());
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
    Ok(true)
}

#[tauri::command]
pub fn provider_key_has(app: AppHandle, provider: String) -> Result<bool, String> {
    let _ = app;
    Ok(secret_get(&provider_key_name(&provider)?).is_some())
}

#[tauri::command]
pub fn provider_key_clear(app: AppHandle, provider: String) -> Result<bool, String> {
    let key = provider_key_name(&provider)?;
    if let Ok(e) = keyring::Entry::new(KEYRING_SERVICE, &key) {
        let _ = e.delete_credential();
    }
    let st = state(&app);
    let mut cfg = st.config.lock().unwrap();
    cfg.secret_env_keys.retain(|k| k != &key);
    st.config_store.save(&cfg).map_err(err_str)?;
    Ok(true)
}

// ─── webhooks (server lifecycle + endpoint bookkeeping) ─────────────────────

/// Whether the serve thread has been spawned this session, and whether
/// webhook_stop has consumed the server's stop latch. WebhookServer::run
/// never resets that latch, so a stop in this port is one-way for the session.
static WEBHOOK_BOUND: AtomicBool = AtomicBool::new(false);
static WEBHOOK_LATCHED: AtomicBool = AtomicBool::new(false);
/// The port webhook_start actually bound (may differ from config when the
/// caller passed an override); status probes this, falling back to config.
static WEBHOOK_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

/// Honest liveness check — the port may have been bound at boot (lib.rs
/// doesn't touch our statics), so "running" = something answers on it.
fn webhook_listening(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(200),
    )
    .is_ok()
}

/// The public tunnel (upstream tunnelmole) — kept so its process stays alive;
/// stopped with the server. Slack keeps a parallel latch below.
static WEBHOOK_TUNNEL: std::sync::Mutex<Option<(String, std::process::Child)>> =
    std::sync::Mutex::new(None);

/// webhook:start — bind the loopback gateway against the configured endpoints
/// and port (upstream reconcileWebhookServer), then open the public tunnel.
/// The tunnel is non-fatal — the loopback handler is the security boundary.
#[tauri::command]
pub fn webhook_start(app: AppHandle, port: Option<u16>) -> Result<Value, String> {
    if WEBHOOK_LATCHED.load(Ordering::SeqCst) {
        return Err(
            "webhook server was stopped this session; restart the app to rebind".into(),
        );
    }
    let st = state(&app);
    let (eps, cfg_port) = {
        let cfg = st.config.lock().unwrap();
        (cfg.webhooks.clone(), cfg.webhook_port)
    };
    let port = port.unwrap_or(cfg_port);
    if st.webhook.set_endpoints(&eps) == 0 {
        return Err("no enabled webhook endpoints — save endpoints first".into());
    }
    WEBHOOK_PORT.store(port, Ordering::SeqCst);
    if WEBHOOK_BOUND.swap(true, Ordering::SeqCst) || webhook_listening(port) {
        return Ok(json!({ "running": true, "port": port }));
    }
    let wh = st.webhook.clone();
    let hive = st.hive.clone();
    let app2 = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = wh.run(port, hive, app2) {
            eprintln!("[webhook] server stopped: {e:#}");
        }
        WEBHOOK_BOUND.store(false, Ordering::SeqCst);
        if let Some((_, mut child)) = WEBHOOK_TUNNEL.lock().unwrap().take() {
            let _ = child.kill();
        }
    });
    // Public tunnel (openTunnel, non-fatal) — mint AFTER the bind so the URL
    // is never forwarded at a dead port.
    if WEBHOOK_TUNNEL.lock().unwrap().is_none() {
        match crate::misc::start_tunnel(port) {
            Ok((url, child)) => *WEBHOOK_TUNNEL.lock().unwrap() = Some((url, child)),
            Err(e) => eprintln!("[webhook] tunnel unavailable: {e}"),
        }
    }
    let tunnel_url = WEBHOOK_TUNNEL.lock().unwrap().as_ref().map(|(u, _)| u.clone());
    Ok(json!({ "running": true, "port": port, "url": tunnel_url }))
}

/// webhook:stop — signal the serve loop to exit (one-way latch; the port
/// rebinds only on the next app launch).
#[tauri::command]
pub fn webhook_stop(app: AppHandle) -> Result<bool, String> {
    state(&app).webhook.stop();
    WEBHOOK_BOUND.store(false, Ordering::SeqCst);
    WEBHOOK_LATCHED.store(true, Ordering::SeqCst);
    if let Some((_, mut child)) = WEBHOOK_TUNNEL.lock().unwrap().take() {
        let _ = child.kill();
    }
    Ok(true)
}

/// webhooks:status — {running, port, endpoints} (upstream also reported the
/// tunnel url + per-endpoint public urls; this port is loopback-only, so
/// `url` is the loopback base and `endpoints` is the served id list).
#[tauri::command]
pub fn webhook_status(app: AppHandle) -> Result<Value, String> {
    let st = state(&app);
    let cfg_port = st.config.lock().unwrap().webhook_port;
    let bound = WEBHOOK_PORT.load(Ordering::SeqCst);
    let port = if bound != 0 { bound } else { cfg_port };
    let tunnel_url = WEBHOOK_TUNNEL
        .lock()
        .unwrap()
        .as_ref()
        .map(|(u, _)| u.clone());
    Ok(json!({
        "running": webhook_listening(port),
        "url": tunnel_url.unwrap_or_else(|| format!("http://127.0.0.1:{port}")),
        "endpoints": st.webhook.endpoint_ids(),
    }))
}

/// webhooks:generateSecret — mint a 256-bit secret for the operator to paste
/// into their caller (not persisted here).
#[tauri::command]
pub fn webhook_generate_secret(app: AppHandle) -> Result<String, String> {
    let _ = app;
    Ok(random_hex32())
}

/// webhooks:delete — drop one endpoint by id and hot-swap the served list (a
/// removed id stops resolving on the very next request; the port never
/// rebinds).
#[tauri::command]
pub fn webhooks_delete(
    app: AppHandle,
    id: String,
) -> Result<Vec<crate::webhook::WebhookEndpoint>, String> {
    let st = state(&app);
    let list = {
        let mut cfg = st.config.lock().unwrap();
        cfg.webhooks.retain(|w| w.id != id);
        let list = cfg.webhooks.clone();
        st.config_store.save(&cfg).map_err(err_str)?;
        list
    };
    let _ = st.webhook.set_endpoints(&list);
    Ok(list)
}

// ─── slack config + reply helper ────────────────────────────────────────────

/// slack:setConfig — persist {channel_id, port, enabled} to the persist kv
/// (`slack_config`; the slimmed Config has no slack fields). Disabling stops
/// the running server, matching upstream's reconcile (we deliberately do NOT
/// auto-start on save — the user presses Start).
#[tauri::command]
pub fn slack_set_config(app: AppHandle, cfg: Value) -> Result<Value, String> {
    let st = state(&app);
    let mut cur = st
        .persist
        .lock()
        .unwrap()
        .get_kv("slack_config")
        .filter(|v| v.is_object())
        .unwrap_or_else(|| json!({}));
    let obj = cur.as_object_mut().unwrap(); // is_object filtered above
    if let Some(v) = cfg
        .get("channel_id")
        .or_else(|| cfg.get("channelId"))
        .and_then(|v| v.as_str())
    {
        obj.insert("channel_id".into(), json!(v.trim()));
    }
    if let Some(p) = cfg.get("port").and_then(|v| v.as_u64()) {
        if p <= u16::MAX as u64 {
            obj.insert("port".into(), json!(p));
        }
    }
    if let Some(e) = cfg.get("enabled").and_then(|v| v.as_bool()) {
        obj.insert("enabled".into(), json!(e));
    }
    let next = Value::Object(obj.clone());
    st.persist.lock().unwrap().set_kv("slack_config", &next);
    if next.get("enabled") == Some(&json!(false)) {
        st.slack.stop();
    }
    Ok(next)
}

/// slack:replyScriptPath — absolute path of the `<harness>/bin/slack-reply`
/// helper agents invoke to post back into a thread (written idempotently;
/// the bot token travels via the SLACK_BOT_TOKEN env, never an argv).
#[tauri::command]
pub fn slack_reply_script_path(app: AppHandle) -> Result<String, String> {
    let path = write_slack_reply_script(&harness_home(&app))?;
    Ok(path.display().to_string())
}

// ─── telemetry (the OTel collector's read surface) ─────────────────────────

/// telemetry:snapshot — cold-start backfill {usage, spans} for a renderer
/// that missed the live pushes.
#[tauri::command]
pub fn telemetry_snapshot(app: AppHandle) -> Result<Value, String> {
    Ok(state(&app).telemetry.snapshot())
}

/// telemetry:spans — one agent's recent tool-span ring, oldest→newest.
#[tauri::command]
pub fn telemetry_spans(app: AppHandle, agent_id: String) -> Result<Value, String> {
    Ok(json!(state(&app).telemetry.spans(&agent_id)))
}

/// telemetry:usage — one agent's cumulative usage with upstream's fallback
/// order: live OTel sample → proxy-sample ledger → transcript reconciler.
#[tauri::command]
pub fn telemetry_usage(app: AppHandle, agent_id: String) -> Result<Value, String> {
    let st = state(&app);
    let sample = st
        .telemetry
        .sample(&agent_id)
        .or_else(|| st.hive.usage_sample(&agent_id))
        .or_else(|| {
            st.hive
                .registry()
                .iter()
                .find(|a| a.id == agent_id)
                .and_then(|a| st.usage.sample(&a.cwd, a.session_id.as_deref()))
        });
    match sample {
        Some(s) => {
            let mut v = serde_json::to_value(&s).unwrap_or_else(|_| json!({}));
            v["agentId"] = json!(agent_id);
            Ok(v)
        }
        None => Ok(Value::Null),
    }
}

// ─── window / config / control extras ───────────────────────────────────────

/// window:newFloor — open a second office window on the same state. Labels
/// roll forward (floor-2, floor-3, …) so reopening never collides.
#[tauri::command]
pub fn window_new_floor(app: AppHandle) -> Result<Value, String> {
    let mut n = 2u32;
    let label = loop {
        let l = format!("floor-{n}");
        if app.get_webview_window(&l).is_none() {
            break l;
        }
        n += 1;
        if n > 64 {
            return Err("too many floors".into());
        }
    };
    tauri::WebviewWindowBuilder::new(&app, &label, tauri::WebviewUrl::App("index.html".into()))
        .title("Agent Office")
        .build()
        .map_err(|e| e.to_string())?;
    Ok(json!({ "ok": true, "label": label }))
}

/// config:changeHome — repointing the harness home requires re-init of every
/// hive-bound service (Hive, roster, persist, trigger history are all
/// constructed against the boot-time path in AppState::new). So this
/// validates + persists the override and defers the actual move to the next
/// launch. Upstream additionally offered 'move' semantics and relaunched in
/// place; this port is persist-and-flag only.
#[tauri::command]
pub fn config_change_home(app: AppHandle, new_home: String) -> Result<Value, String> {
    let expanded = expand_tilde(&new_home);
    if expanded.is_empty() {
        return Err("invalid newHome".into());
    }
    let newp = PathBuf::from(&expanded);
    if !newp.is_absolute() {
        return Err("newHome must be an absolute path".into());
    }
    std::fs::create_dir_all(&newp).map_err(|e| e.to_string())?;
    // Guard against same-folder / nested-folder targets (upstream's move
    // would self-copy; here the guard still stops a confusing repoint).
    let cur = harness_home(&app);
    let canon_new = newp.canonicalize().unwrap_or_else(|_| newp.clone());
    let canon_cur = cur.canonicalize().unwrap_or_else(|_| cur.clone());
    if canon_new == canon_cur {
        return Err("That is already the current home folder.".into());
    }
    if canon_new.starts_with(&canon_cur) || canon_cur.starts_with(&canon_new) {
        return Err(
            "Pick a folder that is not inside (or a parent of) the current home.".into(),
        );
    }
    let st = state(&app);
    st.persist
        .lock()
        .unwrap()
        .set_kv("harness_home", &json!(canon_new.display().to_string()));
    Ok(json!({
        "ok": true,
        "requiresRestart": true,
        "home": canon_new.display().to_string(),
    }))
}

/// config:ensureHome — create the harness home (and its hive/ subdir) on
/// disk; `~` expands first, same defense-in-depth rule as upstream (#140).
#[tauri::command]
pub fn config_ensure_home(app: AppHandle, path: String) -> Result<Value, String> {
    let _ = app;
    let p = expand_tilde(&path);
    if p.is_empty() {
        return Err("invalid path".into());
    }
    std::fs::create_dir_all(&p).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(Path::new(&p).join("hive")).map_err(|e| e.to_string())?;
    Ok(json!({ "ok": true, "path": p }))
}

/// config:homeSync — the current harness home path.
#[tauri::command]
pub fn config_home_sync(app: AppHandle) -> Result<String, String> {
    Ok(harness_home(&app).display().to_string())
}

/// config:setAgentTokenCap — set/clear one agent's work-token ceiling
/// (None/0 clears; upstream's MAX_AGENT_TOKEN_CAP = 10e9). Read-modify-write
/// on the live config so concurrent cap writes merge rather than clobber.
#[tauri::command]
pub fn config_set_agent_token_cap(
    app: AppHandle,
    agent_id: String,
    cap: Option<u64>,
) -> Result<Value, String> {
    const MAX_AGENT_TOKEN_CAP: u64 = 10_000_000_000;
    let id = agent_id.trim();
    if id.is_empty() {
        return Err("invalid agent token cap".into());
    }
    if let Some(v) = cap {
        if v == 0 || v > MAX_AGENT_TOKEN_CAP {
            return Err("invalid agent token cap".into());
        }
    }
    let st = state(&app);
    let mut cfg = st.config.lock().unwrap();
    match cap {
        Some(v) => {
            cfg.agent_token_caps.insert(id.to_string(), v);
        }
        None => {
            cfg.agent_token_caps.remove(id);
        }
    }
    st.config_store.save(&cfg).map_err(err_str)?;
    serde_json::to_value(&*cfg).map_err(|e| e.to_string())
}

/// control:setBreakerState — upstream fanned a BreakerState from the breaker
/// policy out to the renderer on `control:breakerState` (the avatar adapter
/// reads it). The breaker policy in this port already emits `breaker-state`
/// from the beat; this command keeps the renderer-writable seam — the payload
/// is relayed verbatim plus the agent id.
#[tauri::command]
pub fn control_set_breaker_state(
    app: AppHandle,
    agent_id: String,
    enabled: bool,
) -> Result<Value, String> {
    let st = state(&app);
    let _ = app.emit(
        "control:breakerState",
        json!({ "agentId": agent_id, "enabled": enabled }),
    );
    let snap = st.control.lock().unwrap().snapshot(&agent_id);
    Ok(json!(snap))
}

/// control:autoDelivery — set one agent's auto-delivery flag. `enabled`
/// here is the POSITIVE toggle (auto-delivery on); the control registry stores
/// the inverted `auto_delivery_paused` bit. Like upstream, the paused-agent
/// id list is also persisted (persist kv `auto_delivery_paused_agents`) so a
/// restart keeps the holds.
#[tauri::command]
pub fn control_auto_delivery(
    app: AppHandle,
    agent_id: String,
    enabled: bool,
) -> Result<Value, String> {
    let st = state(&app);
    let snap = {
        let mut c = st.control.lock().unwrap();
        c.pause_auto_delivery(&agent_id, !enabled);
        c.snapshot(&agent_id)
    };
    let key = "auto_delivery_paused_agents";
    let mut list: Vec<String> = st
        .persist
        .lock()
        .unwrap()
        .get_kv(key)
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    if enabled {
        list.retain(|id| id != &agent_id);
    } else if !list.iter().any(|id| id == &agent_id) {
        list.push(agent_id.clone());
    }
    list.sort();
    st.persist.lock().unwrap().set_kv(key, &json!(list));
    Ok(json!(snap))
}

// ─── triggers — context rules + the org blob ────────────────────────────────

/// Upstream ContextTriggerConfig default for the `clear` half (this port's
/// running trigger only consumes the compact half — ContextTriggerCfg — but
/// the stored blob keeps both so a round-trip through the UI loses nothing).
fn default_context_blob() -> Value {
    let ct = crate::config::ContextTriggerCfg::default();
    json!({
        "compact": {
            "enabled": ct.enabled,
            "everyMs": ct.every_ms,
            "minContextPct": ct.min_context_pct,
            "minContextPctLargeWindow": 40,
            "message": ct.message,
        },
        "clear": {
            "enabled": false,
            "everyMs": 7_200_000u64,
            "minContextPct": 90,
            "minContextPctLargeWindow": 80,
            "message": "",
        },
    })
}

/// Clamp like upstream sanitizeContextRule — the renderer is not trusted with
/// the arming maths (a zero/negative everyMs would arm a runaway tick).
fn sanitize_context_rule(patch: &Value, current: &Value) -> Value {
    let num = |key: &str, min: u64, max: u64| -> u64 {
        patch
            .get(key)
            .and_then(|v| v.as_f64())
            .filter(|n| n.is_finite())
            .map(|n| (n.round() as i64).clamp(min as i64, max as i64) as u64)
            .unwrap_or_else(|| {
                current
                    .get(key)
                    .and_then(|v| v.as_u64())
                    .unwrap_or(min)
            })
    };
    json!({
        "enabled": patch
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or_else(|| current.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false)),
        "everyMs": num("everyMs", 60_000, 86_400_000),
        "minContextPct": num("minContextPct", 0, 100),
        "minContextPctLargeWindow": num("minContextPctLargeWindow", 0, 100),
        "message": patch
            .get("message")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| {
                current
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string()
            }),
    })
}

/// triggers:getContext — the stored {compact, clear} blob (persist kv
/// `triggers_context`), else derived defaults from the live config.
#[tauri::command]
pub fn triggers_get_context(app: AppHandle) -> Result<Value, String> {
    let st = state(&app);
    if let Some(v) = st.persist.lock().unwrap().get_kv("triggers_context") {
        if v.is_object() {
            return Ok(v);
        }
    }
    let ct = st.config.lock().unwrap().context_trigger.clone();
    let mut d = default_context_blob();
    d["compact"] = json!({
        "enabled": ct.enabled,
        "everyMs": ct.every_ms,
        "minContextPct": ct.min_context_pct,
        "minContextPctLargeWindow": 40,
        "message": ct.message,
    });
    Ok(d)
}

/// triggers:setContext — merge + clamp both halves, persist the blob, and
/// mirror the compact half into cfg.context_trigger so the running trigger
/// (lib.rs's beat) sees the new cadence on its next tick.
#[tauri::command]
pub fn triggers_set_context(app: AppHandle, ctx: Value) -> Result<Value, String> {
    let st = state(&app);
    let current = triggers_get_context(app.clone())?;
    let compact_patch = ctx.get("compact").cloned().unwrap_or_else(|| json!({}));
    let clear_patch = ctx.get("clear").cloned().unwrap_or_else(|| json!({}));
    let next = json!({
        "compact": sanitize_context_rule(&compact_patch, &current["compact"]),
        "clear": sanitize_context_rule(&clear_patch, &current["clear"]),
    });
    st.persist
        .lock()
        .unwrap()
        .set_kv("triggers_context", &next);
    {
        let mut cfg = st.config.lock().unwrap();
        let c = &next["compact"];
        cfg.context_trigger.enabled = c["enabled"].as_bool().unwrap_or(false);
        cfg.context_trigger.every_ms = c["everyMs"].as_u64().unwrap_or(cfg.context_trigger.every_ms);
        cfg.context_trigger.min_context_pct =
            c["minContextPct"].as_u64().unwrap_or(cfg.context_trigger.min_context_pct);
        if let Some(m) = c["message"].as_str() {
            cfg.context_trigger.message = m.to_string();
        }
        st.config_store.save(&cfg).map_err(err_str)?;
    }
    Ok(next)
}

/// org:getTrigger — the org/clone-node blob (persist kv `org_trigger`;
/// upstream persisted-only too — no transport service exists yet).
#[tauri::command]
pub fn org_get_trigger(app: AppHandle) -> Result<Value, String> {
    let st = state(&app);
    let v = st
        .persist
        .lock()
        .unwrap()
        .get_kv("org_trigger")
        .filter(|v| v.is_object())
        .unwrap_or_else(|| json!({ "apiKey": "", "enabled": false, "mode": "strict" }));
    Ok(v)
}

/// org:setTrigger — persist-only merge; valid modes are the shared
/// TriggerMode set. Deliberately no side effects (upstream's comment
/// verbatim: nothing reads apiKey beyond the settings surface).
#[tauri::command]
pub fn org_set_trigger(app: AppHandle, arg: Value) -> Result<Value, String> {
    let st = state(&app);
    let current = org_get_trigger(app.clone())?;
    let valid_mode = |v: &str| matches!(v, "strict" | "allow-all" | "communication-only");
    let next = json!({
        "apiKey": arg
            .get("apiKey")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| current["apiKey"].as_str().unwrap_or_default().to_string()),
        "enabled": arg
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or_else(|| current["enabled"].as_bool().unwrap_or(false)),
        "mode": arg
            .get("mode")
            .and_then(|v| v.as_str())
            .filter(|m| valid_mode(m))
            .map(String::from)
            .unwrap_or_else(|| current["mode"].as_str().unwrap_or("strict").to_string()),
    });
    st.persist
        .lock()
        .unwrap()
        .set_kv("org_trigger", &next);
    Ok(next)
}
