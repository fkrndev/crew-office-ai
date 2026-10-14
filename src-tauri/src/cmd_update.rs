//! update:* — port of `src/main/updater.ts`.
//!
//! Upstream tries electron-updater first (signed feed) and falls back to a
//! notify-only check against GitHub `releases/latest` (`available-manual`
//! state + a release-page/download URL). This port ships no signed update
//! feed, so the notify-only path IS the implementation — same shape, same
//! states, same semver compare and asset picker.
//!
//! States emitted on `update-state`: checking / not-available /
//! available-manual / downloaded / just-updated / error / idle.

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

/// Releases feed this build checks — the app's own repo.
const REPO: &str = "fkrndev/crew-office-ai";
const UA: &str = "agent-office-updater";

#[derive(Default)]
pub struct UpdateState(pub std::sync::Mutex<Value>);

fn emit(app: &AppHandle, status: Value) {
    if let Some(st) = app.try_state::<UpdateState>() {
        *st.0.lock().unwrap() = status.clone();
    }
    let _ = app.emit("update:status", status);
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

fn fetch_latest() -> Result<Value, String> {
    let resp = ureq::get(&format!(
        "https://api.github.com/repos/{REPO}/releases/latest"
    ))
    .header("User-Agent", UA)
    .header("Accept", "application/vnd.github+json")
    .call()
    .map_err(|e| e.to_string())?;
    resp.into_body()
        .read_json::<Value>()
        .map_err(|e| e.to_string())
}

/// update:checkNow — notify-only check; emits update-state transitions.
#[tauri::command(async)]
pub fn update_check_now(app: AppHandle) -> Result<Value, String> {
    emit(&app, json!({ "state": "checking" }));
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let current = env!("CARGO_PKG_VERSION");
        match fetch_latest() {
            Ok(rel) => {
                let tag = rel
                    .get("tag_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if is_newer(tag, current) {
                    let st = json!({
                        "state": "available-manual",
                        "version": tag.trim_start_matches('v'),
                        "url": rel.get("html_url").and_then(|v| v.as_str())
                            .unwrap_or(&format!("https://github.com/{REPO}/releases/latest")),
                        "downloadUrl": pick_download_asset(&rel["assets"]),
                        "notes": rel.get("body").and_then(|v| v.as_str()),
                    });
                    emit(&app2, st.clone());
                } else {
                    emit(&app2, json!({ "state": "not-available" }));
                }
            }
            Err(e) => emit(&app2, json!({ "state": "error", "message": e })),
        }
    });
    Ok(json!({ "ok": true }))
}

/// update:current — the last emitted status (or idle).
#[tauri::command(async)]
pub fn update_current(app: AppHandle) -> Result<Value, String> {
    Ok(app
        .try_state::<UpdateState>()
        .map(|s| s.0.lock().unwrap().clone())
        .unwrap_or_else(|| json!({ "state": "idle" })))
}

/// update:openRelease — open a releases URL; guarded to this repo's release
/// pages (upstream's prefix guard — download URLs live under /download/).
#[tauri::command(async)]
pub fn update_open_release(app: AppHandle, url: String) -> Result<bool, String> {
    let ok = url.starts_with(&format!("https://github.com/{REPO}/releases"));
    if !ok {
        return Err("url is not a release page".into());
    }
    crate::cmd_misc::app_open_external(app, url)?;
    Ok(true)
}

/// update:download / update:restartAndInstall — no signed feed in this build;
/// the honest answer is "manual": the caller opens downloadUrl instead.
#[tauri::command(async)]
pub fn update_download(app: AppHandle) -> Result<Value, String> {
    let _ = app;
    Ok(json!({ "ok": false, "error": "manual-download", "state": "available-manual" }))
}

#[tauri::command(async)]
pub fn update_restart_and_install(app: AppHandle) -> Result<Value, String> {
    let _ = app;
    Ok(json!({ "ok": false, "error": "manual-download", "state": "available-manual" }))
}

/// update:simulate — DEV ONLY (upstream's debug surface).
#[tauri::command(async)]
pub fn update_simulate(
    app: AppHandle,
    version: Option<String>,
    notes: Option<String>,
) -> Result<bool, String> {
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
}
