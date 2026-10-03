//! Tauri commands — the `analytics:*` IPC surface (upstream `analytics:messageSent`
//! plus the Settings → Privacy opt-out and a status probe).
//!
//! Everything is validated backend-side against the upstream EVENTS allowlist
//! (analytics.rs) — unknown events and non-allowlisted property keys are
//! dropped, never sent. The renderer may only name the `terminal`/`composer`
//! message surfaces; `steer`/`hive` are counted by the backend itself.

use serde_json::{json, Map, Value};
use tauri::AppHandle;

/// analytics:messageSent — one allowlisted event with allowlisted string
/// props. `event` is validated against the upstream EVENTS table; for
/// `message_sent` the surface must be a renderer-allowed one (upstream
/// isRendererMessageSurface). Never errors: a refused event is a silent no-op
/// upstream, so we return ok:false instead of throwing.
#[tauri::command]
pub fn analytics_message_sent(
    app: AppHandle,
    event: String,
    props: Option<Map<String, Value>>,
) -> Result<Value, String> {
    let props = props.unwrap_or_default();
    if crate::analytics::event_allowlist(&event).is_none() {
        return Ok(json!({ "ok": false, "error": "event not allowlisted" }));
    }
    crate::analytics::global().track_from_renderer(&app, &event, &props);
    Ok(json!({ "ok": true }))
}

/// Settings → Privacy toggle — `on=false` opts out (persist kv
/// `analytics_opt_out`); sends stop instantly.
#[tauri::command]
pub fn analytics_set_opt_in(app: AppHandle, on: bool) -> Result<Value, String> {
    crate::analytics::global().set_opt_in(&app, on);
    Ok(json!({ "ok": true, "opted_in": on }))
}

/// Diagnostics — whether a send could happen right now and why not. The key
/// itself is never exposed; `has_key` reports only whether the build carries
/// one (empty in dev/fork builds, per upstream).
#[tauri::command]
pub fn analytics_status(app: AppHandle) -> Result<Value, String> {
    let a = crate::analytics::global();
    Ok(json!({
        "ok": true,
        "has_key": !a.posthog_key.is_empty(),
        "endpoint": a.endpoint,
        "opted_in": !a.opted_out(&app),
        "do_not_track": std::env::var("DO_NOT_TRACK")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false),
    }))
}
