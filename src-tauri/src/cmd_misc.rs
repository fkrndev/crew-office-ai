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

use crate::lock_ext::LockExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::commands::state;
use crate::misc::{
    err_str, expand_tilde, harness_home, is_filesystem_root, load_hero, mime_for, os_open,
    random_hex32, write_slack_reply_script,
};

/// fs_read_binary's byte cap — same 2MB guard the confined reader uses.
const MAX_BINARY_READ: u64 = 2 * 1024 * 1024;

// ─── shell / clipboard ──────────────────────────────────────────────────────

/// app:openExternal — open a URL in the system handler. Scheme-gated to
/// https/http/mailto so the renderer can never shell an arbitrary scheme
/// (upstream allowed https + the two OS settings schemes; this port keeps
/// mailto for the contact links and drops the settings schemes).
#[tauri::command(async)]
pub fn app_open_external(app: AppHandle, url: String) -> Result<bool, String> {
    crate::panic_guard::cmd_guard("app_open_external", || {
    let url = url.trim().to_string();
    let lower = url.to_lowercase();
    // Upstream allowlist (index.ts app:openExternal): Settings panes or
    // https only — the renderer can't shell arbitrary schemes.
    if !(lower.starts_with("x-apple.systempreferences:")
        || lower.starts_with("ms-settings:")
        || lower.starts_with("https://"))
    {
        return Err("blocked url".into());
    }
    if let Err(e) = app.opener().open_url(url.clone(), None::<&str>) {
        // Fallback: the OS's own opener. A renderer-provided URL reaching this
        // path is already scheme-gated above.
        os_open(&url).map_err(|e2| format!("{e}; fallback failed: {e2}"))?;
    }
    Ok(true)
    })
}

/// app:copyToClipboard.
#[tauri::command(async)]
pub fn app_clipboard_write(app: AppHandle, text: String) -> Result<bool, String> {
    crate::panic_guard::cmd_guard("app_clipboard_write", || {
    app.clipboard()
        .write_text(text)
        .map_err(|e| e.to_string())?;
    Ok(true)
    })
}

/// app:readClipboard — upstream swallows read errors to ''; keep that.
#[tauri::command(async)]
pub fn app_clipboard_read(app: AppHandle) -> Result<String, String> {
    crate::panic_guard::cmd_guard("app_clipboard_read", || {
    Ok(app.clipboard().read_text().unwrap_or_default())
    })
}

/// Persist a pasted image to the OS clipboard — the paste-flow companion of
/// the file dialog: the renderer hands PNG bytes (base64, optionally a
/// `data:` URL) and the OS clipboard gets the decoded image.
#[tauri::command(async)]
pub fn clipboard_save_image(app: AppHandle, base64: String) -> Result<bool, String> {
    crate::panic_guard::cmd_guard("clipboard_save_image", || {
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
    app.clipboard()
        .write_image(&img)
        .map_err(|e| e.to_string())?;
    Ok(true)
    })
}

/// Decoded-size cap for a pasted image (the renderer sends whole screenshots).
const MAX_PASTED_IMAGE_BYTES: usize = 20 * 1024 * 1024;
/// Pasted-image retention: files older than this or beyond the newest
/// `PASTED_IMAGES_KEEP` are pruned on every new paste.
const PASTED_IMAGES_MAX_AGE_SECS: u64 = 7 * 24 * 3600;
const PASTED_IMAGES_KEEP: usize = 100;

/// File extension for recognised raster-image magic bytes, None otherwise.
fn sniff_image_ext(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("gif")
    } else if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// Delete `paste-*` files in `dir` older than `max_age` or beyond the newest
/// `keep`. Best-effort — a locked file just survives to the next sweep.
fn prune_pasted_images(dir: &Path, max_age: std::time::Duration, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let now = std::time::SystemTime::now();
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("paste-"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file().then(|| (m.modified().unwrap_or(now), e.path()))
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    for (i, (mtime, path)) in files.iter().enumerate() {
        let age = now.duration_since(*mtime).unwrap_or_default();
        if i >= keep || age > max_age {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Decode a pasted image (base64, optionally a `data:image/...;base64,` URL),
/// validate size + magic bytes, and write it to a fresh file under `dir`.
/// Returns the file's absolute path.
fn save_pasted_image(dir: &Path, base64: &str) -> Result<PathBuf, String> {
    let s = base64.trim();
    let s = match s.find("base64,") {
        Some(i) => &s[i + 7..],
        None => s,
    }
    .trim();
    // Reject before decoding so a hostile payload can't balloon memory.
    if s.len() / 4 * 3 > MAX_PASTED_IMAGE_BYTES + 3 {
        return Err("image too large (max 20 MB)".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| format!("base64: {e}"))?;
    if bytes.len() > MAX_PASTED_IMAGE_BYTES {
        return Err("image too large (max 20 MB)".into());
    }
    let ext = sniff_image_ext(&bytes).ok_or_else(|| "not a recognised image".to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let uid = uuid::Uuid::new_v4().simple().to_string();
    let path = dir.join(format!("paste-{}-{}.{ext}", crate::misc::now_ms(), &uid[..8]));
    std::fs::write(&path, &bytes).map_err(|e| format!("write {}: {e}", path.display()))?;
    prune_pasted_images(
        dir,
        std::time::Duration::from_secs(PASTED_IMAGES_MAX_AGE_SECS),
        PASTED_IMAGES_KEEP,
    );
    Ok(path)
}

/// Paste-flow companion to `clipboard_save_image`: persist a pasted
/// screenshot under `<harness home>/pasted-images/` so the renderer can attach
/// it by path. Returns `{ok: true, path}` (absolute).
#[tauri::command(async)]
pub fn clipboard_image_to_file(app: AppHandle, base64: String) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("clipboard_image_to_file", || {
    let dir = harness_home(&app).join("pasted-images");
    let path = save_pasted_image(&dir, &base64)?;
    Ok(json!({ "ok": true, "path": path.to_string_lossy() }))
    })
}

// ─── app lifecycle / desktop integration ────────────────────────────────────

/// The per-hive namespace id the Codex data exposure uses: the first 12 hex
/// chars of sha1(hive root path).
fn codex_hive_id(hive_root: &Path) -> String {
    use sha1::{Digest, Sha1};
    let h = format!("{:x}", Sha1::digest(hive_root.display().to_string().as_bytes()));
    h[..12.min(h.len())].to_string()
}

/// Remove this hive's exposed Codex data — `<codex>/{sessions,
/// archived_sessions}/agent-office/<hive-id>` — and drop the now-empty
/// `agent-office` parents. Returns the directories removed. `user_codex` is
/// injectable (tests never touch the real ~/.codex).
pub(crate) fn remove_exposed_codex_data(user_codex: &Path, hive_root: &Path) -> Vec<PathBuf> {
    let id = codex_hive_id(hive_root);
    let mut gone = Vec::new();
    for kind in ["sessions", "archived_sessions"] {
        let ns = user_codex.join(kind).join("agent-office");
        let dir = ns.join(&id);
        if dir.exists() && std::fs::remove_dir_all(&dir).is_ok() {
            gone.push(dir);
        }
        // Tidy: remove the namespace dir only when nothing else lives in it.
        let _ = std::fs::remove_dir(&ns);
    }
    gone
}

/// app:resetAll — stop everything, then wipe the harness-owned state under
/// the configured home: `hive/`, `roster.json` (archived first, never
/// destroyed outright), `harness.db`. The home dir itself and anything else
/// in it is untouched. Refuses outright when home resolves to a filesystem
/// root — a missing/misconfigured home must never escalate into a volume
/// wipe. On success this NEVER returns — config.json goes back to first-run
/// defaults and the process restarts into onboarding (upstream's
/// `resetConfig()` + `app.relaunch()` + `app.exit(0)`; the renderer's await
/// intentionally never resolves).
#[tauri::command(async)]
pub fn app_reset_all(app: AppHandle) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("app_reset_all", || {
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
        let mut ptys = st.ptys.lock_safe();
        let drained: Vec<crate::pty::PtyHandle> = ptys.drain().map(|(_, h)| h).collect();
        let n = drained.len();
        drop(ptys);
        for mut h in drained {
            let _ = h.kill();
        }
        n
    };
    // The wholesale drain bypasses teardownPty — reset the pty↔agent maps
    // directly so a stale mapping can't outlive the reset.
    st.pty_to_agent.lock_safe().clear();
    st.agent_to_pty.lock_safe().clear();

    // 3. The roster retires into roster-backups/ — never deleted outright.
    st.roster.archive();

    // 3b. Close the persist connection before deleting harness.db — on
    //     Windows the SQLite file stays locked while the connection holds
    //     it, so this must happen before the wipe below.
    st.persist.lock_safe().close();

    // 4. Remove the harness-owned files. Each step is independent and
    //    reported, not fatal — a half-reset that reports is better than an
    //    all-or-nothing that hangs.
    let mut removed: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let hive_dir = home.join("hive");
    if hive_dir.exists() {
        match std::fs::remove_dir_all(&hive_dir) {
            Ok(()) => removed.push(hive_dir.display().to_string()),
            Err(e) => errors.push(format!("{}: {e}", hive_dir.display())),
        }
    }
    // Codex rollouts the hive exposed under the user's own scan roots
    // (~/.codex/{sessions,archived_sessions}/agent-office/<hive-id>/…): they
    // belong to the agents just wiped. Only THIS hive's namespace goes — other
    // hives' data under the same roots stays. The boot step re-exposes what
    // a fresh hive creates.
    if let Some(user_codex) = dirs::home_dir().map(|h| h.join(".codex")) {
        for p in remove_exposed_codex_data(&user_codex, &st.hive.root) {
            removed.push(p.display().to_string());
        }
    }
    for name in [
        "roster.json",
        "harness.db",
        "harness.db-wal",
        "harness.db-shm",
    ] {
        let p = home.join(name);
        if p.exists() {
            match std::fs::remove_file(&p) {
                Ok(()) => removed.push(p.display().to_string()),
                Err(e) => errors.push(format!("{}: {e}", p.display())),
            }
        }
    }
    // 5. Back to first-run defaults, then restart into onboarding — upstream
    //    resetConfig() + app.relaunch() + exit(0). If the config write fails
    //    we surface it instead of rebooting into a half-wiped home that still
    //    claims onboarding is done.
    if let Err(e) = st.config_store.reset() {
        return Err(format!("reset: could not reset config.json: {e}"));
    }
    if !errors.is_empty() {
        // Wipe had partial failures — report instead of restarting, so the
        // user sees what was left behind rather than a silent half-reset.
        return Ok(json!({
            "ok": false,
            "stoppedSlack": true,
            "stoppedWebhook": true,
            "killedPtys": killed,
            "rosterArchived": true,
            "removed": removed,
            "errors": errors,
            "requiresRestart": true,
        }));
    }
    drop(st);
    app.restart(); // diverges — the renderer's await never resolves
    })
}

/// app:setLoginItem — Windows autostart via the Run key. Non-Windows has no
/// equivalent here yet → "not supported" (upstream used Electron's
/// setLoginItemSettings, which was the macOS path).
#[tauri::command(async)]
pub fn app_set_login_item(app: AppHandle, enabled: bool) -> Result<bool, String> {
    crate::panic_guard::cmd_guard("app_set_login_item", || {
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
    })
}

/// app:setNotifications — persist the desktop-notification toggle. The slimmed
/// Config has no field for it, so it lives in the persist kv (readable by the
/// hooks layer when a native-notification path lands).
#[tauri::command(async)]
pub fn app_set_notifications(app: AppHandle, enabled: bool) -> Result<bool, String> {
    crate::panic_guard::cmd_guard("app_set_notifications", || {
    // Upstream writeConfig({notifications}) — the toggle lives in typed
    // config so get_config round-trips it for the Settings UI.
    let st = state(&app);
    {
        let mut cfg = st.config.lock_safe();
        cfg.notifications = enabled;
        st.config_store.save(&cfg).map_err(err_str)?;
    }
    crate::commands::emit_config_changed(&app);
    Ok(true)
    })
}

/// app:forceQuit — the "force quit" buttons (closing time taking too long, a
/// wedged shutdown). Kills every live PTY (process trees included) and exits
/// the process; skips the graceful closing-time protocol by design.
#[tauri::command(async)]
pub fn app_force_quit(app: AppHandle) -> Result<bool, String> {
    crate::panic_guard::cmd_guard("app_force_quit", || {
    let st = state(&app);
    let handles: Vec<crate::pty::PtyHandle> = st
        .ptys
        .lock_safe()
        .drain()
        .map(|(_, h)| h)
        .collect();
    for mut h in handles {
        let _ = h.kill();
    }
    st.pty_to_agent.lock_safe().clear();
    st.agent_to_pty.lock_safe().clear();
    app.exit(0);
    Ok(true)
    })
}

/// terminal:openAtFolder — spawn a system terminal at a folder.
#[tauri::command(async)]
pub fn terminal_open_at_folder(app: AppHandle, path: String) -> Result<bool, String> {
    crate::panic_guard::cmd_guard("terminal_open_at_folder", || {
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
        // The folder rides Command::current_dir — never the command line:
        // cmd.exe would re-parse `&`/`^`/`%` in it as commands and wt.exe
        // treats `;` as a subcommand separator.
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        let path_env = crate::pty::user_shell_path();
        let spawned = if crate::pty::resolve_program("wt", &path_env).is_some() {
            std::process::Command::new("wt.exe")
                .args(["-d", "."])
                .current_dir(&p)
                .spawn()
        } else {
            std::process::Command::new("cmd.exe")
                .arg("/k")
                .current_dir(&p)
                .creation_flags(CREATE_NEW_CONSOLE)
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
        let path_env = crate::pty::user_shell_path();
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
    })
}

/// The binaries `tools_status` probes: the prerequisites, then one per agent
/// engine (the presets' `defaultCommand` binaries - kimi, copilot and
/// cursor-agent were missing). Keyed by binary name, which is how
/// `normalizeToolsProbe` joins them to the toolCatalog rows.
const PROBED_BINS: [&str; 17] = [
    "git", "uv", "node", "claude", "codex", "gemini", "grok", "kimi", "agy", "hermes", "pi",
    "opencode", "qwen", "crush", "openclaw", "copilot", "cursor-agent",
];

/// One bare probe row `{id, found, path}`.
fn probe_row(id: &str, path: Option<String>) -> Value {
    json!({ "id": id, "found": path.is_some(), "path": path })
}

/// tools:status - probe every external tool the setup catalog lists. Presence
/// is a PATH resolution, never a spawn (upstream's argument verbatim: several
/// of these boot a TUI when invoked bare). Rows stay BARE (`{id: <bin>, found,
/// path}`) - the shape `normalizeToolsProbe` joins to the catalog - plus a
/// `mempalace` row whose presence/`detail` come from the memory subsystem's
/// own status (authoritative: it knows whether the palace is initialised).
#[tauri::command(async)]
pub fn tools_status(app: AppHandle) -> Result<Vec<Value>, String> {
    crate::panic_guard::cmd_guard("tools_status", || {
    let path_env = crate::pty::user_shell_path();
    let mut rows: Vec<Value> = PROBED_BINS
        .iter()
        .map(|id| {
            let path = match crate::pty::resolve_program(id, &path_env) {
                Some(crate::pty::Resolved::Exe(p)) | Some(crate::pty::Resolved::Script(p)) => {
                    Some(p.display().to_string())
                }
                None => None,
            };
            probe_row(id, path)
        })
        .collect();
    // MemPalace: re-resolve (it may have been installed after boot), then
    // report availability + whether the palace has been built yet.
    let palace = crate::memory_palace::instance(&harness_home(&app));
    palace.reset_bin_cache();
    let mem = palace.status();
    let available = mem["available"].as_bool() == Some(true);
    let mut row = probe_row(
        "mempalace",
        available.then(|| mem["bin"].as_str().map(String::from)).flatten(),
    );
    row["found"] = json!(available);
    if available {
        row["detail"] = json!(if mem["initialized"].as_bool() == Some(true) {
            "palace initialised"
        } else {
            "installed - palace not built yet"
        });
    }
    rows.push(row);
    Ok(rows)
    })
}

// ─── fs helpers ─────────────────────────────────────────────────────────────

/// fs:statAbs — existence/metadata check for an ABSOLUTE path (the terminal
/// ⌘-click flow). Metadata only, never contents; `~/` is expanded here since
/// the renderer doesn't know the home dir.
#[tauri::command(async)]
pub fn fs_stat_abs(app: AppHandle, path: String) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("fs_stat_abs", || {
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
    })
}

/// Folders the user picked through `dialog_choose_folder` this session —
/// a real OS dialog gesture, so they count as write-approved roots (the
/// session-export flow writes into a folder that is not an agent workspace).
static PICKED_FOLDERS: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Every directory the renderer may legitimately use as an fs root: agent
/// cwds from the registry (+ their main repo), live isolation worktrees, the
/// harness home, and dialog-picked folders. Canonicalized for comparison.
fn known_workspace_roots(st: &crate::commands::AppState) -> Vec<PathBuf> {
    let mut raw: Vec<PathBuf> = Vec::new();
    for a in st.hive.registry() {
        if a.cwd.is_empty() {
            continue;
        }
        let cwd = PathBuf::from(expand_tilde(&a.cwd));
        if let Some(main) = crate::git_ops::main_repo_root(&cwd) {
            raw.push(PathBuf::from(main));
        }
        raw.push(cwd);
    }
    for (wt, orig) in st.worktree_paths.lock_safe().values() {
        raw.push(wt.clone());
        raw.push(orig.clone());
    }
    raw.push(st.hive.root.parent().unwrap_or(&st.hive.root).to_path_buf());
    raw.extend(PICKED_FOLDERS.lock_safe().iter().cloned());
    raw.into_iter()
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .collect()
}

/// `root` must canonicalize to (or inside) one of `known` — a renderer-chosen
/// root would otherwise make safe_resolve's confinement meaningless.
fn root_in_workspaces(root: &Path, known: &[PathBuf]) -> bool {
    let Ok(real) = std::fs::canonicalize(root) else {
        return false;
    };
    known.iter().any(|k| real.starts_with(k))
}

/// fs:readBinary — raw bytes for an absolute path the text reader refuses
/// (images et al.), base64 on the wire. Same pre-stat + post-read cap as
/// upstream so a file that grows mid-read can't slip past the memory
/// guarantee. Only image/PDF types (what the previews render) — this is not a
/// general file-exfiltration read.
#[tauri::command(async)]
pub fn fs_read_binary(app: AppHandle, path: String) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("fs_read_binary", || {
    let _ = app;
    if path.is_empty() || path.len() > 4096 || path.contains('\0') {
        return Err("bad request".into());
    }
    let abs = expand_tilde(&path);
    let mime = mime_for(Path::new(&abs));
    if !(mime.starts_with("image/") || mime == "application/pdf") {
        return Err("unsupported file type".into());
    }
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
    })
}

/// fs:writeFile — confined write through fs_ops::safe_resolve (the one
/// path-escape policy); parent dirs are created so a new nested file lands.
#[tauri::command(async)]
pub fn fs_write_file(
    app: AppHandle,
    root: String,
    rel: String,
    content: String,
) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("fs_write_file", || {
    // The root comes from the renderer, so it is only trusted when it IS a
    // known workspace — otherwise "confined" would mean "confined to
    // whatever the caller says".
    let root = expand_tilde(&root);
    if !root_in_workspaces(Path::new(&root), &known_workspace_roots(&state(&app))) {
        return Err("path escapes the workspace".into());
    }
    let abs = crate::fs_ops::safe_resolve(Path::new(&root), &rel)
        .ok_or_else(|| "path escapes the workspace".to_string())?;
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&abs, content).map_err(|e| e.to_string())?;
    Ok(json!({ "ok": true, "path": abs.display().to_string() }))
    })
}

/// fs:revealPath — reveal in the OS file browser. A FILE is only ever
/// revealed (explorer /select, open -R, xdg-open on the parent) — opening the
/// file itself would hand an arbitrary path from agent output to its default
/// application. A DIRECTORY is opened (a folder has no default app to
/// exploit).
#[tauri::command(async)]
pub fn fs_reveal_path(app: AppHandle, path: String) -> Result<bool, String> {
    crate::panic_guard::cmd_guard("fs_reveal_path", || {
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
    })
}

// ─── dialogs ────────────────────────────────────────────────────────────────

/// dialog:chooseFolder — single-directory picker; upstream {ok,path} shape.
#[tauri::command]
pub fn dialog_choose_folder(app: AppHandle) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("dialog_choose_folder", || {
    match app
        .dialog()
        .file()
        .set_title("Pick a folder")
        .blocking_pick_folder()
    {
        Some(fp) => {
            let picked = fp.to_string();
            // A user-picked folder is a legitimate fs_write_file root.
            if let Ok(real) = std::fs::canonicalize(&picked) {
                let mut roots = PICKED_FOLDERS.lock_safe();
                if !roots.contains(&real) {
                    roots.push(real);
                }
            }
            Ok(json!({ "ok": true, "path": picked }))
        }
        None => Ok(json!({ "ok": false, "error": "cancelled" })),
    }
    })
}

/// dialog:attachFiles — multi-select file picker (Images group + All Files);
/// upstream {ok, files:[{path,name}]} shape.
#[tauri::command]
pub fn dialog_attach_files(app: AppHandle) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("dialog_attach_files", || {
    match app
        .dialog()
        .file()
        .set_title("Attach images or files")
        .add_filter(
            "Images",
            &[
                "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "heic", "tiff", "avif",
            ],
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
    })
}

// ─── catalogs + hero payload ────────────────────────────────────────────────

/// models:catalog — the REMOTE model catalog, `{catalog, fetchedAt, stale}`
/// (6h TTL cache under the harness home, never fatal). A null catalog means
/// "keep the catalog compiled into this build" — the renderer's baked floor.
#[tauri::command(async)]
pub async fn models_catalog(app: AppHandle, force: Option<bool>) -> Result<Value, String> {
    crate::panic_guard::cmd_guard_async("models_catalog", async move {
    let cache = harness_home(&app).join("model-catalog.json");
    let force = force.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || crate::misc::load_model_catalog(&cache, force))
        .await
        .map_err(|e| e.to_string())
    }).await
}

/// hero:payload — the Settings hero card, fetched + cached with a 6h TTL and
/// a never-fail contract ({hero, fetchedAt, stale}). Off the UI thread since
/// a cold cache means a bounded network fetch.
#[tauri::command(async)]
pub async fn hero_payload(app: AppHandle, force: Option<bool>) -> Result<Value, String> {
    crate::panic_guard::cmd_guard_async("hero_payload", async move {
    // Not `hero.json`: earlier builds cached the upstream project's payload
    // there, and a stale copy would otherwise outlive a failing fetch forever.
    let cache = harness_home(&app).join("hero-agent-office.json");
    let force = force.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || load_hero(&cache, force))
        .await
        .map_err(|e| e.to_string())
    }).await
}

// ─── roster ─────────────────────────────────────────────────────────────────

/// roster:readSync — same read as roster_read (upstream kept a sync channel
/// for the renderer's first paint; Tauri IPC is async either way, but the
/// name is the contract).
#[tauri::command(async)]
pub fn roster_read_sync(
    app: AppHandle,
    window: tauri::WebviewWindow,
) -> Result<Option<Value>, String> {
    crate::panic_guard::cmd_guard("roster_read_sync", || {
    if window.label() != crate::floors::MAIN_LABEL {
        return Ok(None); // a floor's roster is its own (see roster_read)
    }
    Ok(state(&app).roster.read())
    })
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
    if p.is_empty()
        || !p
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("invalid provider".into());
    }
    Ok(format!(
        "PROVIDER_KEY_{}",
        p.to_uppercase().replace('-', "_")
    ))
}

#[tauri::command(async)]
pub fn provider_key_set(app: AppHandle, provider: String, value: String) -> Result<bool, String> {
    crate::panic_guard::cmd_guard("provider_key_set", || {
    let key = provider_key_name(&provider)?;
    if value.is_empty() {
        return Err("key required".into());
    }
    // Verify-after-write, DPAPI-file fallback — same guarantee as secret_set:
    // config only lists the key once its value provably persists somewhere.
    let st = state(&app);
    crate::misc::secret_set_persist(&key, &value, st.config_store.dir())?;
    {
        let mut cfg = st.config.lock_safe();
        if !cfg.secret_env_keys.contains(&key) {
            cfg.secret_env_keys.push(key);
            st.config_store.save(&cfg).map_err(err_str)?;
        }
    }
    crate::commands::emit_config_changed(&app);
    Ok(true)
    })
}

#[tauri::command(async)]
pub fn provider_key_has(app: AppHandle, provider: String) -> Result<bool, String> {
    crate::panic_guard::cmd_guard("provider_key_has", || {
    let dir = state(&app).config_store.dir().to_path_buf();
    Ok(crate::misc::secret_get_persist(&provider_key_name(&provider)?, &dir).is_some())
    })
}

#[tauri::command(async)]
pub fn provider_key_clear(app: AppHandle, provider: String) -> Result<bool, String> {
    crate::panic_guard::cmd_guard("provider_key_clear", || {
    let key = provider_key_name(&provider)?;
    let st = state(&app);
    crate::misc::secret_delete_persist(&key, st.config_store.dir());
    {
        let mut cfg = st.config.lock_safe();
        cfg.secret_env_keys.retain(|k| k != &key);
        st.config_store.save(&cfg).map_err(err_str)?;
    }
    crate::commands::emit_config_changed(&app);
    Ok(true)
    })
}

// ─── webhooks (server lifecycle + endpoint bookkeeping) ─────────────────────

/// Whether the serve thread has been spawned this session, and whether
/// webhook_stop has consumed the server's stop latch. WebhookServer::run
/// never resets that latch, so a stop in this port is one-way for the session.
pub(crate) static WEBHOOK_BOUND: AtomicBool = AtomicBool::new(false);
pub(crate) static WEBHOOK_LATCHED: AtomicBool = AtomicBool::new(false);
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
#[tauri::command(async)]
pub fn webhook_start(app: AppHandle, port: Option<u16>) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("webhook_start", || {
    if WEBHOOK_LATCHED.load(Ordering::SeqCst) {
        return Err("webhook server was stopped this session; restart the app to rebind".into());
    }
    let st = state(&app);
    let (eps, cfg_port) = {
        let cfg = st.config.lock_safe();
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
            log::warn!("[webhook] server stopped: {e:#}");
        }
        WEBHOOK_BOUND.store(false, Ordering::SeqCst);
        if let Some((_, mut child)) = WEBHOOK_TUNNEL.lock_safe().take() {
            crate::misc::kill_process_tree(&mut child);
        }
    });
    // Public tunnel (openTunnel, non-fatal) — mint AFTER the bind so the URL
    // is never forwarded at a dead port.
    if WEBHOOK_TUNNEL.lock_safe().is_none() {
        match crate::misc::start_tunnel(port) {
            Ok((url, child)) => *WEBHOOK_TUNNEL.lock_safe() = Some((url, child)),
            Err(e) => log::warn!("[webhook] tunnel unavailable: {e}"),
        }
    }
    let tunnel_url = WEBHOOK_TUNNEL
        .lock_safe()
        .as_ref()
        .map(|(u, _)| u.clone());
    Ok(json!({ "running": true, "port": port, "url": tunnel_url }))
    })
}

/// webhook:stop — signal the serve loop to exit (one-way latch; the port
/// rebinds only on the next app launch).
#[tauri::command(async)]
pub fn webhook_stop(app: AppHandle) -> Result<bool, String> {
    crate::panic_guard::cmd_guard("webhook_stop", || {
    state(&app).webhook.stop();
    WEBHOOK_BOUND.store(false, Ordering::SeqCst);
    WEBHOOK_LATCHED.store(true, Ordering::SeqCst);
    if let Some((_, mut child)) = WEBHOOK_TUNNEL.lock_safe().take() {
        crate::misc::kill_process_tree(&mut child);
    }
    Ok(true)
    })
}

/// webhooks:status — {running, port, endpoints} (upstream also reported the
/// tunnel url + per-endpoint public urls; this port is loopback-only, so
/// `url` is the loopback base and `endpoints` is the served id list).
#[tauri::command(async)]
pub fn webhook_status(app: AppHandle) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("webhook_status", || {
    let st = state(&app);
    let cfg_port = st.config.lock_safe().webhook_port;
    let bound = WEBHOOK_PORT.load(Ordering::SeqCst);
    let port = if bound != 0 { bound } else { cfg_port };
    let tunnel_url = WEBHOOK_TUNNEL
        .lock_safe()
        .as_ref()
        .map(|(u, _)| u.clone());
    Ok(json!({
        "running": webhook_listening(port),
        "url": tunnel_url.unwrap_or_else(|| format!("http://127.0.0.1:{port}")),
        "endpoints": st.webhook.endpoint_ids(),
    }))
    })
}

/// webhooks:generateSecret — mint a 256-bit secret for the operator to paste
/// into their caller (not persisted here).
#[tauri::command(async)]
pub fn webhook_generate_secret(app: AppHandle) -> Result<String, String> {
    crate::panic_guard::cmd_guard("webhook_generate_secret", || {
    let _ = app;
    Ok(random_hex32())
    })
}

/// webhooks:delete — drop one endpoint by id and hot-swap the served list (a
/// removed id stops resolving on the very next request; the port never
/// rebinds).
#[tauri::command(async)]
pub fn webhooks_delete(
    app: AppHandle,
    id: String,
) -> Result<Vec<crate::webhook::WebhookEndpoint>, String> {
    crate::panic_guard::cmd_guard("webhooks_delete", || {
    let st = state(&app);
    let list = {
        let mut cfg = st.config.lock_safe();
        cfg.webhooks.retain(|w| w.id != id);
        let list = cfg.webhooks.clone();
        st.config_store.save(&cfg).map_err(err_str)?;
        list
    };
    let _ = st.webhook.set_endpoints(&list);
    crate::commands::emit_config_changed(&app);
    Ok(list)
    })
}

// ─── slack config + reply helper ────────────────────────────────────────────

/// slack:setConfig - upstream `writeConfig({slack*})`: channelId / port /
/// enabled / proactivePosting land in config.json (`slackChannelId`,
/// `slackPort`, `slackEnabled`, `slackProactivePosting` - what Settings
/// re-reads on reopen; the kv `slack_config` mirror is kept for older readers).
/// `signingSecret` / `botToken` go to the credential store, never to disk or
/// the agent env; a blank value leaves the stored secret alone (the UI cannot
/// show it back). Disabling - or having no signing secret - stops the running
/// adapter; we deliberately do NOT auto-start on save, the user presses Start.
#[tauri::command(async)]
pub fn slack_set_config(app: AppHandle, cfg: Value) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("slack_set_config", || {
    let st = state(&app);
    let trimmed = |v: Option<&Value>| -> Option<String> {
        v.and_then(|v| v.as_str()).map(|s| s.trim().to_string())
    };
    for (field, key) in [
        ("signingSecret", "SLACK_SIGNING_SECRET"),
        ("botToken", "SLACK_BOT_TOKEN"),
    ] {
        if let Some(v) = trimmed(cfg.get(field)).filter(|v| !v.is_empty()) {
            crate::misc::secret_set_persist(key, &v, st.config_store.dir())?;
        }
    }
    let mut patch = serde_json::Map::new();
    let mut mirror = st
        .persist
        .lock_safe()
        .get_kv("slack_config")
        .filter(|v| v.is_object())
        .unwrap_or_else(|| json!({}));
    let obj = mirror.as_object_mut().unwrap(); // is_object filtered above
    if let Some(c) = trimmed(cfg.get("channel_id").or_else(|| cfg.get("channelId"))) {
        obj.insert("channel_id".into(), json!(c));
        // An emptied field clears back to unset.
        patch.insert(
            "slackChannelId".into(),
            if c.is_empty() { Value::Null } else { json!(c) },
        );
    }
    if let Some(p) = cfg.get("port").and_then(|v| v.as_u64()) {
        if p <= u16::MAX as u64 {
            obj.insert("port".into(), json!(p));
            patch.insert("slackPort".into(), json!(p));
        }
    }
    if let Some(e) = cfg.get("enabled").and_then(|v| v.as_bool()) {
        obj.insert("enabled".into(), json!(e));
        patch.insert("slackEnabled".into(), json!(e));
    }
    if let Some(pp) = cfg.get("proactivePosting").and_then(|v| v.as_bool()) {
        patch.insert("slackProactivePosting".into(), json!(pp));
    }
    let next = Value::Object(obj.clone());
    st.persist.lock_safe().set_kv("slack_config", &next);
    if !patch.is_empty() {
        let typed = st
            .config_store
            .patch_raw(&patch)
            .map_err(|e| format!("could not save Slack settings: {e}"))?;
        *st.config.lock_safe() = typed;
    }
    // Reconcile the running adapter: disabled, or no signing secret -> stop.
    let raw = st.config_store.raw_value();
    let enabled = raw.get("slackEnabled").and_then(|v| v.as_bool()) == Some(true);
    let has_secret =
        crate::misc::secret_get_persist("SLACK_SIGNING_SECRET", st.config_store.dir()).is_some();
    if !enabled || !has_secret {
        crate::commands::slack_teardown(&app);
    }
    crate::commands::emit_config_changed(&app);
    Ok(next)
    })
}

/// slack:replyScriptPath — absolute path of the `<harness>/bin/slack-reply`
/// helper agents invoke to post back into a thread (written idempotently;
/// the bot token travels via the SLACK_BOT_TOKEN env, never an argv).
#[tauri::command(async)]
pub fn slack_reply_script_path(app: AppHandle) -> Result<String, String> {
    crate::panic_guard::cmd_guard("slack_reply_script_path", || {
    let path = write_slack_reply_script(&harness_home(&app))?;
    Ok(path.display().to_string())
    })
}

// ─── telemetry (the OTel collector's read surface) ─────────────────────────

/// telemetry:snapshot — cold-start backfill {usage, spans} for a renderer
/// that missed the live pushes.
#[tauri::command(async)]
pub fn telemetry_snapshot(app: AppHandle) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("telemetry_snapshot", || {
    Ok(state(&app).telemetry.snapshot())
    })
}

/// telemetry:spans — one agent's recent tool-span ring, oldest→newest.
#[tauri::command(async)]
pub fn telemetry_spans(app: AppHandle, agent_id: String) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("telemetry_spans", || {
    Ok(json!(state(&app).telemetry.spans(&agent_id)))
    })
}

/// telemetry:usage — one agent's cumulative usage with upstream's fallback
/// order: live OTel sample → proxy-sample ledger → transcript reconciler.
#[tauri::command(async)]
pub fn telemetry_usage(app: AppHandle, agent_id: String) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("telemetry_usage", || {
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
    })
}

// ─── window / config / control extras ───────────────────────────────────────

/// window:newFloor — open another office window ("floor", floors.rs), gated by
/// the `multiWindow` config flag (default OFF). Resolves `{ok}` like upstream
/// (`{ok:false}` when the flag is off); a real failure rejects. Async on
/// purpose: WebView2 deadlocks when a window is built from a synchronous
/// command.
#[tauri::command(async)]
pub fn window_new_floor(app: AppHandle, window: tauri::WebviewWindow) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("window_new_floor", || {
    if !crate::floors::multi_window_enabled(&app) {
        return Ok(json!({
            "ok": false,
            "error": "multi-window floors are off (Settings > General > Multiple windows)"
        }));
    }
    let label = crate::floors::open_floor(&app, Some(&window))?;
    Ok(json!({ "ok": true, "label": label }))
    })
}

/// window:closeFloor — the CONFIRMED close of a floor that still has live
/// terminals: stop only the PTYs this floor owns (their normal exit teardown
/// still runs), then destroy the window. Floors only — `main` closes through
/// the app-quit gate.
#[tauri::command(async)]
pub fn window_close_floor(app: AppHandle, window: tauri::WebviewWindow) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("window_close_floor", || {
    let label = window.label().to_string();
    if !crate::floors::is_floor_label(&label) {
        return Err("only a floor window can be closed this way".into());
    }
    let killed = crate::floors::kill_owned(&app, &label);
    window.destroy().map_err(|e| e.to_string())?;
    Ok(json!({ "ok": true, "killed": killed }))
    })
}

/// config:changeHome — repointing the harness home requires re-init of every
/// hive-bound service (Hive, roster, persist, trigger history are all
/// constructed against the boot-time path in AppState::new). So this
/// validates the target, carries the harness-owned state across on 'move',
/// records the pointer the next boot reads, and restarts the process —
/// upstream's writeConfig({harnessHome}) + killAll + app.relaunch(). On
/// success this NEVER returns (the renderer's await never resolves); a
/// return means an error the caller surfaces.
#[tauri::command(async)]
pub fn config_change_home(
    app: AppHandle,
    new_home: String,
    mode: Option<String>,
) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("config_change_home", || {
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
        return Err("Pick a folder that is not inside (or a parent of) the current home.".into());
    }
    let st = state(&app);

    // 'move' carries the harness-owned state across — hive (memory, tasks,
    // git history), palace, roster, db, config. 'fresh' leaves the target
    // empty so the relaunch lands on onboarding there.
    if mode.as_deref() == Some("move") {
        // Close the SQLite conn before touching harness.db — the files stay
        // locked on Windows while a connection holds them.
        st.persist.lock_safe().close();
        let mut copied: Vec<String> = Vec::new();
        let mut move_errs: Vec<String> = Vec::new();
        // EVERYTHING the home holds is state — hive, palace, knowledge,
        // secrets (DPAPI blobs), integrations/records, bin helpers,
        // slack-files, trigger history, roster, db, config … — so copy every
        // top-level entry rather than a hand-kept list that rots. Only live
        // git worktrees are left behind (their .git metadata points at the
        // old repo paths; a copy would be a broken checkout).
        const MOVE_SKIP: &[&str] = &["worktrees"];
        let entries = std::fs::read_dir(&cur).map_err(|e| format!("read home: {e}"))?;
        for entry in entries.flatten() {
            let name = entry.file_name();
            if MOVE_SKIP.iter().any(|s| name == std::ffi::OsStr::new(s)) {
                continue;
            }
            let src = entry.path();
            let dst = canon_new.join(&name);
            // Links are never followed (a junction inside would copy a whole
            // foreign tree, or fail the move on a dir link).
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let res = if ft.is_dir() {
                copy_dir_recursive(&src, &dst)
            } else {
                std::fs::copy(&src, &dst).map(|_| ())
            };
            match res {
                Ok(()) => copied.push(dst.display().to_string()),
                Err(e) => move_errs.push(format!("{}: {e}", src.display())),
            }
        }
        if !move_errs.is_empty() {
            return Err(format!("move failed: {}", move_errs.join("; ")));
        }
    }

    // Write the pointer where the NEXT boot reads it: `harnessHome` in the
    // FIXED default-home config.json (lib.rs resolves it before AppState
    // exists). Also push the old home onto recentHives for the picker.
    let default_home = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("harness");
    let pointer = crate::config::ConfigStore::new(default_home);
    let mut patch = serde_json::Map::new();
    patch.insert("harnessHome".into(), json!(canon_new.display().to_string()));
    let mut recents: Vec<Value> = pointer
        .raw_value()
        .get("recentHives")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let cur_s = canon_cur.display().to_string();
    recents.retain(|v| v.as_str() != Some(cur_s.as_str()));
    recents.insert(0, json!(cur_s));
    recents.truncate(8);
    patch.insert("recentHives".into(), json!(recents));
    pointer
        .patch_raw(&patch)
        .map_err(|e| format!("could not persist new home: {e}"))?;

    drop(st);
    app.restart(); // diverges — the renderer's await never resolves
    })
}

/// Recursive dir copy for 'move' mode — follows nothing exotic, plain files
/// and subdirs (upstream copyRecursive for hive + palace).
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            continue; // never follow links/junctions out of the tree
        }
        if ft.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// config:ensureHome — create the harness home (and its hive/ subdir) on
/// disk; `~` expands first, same defense-in-depth rule as upstream (#140).
#[tauri::command(async)]
pub fn config_ensure_home(app: AppHandle, path: String) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("config_ensure_home", || {
    let _ = app;
    let p = expand_tilde(&path);
    if p.is_empty() {
        return Err("invalid path".into());
    }
    std::fs::create_dir_all(&p).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(Path::new(&p).join("hive")).map_err(|e| e.to_string())?;
    Ok(json!({ "ok": true, "path": p }))
    })
}

/// config:homeSync — the current harness home path.
#[tauri::command(async)]
pub fn config_home_sync(app: AppHandle) -> Result<String, String> {
    crate::panic_guard::cmd_guard("config_home_sync", || {
    Ok(harness_home(&app).display().to_string())
    })
}

/// config:setAgentTokenCap — set/clear one agent's work-token ceiling
/// (None/0 clears; upstream's MAX_AGENT_TOKEN_CAP = 10e9). Read-modify-write
/// on the live config so concurrent cap writes merge rather than clobber.
#[tauri::command(async)]
pub fn config_set_agent_token_cap(
    app: AppHandle,
    agent_id: String,
    cap: Option<u64>,
) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("config_set_agent_token_cap", || {
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
    let mut cfg = st.config.lock_safe();
    match cap {
        Some(v) => {
            cfg.agent_token_caps.insert(id.to_string(), v);
        }
        None => {
            cfg.agent_token_caps.remove(id);
        }
    }
    st.config_store.save(&cfg).map_err(err_str)?;
    let out = serde_json::to_value(&*cfg).map_err(|e| e.to_string());
    drop(cfg);
    crate::commands::emit_config_changed(&app);
    out
    })
}

/// config:setAgentCostCap — set/clear one agent's USD spend ceiling
/// (None/0 clears; sanity ceiling $1M so a typo can't pin a $1e12 budget).
/// Mirrors config_set_agent_token_cap: read-modify-write on the live config.
#[tauri::command(async)]
pub fn config_set_agent_cost_cap(
    app: AppHandle,
    agent_id: String,
    cap: Option<f64>,
) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("config_set_agent_cost_cap", || {
    const MAX_AGENT_COST_CAP_USD: f64 = 1_000_000.0;
    let id = agent_id.trim();
    if id.is_empty() {
        return Err("invalid agent cost cap".into());
    }
    if let Some(v) = cap {
        if !v.is_finite() || v <= 0.0 || v > MAX_AGENT_COST_CAP_USD {
            return Err("invalid agent cost cap".into());
        }
    }
    let st = state(&app);
    let mut cfg = st.config.lock_safe();
    match cap {
        Some(v) => {
            cfg.agent_cost_caps.insert(id.to_string(), v);
        }
        None => {
            cfg.agent_cost_caps.remove(id);
        }
    }
    st.config_store.save(&cfg).map_err(err_str)?;
    let out = serde_json::to_value(&*cfg).map_err(|e| e.to_string());
    drop(cfg);
    crate::commands::emit_config_changed(&app);
    out
    })
}

/// control:setBreakerState — upstream fanned a BreakerState from the breaker
/// policy out to the renderer on `control:breakerState` (the avatar adapter
/// reads it). The breaker policy in this port already emits `breaker-state`
/// from the beat; this command keeps the renderer-writable seam — the payload
/// is relayed verbatim plus the agent id.
#[tauri::command(async)]
pub fn control_set_breaker_state(
    app: AppHandle,
    agent_id: String,
    enabled: bool,
) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("control_set_breaker_state", || {
    let st = state(&app);
    let _ = app.emit(
        "control:breakerState",
        json!({ "agentId": agent_id, "enabled": enabled }),
    );
    let snap = st.control.lock_safe().snapshot(&agent_id);
    Ok(json!(snap))
    })
}

/// control:autoDelivery — set one agent's auto-delivery hold. `paused` is the
/// frontend's argument (true = hold deliveries) and maps straight onto the
/// control registry's `auto_delivery_paused` bit. Like upstream, the
/// paused-agent id list is also persisted (persist kv
/// `auto_delivery_paused_agents`) so a restart keeps the holds.
#[tauri::command(async)]
pub fn control_auto_delivery(
    app: AppHandle,
    agent_id: String,
    paused: bool,
) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("control_auto_delivery", || {
    let st = state(&app);
    let snap = {
        let mut c = st.control.lock_safe();
        c.pause_auto_delivery(&agent_id, paused);
        c.snapshot(&agent_id)
    };
    let key = "auto_delivery_paused_agents";
    let mut list: Vec<String> = st
        .persist
        .lock_safe()
        .get_kv(key)
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    if !paused {
        list.retain(|id| id != &agent_id);
    } else if !list.iter().any(|id| id == &agent_id) {
        list.push(agent_id.clone());
    }
    list.sort();
    st.persist.lock_safe().set_kv(key, &json!(list));
    Ok(json!(snap))
    })
}

// ─── triggers — context rules + the org blob ────────────────────────────────

/// Upstream DEFAULT_CONTEXT_TRIGGER blob — both halves of the typed struct
/// serialize into exactly this shape.
fn default_context_blob() -> Value {
    serde_json::to_value(crate::config::ContextTriggerCfg::default()).unwrap_or_else(|_| json!({}))
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
            .unwrap_or_else(|| current.get(key).and_then(|v| v.as_u64()).unwrap_or(min))
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
#[tauri::command(async)]
pub fn triggers_get_context(app: AppHandle) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("triggers_get_context", || {
    let st = state(&app);
    if let Some(v) = st.persist.lock_safe().get_kv("triggers_context") {
        if v.is_object() {
            return Ok(v);
        }
    }
    let ct = st.config.lock_safe().context_trigger.clone();
    let mut d = default_context_blob();
    d["compact"] = serde_json::to_value(&ct.compact).unwrap_or_else(|_| json!({}));
    d["clear"] = serde_json::to_value(&ct.clear).unwrap_or_else(|_| json!({}));
    Ok(d)
    })
}

/// triggers:setContext — merge + clamp both halves, persist the blob, and
/// mirror the compact half into cfg.context_trigger so the running trigger
/// (lib.rs's beat) sees the new cadence on its next tick.
#[tauri::command(async)]
pub fn triggers_set_context(app: AppHandle, ctx: Value) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("triggers_set_context", || {
    let st = state(&app);
    let current = triggers_get_context(app.clone())?;
    let compact_patch = ctx.get("compact").cloned().unwrap_or_else(|| json!({}));
    let clear_patch = ctx.get("clear").cloned().unwrap_or_else(|| json!({}));
    let next = json!({
        "compact": sanitize_context_rule(&compact_patch, &current["compact"]),
        "clear": sanitize_context_rule(&clear_patch, &current["clear"]),
    });
    st.persist.lock_safe().set_kv("triggers_context", &next);
    {
        let mut cfg = st.config.lock_safe();
        // Mirror BOTH halves into typed config — the struct now carries the
        // upstream {compact,clear} shape so the sanitized blob parses 1:1.
        if let Ok(c) = serde_json::from_value::<crate::config::ContextRule>(next["compact"].clone())
        {
            cfg.context_trigger.compact = c;
        }
        if let Ok(c) = serde_json::from_value::<crate::config::ContextRule>(next["clear"].clone()) {
            cfg.context_trigger.clear = c;
        }
        st.config_store.save(&cfg).map_err(err_str)?;
    }
    crate::commands::emit_config_changed(&app);
    Ok(next)
    })
}

/// org:getTrigger — the org/clone-node blob (persist kv `org_trigger`;
/// upstream persisted-only too — no transport service exists yet).
#[tauri::command(async)]
pub fn org_get_trigger(app: AppHandle) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("org_get_trigger", || {
    let st = state(&app);
    let v = st
        .persist
        .lock_safe()
        .get_kv("org_trigger")
        .filter(|v| v.is_object())
        .unwrap_or_else(|| json!({ "apiKey": "", "enabled": false, "mode": "strict" }));
    Ok(v)
    })
}

/// org:setTrigger — persist-only merge; valid modes are the shared
/// TriggerMode set. Deliberately no side effects (upstream's comment
/// verbatim: nothing reads apiKey beyond the settings surface).
#[tauri::command(async)]
pub fn org_set_trigger(app: AppHandle, arg: Value) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("org_set_trigger", || {
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
    st.persist.lock_safe().set_kv("org_trigger", &next);
    Ok(next)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oa-cm-{tag}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::canonicalize(d).unwrap()
    }

    const PNG_1X1_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

    #[test]
    fn pasted_image_is_saved_with_data_url_prefix_and_rejects_non_images() {
        let dir = tmp("paste").join("pasted-images");
        let p = save_pasted_image(&dir, PNG_1X1_B64).unwrap();
        assert!(p.starts_with(&dir) && p.is_absolute());
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("paste-") && name.ends_with(".png"), "{name}");
        assert_eq!(&std::fs::read(&p).unwrap()[1..4], b"PNG");
        // data: URL prefix accepted; names are unique.
        let p2 = save_pasted_image(&dir, &format!("data:image/png;base64,{PNG_1X1_B64}")).unwrap();
        assert_ne!(p, p2);
        // Non-image bytes and bad base64 are refused and leave no file.
        let txt = base64::engine::general_purpose::STANDARD.encode(b"hello world, not an image");
        assert!(save_pasted_image(&dir, &txt).unwrap_err().contains("image"));
        assert!(save_pasted_image(&dir, "!!!not base64!!!").unwrap_err().contains("base64"));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
        // Oversize (> 20 MB decoded) is refused.
        let mut big = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        big.resize(MAX_PASTED_IMAGE_BYTES + 1, 0);
        let big_b64 = base64::engine::general_purpose::STANDARD.encode(&big);
        assert!(save_pasted_image(&dir, &big_b64).unwrap_err().contains("too large"));
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn reset_removes_only_this_hives_exposed_codex_data() {
        let codex = tmp("codexexp"); // stands in for ~/.codex (never the real one)
        let hive_root = codex.join("somewhere").join("hive");
        let id = codex_hive_id(&hive_root);
        let other = "0123456789ab";
        assert_ne!(id, other);
        for kind in ["sessions", "archived_sessions"] {
            let ns = codex.join(kind).join("agent-office");
            std::fs::create_dir_all(ns.join(&id).join("dev1")).unwrap();
            std::fs::write(ns.join(&id).join("dev1").join("rollout.jsonl"), "{}").unwrap();
            std::fs::create_dir_all(ns.join(other).join("dev9")).unwrap();
        }
        // the user's own sessions are untouched
        std::fs::create_dir_all(codex.join("sessions").join("2026")).unwrap();
        let gone = remove_exposed_codex_data(&codex, &hive_root);
        assert_eq!(gone.len(), 2);
        for kind in ["sessions", "archived_sessions"] {
            let ns = codex.join(kind).join("agent-office");
            assert!(!ns.join(&id).exists());
            assert!(ns.join(other).join("dev9").exists(), "another hive's data stays");
        }
        assert!(codex.join("sessions").join("2026").exists());
        // second pass: nothing left, empty namespace dirs are tidied
        std::fs::remove_dir_all(codex.join("sessions").join("agent-office").join(other)).unwrap();
        assert!(remove_exposed_codex_data(&codex, &hive_root).is_empty());
        assert!(!codex.join("sessions").join("agent-office").exists());
        let _ = std::fs::remove_dir_all(&codex);
    }

    #[test]
    fn pasted_images_prune_by_count_and_age() {
        let dir = tmp("paste-prune");
        for i in 0..5 {
            std::fs::write(dir.join(format!("paste-{i}.png")), b"x").unwrap();
            // distinct mtimes, ascending with i
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        std::fs::write(dir.join("keep-me.txt"), b"x").unwrap();
        prune_pasted_images(&dir, std::time::Duration::from_secs(3600), 3);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        left.sort();
        assert_eq!(left, vec!["keep-me.txt", "paste-2.png", "paste-3.png", "paste-4.png"]);
        // Zero max-age drops everything pasted, never foreign files.
        std::thread::sleep(std::time::Duration::from_millis(15));
        prune_pasted_images(&dir, std::time::Duration::ZERO, 100);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_root_must_be_inside_a_known_workspace() {
        let ws = tmp("ws");
        let other = tmp("other");
        std::fs::create_dir_all(ws.join("sub")).unwrap();
        let known = vec![ws.clone()];
        assert!(root_in_workspaces(&ws, &known));
        assert!(root_in_workspaces(&ws.join("sub"), &known));
        // A renderer-chosen root elsewhere, a parent, a traversal spelling,
        // and a nonexistent dir are all refused.
        assert!(!root_in_workspaces(&other, &known));
        assert!(!root_in_workspaces(ws.parent().unwrap(), &known));
        assert!(!root_in_workspaces(&ws.join("sub").join("..").join("..").join(other.file_name().unwrap()), &known));
        assert!(!root_in_workspaces(&ws.join("missing"), &known));
        assert!(!root_in_workspaces(&ws, &[]));
        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&other);
    }
}
