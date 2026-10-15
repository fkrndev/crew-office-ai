//! update:* — port of `src/main/updater.ts`, on the official signed updater.
//!
//! The check goes to the signed feed first (`plugins.updater.endpoints` in
//! tauri.conf.json -> the `latest.json` the release workflow uploads) through
//! `tauri-plugin-updater`. The update signature is MANDATORY and unrelated to
//! OS code-signing: the installer is verified against the minisign public key
//! baked into tauri.conf.json before anything is installed. When that feed is
//! unreachable or has no `latest.json` yet (no signed release exists), the
//! check FALLS BACK to the original notify-only GitHub `releases/latest` poll
//! (`available-manual` + release-page/download URL), so nothing regresses.
//!
//! States emitted on `update:status`: checking / not-available / available
//! (signed artifact -> Download & restart) / downloading (percent) /
//! downloaded / available-manual (notify-only, or a failed signed download) /
//! just-updated / error / idle.
//!
//! Nothing installs without the user's click: `update_download` only fetches
//! and verifies the bytes (state `downloaded`); `update_restart_and_install`
//! installs them. With live agents it raises the existing quit warning instead
//! and installs when the app actually exits (`install_staged_on_exit`).
//! Windows: the plugin's installer (NSIS/MSI, `passive` mode) takes over and
//! exits the process; macOS/Linux: install, then `AppHandle::restart`. The
//! macOS/Linux install paths are unverified on this Windows-only dev machine.
//!
//! Background behaviour (upstream initAutoUpdater): `start_background_checks`
//! runs a first check 30s after boot and then every 6h, gated on config
//! `autoUpdate` (default ON) - it only ever CHECKS; a `last-run-version` stamp
//! makes the first launch of a new version emit `just-updated` with THAT
//! release's notes; everything notable is appended to `<harness>/updater.log`
//! (the file analytics.rs reads to tell a manual update from an auto one).

use crate::lock_ext::LockExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_updater::{Update, UpdaterBuilder, UpdaterExt};

/// Releases feed this build checks — the app's own repo.
const REPO: &str = "fkrndev/crew-office-ai";
const UA: &str = "agent-office-updater";

#[derive(Default)]
pub struct UpdateState(pub std::sync::Mutex<Value>);

/// First check shortly after boot (don't compete with spawn/startup I/O), then
/// every CHECK_INTERVAL.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(30);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// Hard cap on one version check: the feed answer is a few KB, so anything past
/// this is hung (captive portal, half-open socket after sleep), not slow. A
/// check must always reach a terminal state instead of spinning on `checking`.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY_BYTES: u64 = 256 * 1024;

fn updater_log_path(app: &AppHandle) -> PathBuf {
    crate::misc::harness_home(app).join("updater.log")
}

/// Append one `[iso] message` line to updater.log. Logging must never take the
/// app down — every failure is swallowed.
fn append_log_line(path: &Path, msg: &str) {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let line = format!(
        "[{}] {msg}\n",
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    );
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

fn log_line(app: &AppHandle, msg: &str) {
    log::info!("[updater] {msg}");
    append_log_line(&updater_log_path(app), msg);
}

/// Emit a status. The event always goes out (the renderer's reducer decides);
/// the REMEMBERED status (`update:current`) only advances per `state_replaces`,
/// so a re-check can never wipe a staged download out of a fresh window.
fn emit(app: &AppHandle, status: Value) {
    if let Some(st) = app.try_state::<UpdateState>() {
        let mut cur = st.0.lock_safe();
        if state_replaces(&cur, &status) {
            *cur = status.clone();
        }
    }
    let _ = app.emit("update:status", status);
}

/// Emit a status that must win over whatever is remembered (a failed
/// download/install replaces `downloading`).
fn emit_force(app: &AppHandle, status: Value) {
    if let Some(st) = app.try_state::<UpdateState>() {
        *st.0.lock_safe() = status.clone();
    }
    let _ = app.emit("update:status", status);
}

fn status_rank(state: &str) -> u8 {
    match state {
        "idle" => 0,
        "checking" | "not-available" | "error" | "just-updated" => 1,
        "available-manual" => 2,
        "available" => 3,
        "downloading" => 4,
        "downloaded" => 5,
        _ => 0,
    }
}

/// Port of shared/updateState.reduceStatus: does `next` replace `prev`?
/// A later pipeline stage is never replaced by an earlier one for the SAME
/// version; a different version always wins. One addition mirrored in the
/// renderer reducer: a failed download (`error` / `available-manual` /
/// `available`) replaces `downloading`, otherwise the UI would sit on
/// "downloading 100%" forever.
pub(crate) fn state_replaces(prev: &Value, next: &Value) -> bool {
    let ps = prev.get("state").and_then(|v| v.as_str()).unwrap_or("idle");
    let ns = next.get("state").and_then(|v| v.as_str()).unwrap_or("idle");
    let pv = prev.get("version").and_then(|v| v.as_str());
    let nv = next.get("version").and_then(|v| v.as_str());
    if let (Some(p), Some(n)) = (pv, nv) {
        if p != n {
            return true; // newer, or a different (rolled-back) release
        }
    }
    if ps == "downloading" && matches!(ns, "error" | "available-manual" | "available") {
        return true;
    }
    status_rank(ns) >= status_rank(ps)
}

/// A signed, installable update was found.
fn signed_available_status(version: &str, notes: Option<&str>) -> Value {
    json!({
        "state": "available",
        "version": version,
        "notes": notes,
        "url": release_tag_url(version),
        "signed": true,
    })
}

fn release_tag_url(version: &str) -> String {
    format!("https://github.com/{REPO}/releases/tag/v{}", version.trim_start_matches('v'))
}

/// A signed download/install failed: degrade to the manual path (the reason is
/// shown verbatim) instead of leaving the UI mid-flight.
fn failed_status(version: &str, notes: Option<&str>, reason: &str) -> Value {
    json!({
        "state": "available-manual",
        "version": version,
        "notes": notes,
        "url": release_tag_url(version),
        "reason": reason,
    })
}

/// Download progress as a whole percent; None when the total is unknown.
fn percent(downloaded: u64, total: Option<u64>) -> Option<u8> {
    let t = total.filter(|t| *t > 0)?;
    Some((downloaded.saturating_mul(100) / t).min(100) as u8)
}

/// The staged update (found by the last signed check) and its downloaded,
/// signature-verified bytes. Module-global so lib.rs needs no extra managed
/// state.
struct Staged {
    update: Update,
    bytes: Option<std::sync::Arc<Vec<u8>>>,
    downloading: bool,
    /// The user clicked "restart" while agents were running: the quit warning
    /// is up and the install happens when the app actually exits.
    install_on_exit: bool,
}

static STAGED: std::sync::Mutex<Option<Staged>> = std::sync::Mutex::new(None);

fn staged() -> std::sync::MutexGuard<'static, Option<Staged>> {
    STAGED.lock_safe()
}

/// A re-check that finds the version we already hold (with its bytes) keeps the
/// held copy instead of discarding a finished download.
fn keep_existing_stage(existing: Option<(&str, bool)>, new_version: &str) -> bool {
    matches!(existing, Some((v, true)) if v == new_version)
}

fn download_in_flight() -> bool {
    staged().as_ref().is_some_and(|s| s.downloading)
}

/// shared/updateState.isNewer — numeric-triple compare, `v` prefix tolerated.
fn is_newer(tag: &str, current: &str) -> bool {
    fn triple(s: &str) -> Vec<u64> {
        s.trim_start_matches('v')
            .split('.')
            .map(|p| {
                p.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    }
    let (a, b) = (triple(tag), triple(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

/// pickDownloadAsset — the electron-builder asset name for THIS platform.
/// Tauri bundles differ, but the convention is kept: win → -win-x64-setup.exe
/// (else msi/nsis matches), linux → AppImage, mac → -mac-<arch>.dmg.
fn pick_download_asset(assets: &Value) -> Option<String> {
    let arr = assets.as_array()?;
    let want: &str = if cfg!(target_os = "windows") {
        "setup.exe"
    } else if cfg!(target_os = "macos") {
        ".dmg"
    } else {
        "AppImage"
    };
    arr.iter()
        .filter_map(|a| {
            let name = a.get("name")?.as_str()?;
            let url = a.get("browser_download_url")?.as_str()?;
            name.ends_with(want).then(|| url.to_string())
        })
        .next()
}

/// One bounded GET of a GitHub API document (hard overall timeout, https only,
/// body capped). Never hangs.
fn gh_api_get(url: &str, timeout: Duration) -> Result<Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .https_only(true)
        .build()
        .into();
    let mut resp = agent
        .get(url)
        .header("User-Agent", UA)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    resp.body_mut()
        .with_config()
        .limit(MAX_BODY_BYTES)
        .read_json::<Value>()
        .map_err(|e| e.to_string())
}

fn fetch_latest() -> Result<Value, String> {
    gh_api_get(
        &format!("https://api.github.com/repos/{REPO}/releases/latest"),
        CHECK_TIMEOUT,
    )
}

/// Body of the release tagged v{version}, or None. Never throws.
fn fetch_release_body(version: &str) -> Option<String> {
    gh_api_get(
        &format!("https://api.github.com/repos/{REPO}/releases/tags/v{version}"),
        Duration::from_secs(10),
    )
    .ok()?
    .get("body")
    .and_then(|b| b.as_str())
    .map(String::from)
}

/// The notify-only status for a GitHub `releases/latest` document, or None when
/// it is not newer than `current`. `reason` says why the signed path was not
/// used (shown in the UI next to "this install can't update itself").
fn manual_status_from_release(rel: &Value, current: &str, reason: Option<&str>) -> Option<Value> {
    let tag = rel
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if !is_newer(tag, current) {
        return None;
    }
    let mut st = json!({
        "state": "available-manual",
        "version": tag.trim_start_matches('v'),
        "url": rel.get("html_url").and_then(|v| v.as_str())
            .unwrap_or(&format!("https://github.com/{REPO}/releases/latest")),
        "downloadUrl": pick_download_asset(&rel["assets"]),
        "notes": rel.get("body").and_then(|v| v.as_str()),
    });
    if let Some(r) = reason {
        st["reason"] = json!(r);
    }
    Some(st)
}

/// Why the signed path was skipped, as a short user-facing phrase.
const NO_SIGNED_FEED: &str = "no signed update feed for this release yet";

/// Ask the signed feed (the plugin's `check`). `endpoint` overrides the
/// configured endpoints (tests point it at a local stub); `configure` tweaks
/// the builder (tests disable the system proxy). Generic over the runtime so
/// the mock runtime can drive it.
pub(crate) async fn check_signed<R: Runtime>(
    app: &AppHandle<R>,
    endpoint: Option<tauri::Url>,
    configure: impl FnOnce(UpdaterBuilder) -> UpdaterBuilder,
) -> Result<Option<Update>, String> {
    let mut builder = app.updater_builder().timeout(CHECK_TIMEOUT);
    if let Some(url) = endpoint {
        builder = builder.endpoints(vec![url]).map_err(|e| e.to_string())?;
    }
    let updater = configure(builder).build().map_err(|e| e.to_string())?;
    updater.check().await.map_err(|e| e.to_string())
}

/// Download + verify the signed artifact. The plugin checks the minisign
/// signature against the configured public key BEFORE returning the bytes, so
/// a tampered or mis-signed file is an `Err` here and is never installable.
pub(crate) async fn download_signed(
    update: &Update,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<Vec<u8>, String> {
    let mut got: u64 = 0;
    update
        .download(
            |chunk, total| {
                got += chunk as u64;
                on_progress(got, total);
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())
}

/// Stage a found update (keeping an already-downloaded copy of the same
/// version) and return the status to emit.
fn stage_update(update: Update) -> Value {
    let version = update.version.clone();
    let notes = update.body.clone();
    let mut g = staged();
    let existing = g
        .as_ref()
        .map(|s| (s.update.version.as_str(), s.bytes.is_some()));
    if keep_existing_stage(existing, &version) {
        return json!({ "state": "downloaded", "version": version, "notes": notes });
    }
    *g = Some(Staged {
        update,
        bytes: None,
        downloading: false,
        install_on_exit: false,
    });
    signed_available_status(&version, notes.as_deref())
}

/// The shared check: emit `checking`, ask the signed feed, and if that feed is
/// unavailable fall back to the notify-only releases/latest poll.
async fn run_check(app: &AppHandle) {
    // A download is mid-flight: a re-check must not disturb it.
    if download_in_flight() {
        return;
    }
    emit(app, json!({ "state": "checking" }));
    let current = env!("CARGO_PKG_VERSION");
    match check_signed(app, None, |b| b).await {
        Ok(Some(update)) => {
            let status = stage_update(update);
            emit(app, status);
            return;
        }
        Ok(None) => {
            emit(app, json!({ "state": "not-available" }));
            return;
        }
        Err(e) => log_line(
            app,
            &format!("signed update feed unavailable ({e}); falling back to the GitHub releases check"),
        ),
    }
    // Fallback: the original notify-only path (blocking ureq off the runtime).
    let fetched = tauri::async_runtime::spawn_blocking(fetch_latest)
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
    match fallback_status(fetched, current) {
        Ok(st) => emit(app, st),
        Err(e) => {
            log_line(app, &format!("update check failed: {e}"));
            emit(app, json!({ "state": "error", "message": e }));
        }
    }
}

/// The status the notify-only fallback reports for the `releases/latest` fetch.
/// A 404 there means the repo has no release yet - nothing to update to, not a
/// failure - so it maps to `not-available` like an up-to-date install.
fn fallback_status(fetched: Result<Value, String>, current: &str) -> Result<Value, String> {
    match fetched {
        Ok(rel) => Ok(manual_status_from_release(&rel, current, Some(NO_SIGNED_FEED))
            .unwrap_or_else(|| json!({ "state": "not-available" }))),
        // ureq renders StatusCode(404) as "http status: 404".
        Err(e) if e.trim() == "http status: 404" => Ok(json!({ "state": "not-available" })),
        Err(e) => Err(e),
    }
}

/// shared/updateState.shouldShowReleaseDrop — should the "what's new" page
/// open on this launch? Same version -> no; never run before -> yes; moved
/// forward -> yes; moved BACKWARD -> no (a downgrade announces nothing). The
/// forward test is "not a downgrade", so a second RC of the same version
/// (isNewer ignores -rc.N) still shows its notes.
pub fn should_show_release_drop(previous: Option<&str>, current: &str) -> bool {
    match previous {
        Some(p) if p == current => false,
        None => true,
        Some(p) => !is_newer(p, current),
    }
}

/// The `last-run-version` stamp decision, on an injectable stamp file. Reads
/// the previous stamp, writes the current one when it differs, and returns
/// `Some(previous)` iff the "just updated" drop should fire - gated on the
/// stamp having LANDED (an unwritable dir would otherwise reopen the drop on
/// every single boot forever).
pub fn stamp_last_run_version(stamp_file: &Path, current: &str) -> Option<Option<String>> {
    let previous = std::fs::read_to_string(stamp_file)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if previous.as_deref() == Some(current) {
        return None;
    }
    if let Some(dir) = stamp_file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::write(stamp_file, format!("{current}\n")).is_err() {
        return None;
    }
    should_show_release_drop(previous.as_deref(), current).then_some(previous)
}

/// Background updater wiring - call once from setup. First launch of a new
/// version emits `just-updated` with that release's notes; then (release
/// builds only, gated on config `autoUpdate`, default ON) a check 30s after
/// boot and every 6h. Everything runs off the setup thread.
pub fn start_background_checks(app: &AppHandle) {
    // analytics.rs reads this marker to disprove an "auto" attribution when a
    // later launch names a different version (LOG_READY_PREFIX).
    log_line(
        app,
        &format!("native updater ready (current v{})", env!("CARGO_PKG_VERSION")),
    );
    // Post-update page: the stamp is the updater's own (analytics keeps a
    // separate one that only exists when telemetry initialised). The release
    // fetch failing just means no notes, never a broken boot.
    {
        let app2 = app.clone();
        std::thread::spawn(move || {
            let stamp = crate::misc::harness_home(&app2).join("last-run-version");
            let current = env!("CARGO_PKG_VERSION");
            if let Some(previous) = stamp_last_run_version(&stamp, current) {
                log_line(
                    &app2,
                    &format!(
                        "first run of {current} (previous {}); fetching its release page",
                        previous.as_deref().unwrap_or("none")
                    ),
                );
                let notes = fetch_release_body(current);
                emit(
                    &app2,
                    json!({ "state": "just-updated", "version": current, "notes": notes }),
                );
                log_line(
                    &app2,
                    &format!(
                        "just-updated {current} {} release notes",
                        if notes.is_some() { "with" } else { "without" }
                    ),
                );
            }
        });
    }
    // Dev runs never poll (upstream: behind app.isPackaged).
    if cfg!(debug_assertions) {
        return;
    }
    let app2 = app.clone();
    crate::panic_guard::supervise_arg("updater-background-checks", app2, |app2| async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            if auto_update_enabled(&app2) {
                run_check(&app2).await;
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

/// config.autoUpdate - default ON; only an explicit `false` disables.
fn auto_update_enabled(app: &AppHandle) -> bool {
    app.try_state::<crate::commands::AppState>()
        .map(|st| st.config_store.raw_value())
        .and_then(|c| c.get("autoUpdate").and_then(|v| v.as_bool()))
        != Some(false)
}

/// update:checkNow — signed-feed check (notify-only fallback); emits
/// update-state transitions.
#[tauri::command(async)]
pub fn update_check_now(app: AppHandle) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("update_check_now", || {
    emit(&app, json!({ "state": "checking" }));
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move { run_check(&app2).await });
    Ok(json!({ "ok": true }))
    })
}

/// update:current — the last emitted status (or idle).
#[tauri::command(async)]
pub fn update_current(app: AppHandle) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("update_current", || {
    Ok(app
        .try_state::<UpdateState>()
        .map(|s| s.0.lock_safe().clone())
        .unwrap_or_else(|| json!({ "state": "idle" })))
    })
}

/// update:openRelease — open a releases URL; guarded to this repo's release
/// pages (upstream's prefix guard — download URLs live under /download/).
#[tauri::command(async)]
pub fn update_open_release(app: AppHandle, url: String) -> Result<bool, String> {
    crate::panic_guard::cmd_guard("update_open_release", || {
    let ok = url.starts_with(&format!("https://github.com/{REPO}/releases"));
    if !ok {
        return Err("url is not a release page".into());
    }
    // An asset URL means the badge's download click, not the notes link. It is
    // the only positive trace the manual path leaves (analytics reads
    // updater.log to tell a manual update from an auto one).
    if let Some(rest) = url.split("/releases/download/v").nth(1) {
        let ver = rest.split('/').next().unwrap_or_default();
        if ver.starts_with(|c: char| c.is_ascii_digit()) {
            log_line(&app, &format!("manual download opened: {ver}"));
        }
    }
    crate::cmd_misc::app_open_external(app, url)?;
    Ok(true)
    })
}

/// update:download — fetch + signature-verify the staged signed update,
/// emitting `downloading` (percent) then `downloaded` on `update:status`. It
/// never installs. Without a staged signed update (the notify-only fallback)
/// the honest answer is "manual": the caller opens downloadUrl instead.
#[tauri::command(async)]
pub fn update_download(app: AppHandle) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("update_download", || {
    enum Next {
        Manual,
        InFlight,
        Done(Value),
        Fetch(Update),
    }
    let next = {
        let mut g = staged();
        match g.as_mut() {
            None => Next::Manual,
            Some(s) if s.downloading => Next::InFlight,
            Some(s) if s.bytes.is_some() => Next::Done(json!({
                "state": "downloaded",
                "version": s.update.version,
                "notes": s.update.body,
            })),
            Some(s) => {
                s.downloading = true;
                Next::Fetch(s.update.clone())
            }
        }
    };
    let update = match next {
        Next::Manual => {
            return Ok(json!({ "ok": false, "error": "manual-download", "state": "available-manual" }))
        }
        Next::InFlight => return Ok(json!({ "ok": true, "state": "downloading" })),
        Next::Done(st) => {
            emit(&app, st);
            return Ok(json!({ "ok": true, "state": "downloaded" }));
        }
        Next::Fetch(u) => u,
    };
    let version = update.version.clone();
    emit_force(
        &app,
        json!({ "state": "downloading", "version": version, "percent": 0, "notes": update.body }),
    );
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move { run_download(&app2, update).await });
    Ok(json!({ "ok": true, "state": "downloading" }))
    })
}

async fn run_download(app: &AppHandle, update: Update) {
    let version = update.version.clone();
    let notes = update.body.clone();
    let mut last: Option<u8> = None;
    let res = download_signed(&update, |got, total| {
        let p = percent(got, total);
        if p.is_some() && p != last {
            last = p;
            emit(
                app,
                json!({ "state": "downloading", "version": version, "percent": p, "notes": notes }),
            );
        }
    })
    .await;
    match res {
        Ok(bytes) => {
            {
                let mut g = staged();
                match g.as_mut() {
                    // Still the update we fetched (a newer check may have replaced it).
                    Some(s) if s.update.version == version => {
                        s.bytes = Some(std::sync::Arc::new(bytes));
                        s.downloading = false;
                    }
                    _ => return,
                }
            }
            log_line(
                app,
                &format!("update downloaded: {version} — waiting for the user to restart"),
            );
            emit_force(
                app,
                json!({ "state": "downloaded", "version": version, "notes": notes }),
            );
        }
        Err(e) => {
            if let Some(s) = staged().as_mut() {
                s.downloading = false;
            }
            log_line(app, &format!("update download failed: {e}"));
            emit_force(app, failed_status(&version, notes.as_deref(), &e));
        }
    }
}

/// Live agent PTYs (0 when state is not up yet).
fn live_pty_count(app: &AppHandle) -> usize {
    app.try_state::<crate::commands::AppState>()
        .and_then(|st| Some(st.ptys.lock_safe().len()))
        .unwrap_or(0)
}

/// Install the staged, downloaded update. Windows: the plugin launches the
/// NSIS/MSI installer (passive) and EXITS this process, the installer relaunches
/// the app. macOS/Linux: replace the install in place, then restart (unix
/// behaviour is unverified on the Windows-only dev machine). Returns only on
/// failure (or when nothing is staged).
fn install_staged(app: &AppHandle) -> Result<(), String> {
    let (update, bytes) = {
        let g = staged();
        let s = g.as_ref().ok_or("no staged update")?;
        let bytes = s.bytes.clone().ok_or("the update has not been downloaded")?;
        (s.update.clone(), bytes)
    };
    update.install(bytes.as_slice()).map_err(|e| e.to_string())?;
    app.restart()
}

/// update:restartAndInstall — the user's click. Never installs anything that
/// was not downloaded and signature-verified. With live agents it raises the
/// existing quit warning and defers the install to the actual exit
/// (`install_staged_on_exit`), so a click can never silently kill running work.
#[tauri::command(async)]
pub fn update_restart_and_install(app: AppHandle) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("update_restart_and_install", || {
    let version = match staged().as_ref() {
        Some(s) if s.bytes.is_some() => s.update.version.clone(),
        _ => {
            return Ok(json!({ "ok": false, "error": "manual-download", "state": "available-manual" }))
        }
    };
    log_line(&app, "quitAndInstall requested by the user");
    let live = live_pty_count(&app);
    if live > 0 {
        if let Some(s) = staged().as_mut() {
            s.install_on_exit = true;
        }
        let _ = app.emit("app:closeRequested", json!({ "ptyCount": live }));
        return Ok(json!({ "ok": true, "deferred": true, "ptyCount": live }));
    }
    match install_staged(&app) {
        Ok(()) => Ok(json!({ "ok": true })),
        Err(e) => {
            log_line(&app, &format!("quitAndInstall failed: {e}"));
            let notes = staged().as_ref().and_then(|s| s.update.body.clone());
            emit_force(&app, failed_status(&version, notes.as_deref(), &e));
            Ok(json!({ "ok": false, "error": e }))
        }
    }
    })
}

/// RunEvent::Exit hook (lib.rs, after the agent PTYs are killed): when the
/// user asked to restart-and-install while agents were running and then
/// confirmed the quit, perform the install now.
pub fn install_staged_on_exit(app: &AppHandle) {
    let armed = staged().as_ref().is_some_and(|s| s.install_on_exit);
    if !armed {
        return;
    }
    if let Err(e) = install_staged(app) {
        log_line(app, &format!("quitAndInstall failed: {e}"));
    }
}

/// The user dismissed the quit warning (app:cancelClose): drop a pending
/// install-on-exit so a later, unrelated quit does not install.
pub fn note_quit_cancelled(app: &AppHandle) {
    let was = staged().as_mut().map(|s| std::mem::replace(&mut s.install_on_exit, false));
    if was == Some(true) {
        log_line(app, "quitAndInstall cancelled by the user at the quit warning");
    }
}

/// update:simulate — DEV ONLY (upstream's debug surface).
#[tauri::command(async)]
pub fn update_simulate(
    app: AppHandle,
    version: Option<String>,
    notes: Option<String>,
) -> Result<bool, String> {
    crate::panic_guard::cmd_guard("update_simulate", || {
    // Registered in release builds too — but a spoofed "update available"
    // banner is a dev-only affordance.
    if !cfg!(debug_assertions) {
        return Err("update:simulate is only available in dev builds".into());
    }
    let v = version.unwrap_or_else(|| "9.9.9".into());
    emit(
        &app,
        json!({
            "state": "available-manual",
            "version": v,
            "notes": notes.unwrap_or_else(|| "simulated release".into()),
            "url": format!("https://github.com/{REPO}/releases/tag/v{v}"),
        }),
    );
    Ok(true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_maps_a_404_to_not_available_but_keeps_real_errors() {
        let na = json!({ "state": "not-available" });
        // repo with no release yet
        assert_eq!(fallback_status(Err("http status: 404".into()), "0.4.6"), Ok(na.clone()));
        // an up-to-date install
        assert_eq!(fallback_status(Ok(json!({ "tag_name": "v0.4.6" })), "0.4.6"), Ok(na));
        // a newer release is still offered
        let st = fallback_status(Ok(json!({ "tag_name": "v9.0.0", "assets": [] })), "0.4.6").unwrap();
        assert_eq!(st["state"], "available-manual");
        // everything else stays an error
        assert_eq!(
            fallback_status(Err("http status: 500".into()), "0.4.6"),
            Err("http status: 500".to_string())
        );
    }

    #[test]
    fn release_drop_rules() {
        assert!(!should_show_release_drop(Some("0.4.6"), "0.4.6"));
        assert!(should_show_release_drop(None, "0.4.6"));
        assert!(should_show_release_drop(Some("0.4.5"), "0.4.6"));
        // a downgrade announces nothing
        assert!(!should_show_release_drop(Some("0.4.7"), "0.4.6"));
        // a second RC of the same x.y.z is "not a downgrade" -> shown
        assert!(should_show_release_drop(Some("0.4.7-rc.1"), "0.4.7-rc.2"));
    }

    #[test]
    fn last_run_version_stamp_fires_once_per_version() {
        let d = std::env::temp_dir().join(format!("oa-upd-{}", uuid::Uuid::new_v4()));
        let stamp = d.join("last-run-version");
        // fresh install: shows the page and stamps
        assert_eq!(stamp_last_run_version(&stamp, "0.4.6"), Some(None));
        assert_eq!(std::fs::read_to_string(&stamp).unwrap().trim(), "0.4.6");
        // same version relaunch: nothing
        assert_eq!(stamp_last_run_version(&stamp, "0.4.6"), None);
        // upgrade: fires with the previous version, re-stamps
        assert_eq!(
            stamp_last_run_version(&stamp, "0.4.7"),
            Some(Some("0.4.6".to_string()))
        );
        // downgrade: stamps but announces nothing
        assert_eq!(stamp_last_run_version(&stamp, "0.4.5"), None);
        assert_eq!(std::fs::read_to_string(&stamp).unwrap().trim(), "0.4.5");
        // an unwritable stamp (a directory where the file should be) never fires
        let blocked = d.join("blocked");
        std::fs::create_dir_all(blocked.join("last-run-version")).unwrap();
        assert_eq!(stamp_last_run_version(&blocked.join("last-run-version"), "0.4.6"), None);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn updater_log_lines_are_appended_with_a_timestamp() {
        let d = std::env::temp_dir().join(format!("oa-updlog-{}", uuid::Uuid::new_v4()));
        let log = d.join("updater.log");
        append_log_line(&log, "manual download opened: 0.4.7");
        append_log_line(&log, "just-updated 0.4.7 with release notes");
        let text = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with('[') && lines[0].ends_with("manual download opened: 0.4.7"));
        // analytics.rs reads this file back
        assert!(crate::analytics::read_updater_log_tail(&log).unwrap().contains("just-updated"));
        std::fs::remove_dir_all(&d).ok();
    }

    // ---- signed updater: state machine + endpoint config ----

    fn conf() -> Value {
        serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json parses")
    }

    #[test]
    fn updater_config_is_wired_to_this_repos_signed_feed() {
        use base64::Engine;
        let c = conf();
        let up = &c["plugins"]["updater"];
        // Endpoint: https, this repo, the latest release's latest.json.
        let ep = up["endpoints"][0].as_str().expect("an endpoint");
        let url = tauri::Url::parse(ep).expect("endpoint parses as a URL");
        assert_eq!(url.scheme(), "https");
        assert_eq!(
            ep,
            format!("https://github.com/{REPO}/releases/latest/download/latest.json")
        );
        // Public key: the CONTENT (base64 of a minisign .pub), never a path.
        let pk = up["pubkey"].as_str().expect("a pubkey");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(pk)
            .expect("pubkey is base64");
        let text = String::from_utf8(decoded).unwrap();
        assert!(text.starts_with("untrusted comment: minisign public key"), "{text}");
        assert_eq!(up["windows"]["installMode"], "passive");
        assert_eq!(c["bundle"]["createUpdaterArtifacts"], true);
        // No private key material may ever sit in the config.
        assert!(!c.to_string().contains("secret key"));
    }

    #[test]
    fn state_machine_never_regresses_a_staged_download() {
        let dl = json!({ "state": "downloaded", "version": "1.2.0" });
        // re-check noise loses to a staged download of the same version
        assert!(!state_replaces(&dl, &json!({ "state": "checking" })));
        assert!(!state_replaces(&dl, &json!({ "state": "not-available" })));
        assert!(!state_replaces(&dl, &json!({ "state": "error", "message": "x" })));
        assert!(!state_replaces(&dl, &json!({ "state": "available", "version": "1.2.0" })));
        // a different (newer) release always wins
        assert!(state_replaces(&dl, &json!({ "state": "available", "version": "1.3.0" })));
        // forward progress
        let av = json!({ "state": "available", "version": "1.2.0" });
        assert!(state_replaces(&av, &json!({ "state": "downloading", "version": "1.2.0", "percent": 1 })));
        assert!(state_replaces(&json!({ "state": "downloading", "version": "1.2.0" }), &dl));
        // fresh start accepts anything
        assert!(state_replaces(&json!({ "state": "idle" }), &json!({ "state": "checking" })));
        // a failed download replaces `downloading` (else the UI sticks at 100%)
        let busy = json!({ "state": "downloading", "version": "1.2.0", "percent": 100 });
        assert!(state_replaces(&busy, &failed_status("1.2.0", None, "boom")));
        assert!(state_replaces(&busy, &json!({ "state": "error", "message": "boom" })));
    }

    #[test]
    fn status_builders_and_progress_math() {
        let a = signed_available_status("1.2.0", Some("notes"));
        assert_eq!(a["state"], "available");
        assert_eq!(a["version"], "1.2.0");
        assert_eq!(a["url"], format!("https://github.com/{REPO}/releases/tag/v1.2.0"));
        assert_eq!(release_tag_url("v1.2.0"), release_tag_url("1.2.0"));
        let f = failed_status("1.2.0", None, "bad signature");
        assert_eq!(f["state"], "available-manual");
        assert_eq!(f["reason"], "bad signature");
        assert!(f["url"].as_str().unwrap().starts_with("https://github.com/"));

        assert_eq!(percent(0, Some(200)), Some(0));
        assert_eq!(percent(50, Some(200)), Some(25));
        assert_eq!(percent(999, Some(200)), Some(100)); // clamped
        assert_eq!(percent(10, None), None);
        assert_eq!(percent(10, Some(0)), None);

        assert!(keep_existing_stage(Some(("1.2.0", true)), "1.2.0"));
        assert!(!keep_existing_stage(Some(("1.2.0", false)), "1.2.0"));
        assert!(!keep_existing_stage(Some(("1.1.0", true)), "1.2.0"));
        assert!(!keep_existing_stage(None, "1.2.0"));
    }

    #[test]
    fn notify_only_fallback_status_from_a_github_release() {
        let rel = json!({
            "tag_name": "v9.9.9",
            "html_url": "https://github.com/fkrndev/crew-office-ai/releases/tag/v9.9.9",
            "body": "what's new",
            "assets": [
                { "name": "Agent.Office_9.9.9_x64-setup.exe", "browser_download_url": "https://github.com/fkrndev/crew-office-ai/releases/download/v9.9.9/Agent.Office_9.9.9_x64-setup.exe" },
                { "name": "Agent.Office_9.9.9_aarch64.dmg", "browser_download_url": "https://github.com/fkrndev/crew-office-ai/releases/download/v9.9.9/x.dmg" },
                { "name": "Agent.Office_9.9.9_amd64.AppImage", "browser_download_url": "https://github.com/fkrndev/crew-office-ai/releases/download/v9.9.9/x.AppImage" }
            ]
        });
        let st = manual_status_from_release(&rel, "0.1.0", Some(NO_SIGNED_FEED)).unwrap();
        assert_eq!(st["state"], "available-manual");
        assert_eq!(st["version"], "9.9.9");
        assert_eq!(st["reason"], NO_SIGNED_FEED);
        assert_eq!(st["notes"], "what's new");
        assert!(st["downloadUrl"].as_str().unwrap().starts_with("https://github.com/"));
        // not newer -> nothing to announce
        assert!(manual_status_from_release(&rel, "9.9.9", None).is_none());
        assert!(manual_status_from_release(&json!({}), "0.1.0", None).is_none());
    }

    // ---- signed updater against a local stub feed (no real network) ----

    /// base64 of the minisign signature the app's REAL updater key produced for
    /// the 6 bytes `hello\n` (a signature is public; it proves nothing about
    /// the private key). Anything other than those exact bytes must fail.
    const HELLO_SIG: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVSSEdDNHVFNEdmaUE1T2ljZFpPbjBUc3dUOVhhMVhhc21yRTdzQUlSZnYvcDc5OTBqdXhoamp3ODRXditEa04yRFFaUGN3aEpLMTdQZTdWK3F0cXBkQzJXTU40NDJsaFFVPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkyMDQyMzQzCWZpbGU6YS5iaW4KbVRxWGtXV3NBUEJVSUl1VjRIbXp5N2ZlTDJDaElua3JtMlFaWEFGcmJuVUJZNkdKUVpmS2s4RmlkMGh5Umo0WS9URzhWTjg3bE92cG90UDl0bFp1QVE9PQo=";

    struct Stub {
        addr: String,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        th: Option<std::thread::JoinHandle<()>>,
    }

    impl Stub {
        fn url(&self, path: &str) -> tauri::Url {
            tauri::Url::parse(&format!("http://{}{path}", self.addr)).unwrap()
        }
    }

    impl Drop for Stub {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(t) = self.th.take() {
                let _ = t.join();
            }
        }
    }

    fn start_stub() -> Stub {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let server = tiny_http::Server::http("127.0.0.1:0").expect("bind stub");
        let addr = format!("127.0.0.1:{}", server.server_addr().to_ip().unwrap().port());
        let stop = Arc::new(AtomicBool::new(false));
        let (stop2, addr2) = (stop.clone(), addr.clone());
        let th = std::thread::spawn(move || {
            while !stop2.load(Ordering::SeqCst) {
                let Ok(Some(req)) = server.recv_timeout(Duration::from_millis(50)) else {
                    continue;
                };
                let manifest = |version: &str, sig: &str, artifact: &str| {
                    json!({
                        "version": version,
                        "notes": "stub notes",
                        "pub_date": "2026-01-01T00:00:00Z",
                        "platforms": { "agent-office-test": {
                            "signature": sig,
                            "url": format!("http://{addr2}{artifact}"),
                        }}
                    })
                    .to_string()
                };
                let path = req.url().to_string();
                let (code, body) = match path.as_str() {
                    "/latest-good.json" => (200, manifest("99.0.0", HELLO_SIG, "/good.bin")),
                    "/latest-tampered.json" => (200, manifest("99.0.0", HELLO_SIG, "/tampered.bin")),
                    "/latest-bogus-sig.json" => (200, manifest("99.0.0", "Ym9ndXMgc2lnbmF0dXJl", "/good.bin")),
                    "/latest-older.json" => (200, manifest("0.0.1", HELLO_SIG, "/good.bin")),
                    "/good.bin" => (200, "hello\n".to_string()),
                    "/tampered.bin" => (200, "malicious installer bytes".to_string()),
                    _ => (404, "not found".to_string()),
                };
                let _ = req.respond(tiny_http::Response::from_string(body).with_status_code(code));
            }
        });
        Stub { addr, stop, th: Some(th) }
    }

    fn mock_updater_app() -> tauri::App<tauri::test::MockRuntime> {
        let mut ctx = tauri::test::mock_context(tauri::test::noop_assets());
        // The app's REAL public key and install mode from tauri.conf.json, so
        // verification is exercised against the shipped key.
        let mut cfg = conf()["plugins"]["updater"].clone();
        cfg["endpoints"] = json!([]);
        cfg["dangerousInsecureTransportProtocol"] = json!(true);
        ctx.config_mut().plugins.0.insert("updater".into(), cfg);
        tauri::test::mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().target("agent-office-test").build())
            .build(ctx)
            .expect("mock app with the updater plugin")
    }

    fn check(app: &tauri::App<tauri::test::MockRuntime>, stub: &Stub, path: &str) -> Result<Option<Update>, String> {
        tauri::async_runtime::block_on(check_signed(app.handle(), Some(stub.url(path)), |b| {
            b.no_proxy() // never route the loopback stub through a system proxy
        }))
    }

    #[test]
    fn signed_check_and_download_against_a_local_stub_feed() {
        let stub = start_stub();
        let app = mock_updater_app();

        // 1. a newer version is reported, with its notes and download target
        let update = check(&app, &stub, "/latest-good.json")
            .expect("check ok")
            .expect("an update is available");
        assert_eq!(update.version, "99.0.0");
        assert_eq!(update.body.as_deref(), Some("stub notes"));

        // 2. positive control: the correctly signed artifact downloads, with progress
        let mut progress = Vec::new();
        let bytes = tauri::async_runtime::block_on(download_signed(&update, |got, total| {
            progress.push((got, total));
        }))
        .expect("a correctly signed artifact verifies");
        assert_eq!(bytes, b"hello\n");
        assert_eq!(progress.last().map(|p| p.0), Some(6));
        assert_eq!(percent(6, progress.last().and_then(|p| p.1)), Some(100));

        // 3. a TAMPERED artifact fails signature verification and yields no bytes
        let bad = check(&app, &stub, "/latest-tampered.json").unwrap().unwrap();
        let err = tauri::async_runtime::block_on(download_signed(&bad, |_, _| {}))
            .expect_err("tampered bytes must NOT verify");
        assert!(!err.is_empty());
        // ... and the full download_and_install path refuses it too: it never
        // reaches install (the process is still here to assert).
        let both = tauri::async_runtime::block_on(bad.download_and_install(|_, _| {}, || {}));
        assert!(both.is_err(), "download_and_install must fail verification");

        // 4. a bogus signature string is rejected as well
        let bogus = check(&app, &stub, "/latest-bogus-sig.json").unwrap().unwrap();
        assert!(tauri::async_runtime::block_on(download_signed(&bogus, |_, _| {})).is_err());

        // 5. an older feed is "no update"
        assert!(check(&app, &stub, "/latest-older.json").unwrap().is_none());

        // 6. a missing latest.json is an Err -> run_check falls back to GitHub
        assert!(check(&app, &stub, "/latest-missing.json").is_err());
    }
}
