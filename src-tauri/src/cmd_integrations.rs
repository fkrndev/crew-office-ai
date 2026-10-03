//! IPC surface for the integrations registry + loopback secret broker — port
//! of index.ts `integrations:*` handlers plus explicit broker controls:
//!   integrations:list      → integrations_list      (redacted, hasSecret only)
//!   integrations:templates → integrations_templates
//!   integrations:upsert    → integrations_upsert(record)
//!   integrations:remove    → integrations_remove(payload{id})
//!   integrations:setSecret → integrations_set_secret(payload{id,secret,key?})
//!   integrations:test      → integrations_test(payload{id,path?})
//!   broker start/stop/status/token — explicit commands (upstream started the
//!   broker implicitly at bootstrap and granted tokens at worker spawn).
//!
//! Records are metadata only; secrets ride the OS keychain and are NEVER
//! returned over IPC.

use std::path::PathBuf;

use serde_json::{json, Value};
use tauri::AppHandle;

use crate::commands::state;
use crate::integrations::{self, BROKER};

/// <harness> = parent of the hive root (<harness>/hive).
fn harness_home(app: &AppHandle) -> PathBuf {
    let st = state(app);
    st.hive
        .root
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| st.hive.root.clone())
}

/// `integrations:list` — records with secretRef redacted to `hasSecret`.
#[tauri::command]
pub fn integrations_list(app: AppHandle) -> Result<Value, String> {
    Ok(json!(integrations::list_redacted(&harness_home(&app))))
}

/// `integrations:templates` — the v1 reference presets.
#[tauri::command]
pub fn integrations_templates() -> Result<Value, String> {
    Ok(integrations::templates())
}

/// `integrations:upsert` — validated create/replace; same {ok,record|error} shape.
#[tauri::command]
pub fn integrations_upsert(app: AppHandle, record: Value) -> Result<Value, String> {
    match integrations::upsert_record(&harness_home(&app), &record) {
        Ok(rec) => Ok(json!({ "ok": true, "record": rec })),
        Err(e) => Ok(json!({ "ok": false, "error": e })),
    }
}

/// `integrations:remove` — drops the record AND its stored secret.
#[tauri::command]
pub fn integrations_remove(app: AppHandle, payload: Value) -> Result<Value, String> {
    let id = payload
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if id.is_empty() {
        return Ok(json!({ "ok": false, "error": "id required" }));
    }
    match integrations::remove_record(&harness_home(&app), id) {
        Ok(()) => Ok(json!({ "ok": true })),
        Err(e) => Ok(json!({ "ok": false, "error": e })),
    }
}

/// `integrations:setSecret` — {id, secret, key?} → keychain only, never echoed.
#[tauri::command]
pub fn integrations_set_secret(app: AppHandle, payload: Value) -> Result<Value, String> {
    let _ = app;
    let id = payload.get("id").and_then(|v| v.as_str()).unwrap_or("");
    if id.is_empty() {
        return Ok(json!({ "ok": false, "error": "id required" }));
    }
    let secret = payload
        .get("secret")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if secret.is_empty() {
        return Ok(json!({ "ok": false, "error": "secret required" }));
    }
    let key = payload
        .get("key")
        .and_then(|v| v.as_str())
        .unwrap_or("secret");
    match integrations::set_secret_key(id, key, secret) {
        Ok(()) => Ok(json!({ "ok": true })),
        Err(e) => Ok(json!({ "ok": false, "error": e })),
    }
}

/// `integrations:test` — probe reachability through the broker's own auth
/// path (confined path resolved BEFORE the secret is materialized; only the
/// upstream status crosses back).
#[tauri::command]
pub fn integrations_test(app: AppHandle, payload: Value) -> Result<Value, String> {
    let id = payload.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let path = payload
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    Ok(integrations::test_integration(&harness_home(&app), id, path))
}

/// Start the loopback broker (idempotent). `preferred_port` 0/absent ⇒ OS pick.
/// Resolves {ok, port, url} or {ok:false, error} — never a Tauri error.
#[tauri::command]
pub fn integrations_broker_start(
    app: AppHandle,
    preferred_port: Option<u16>,
) -> Result<Value, String> {
    match BROKER.start(&harness_home(&app), preferred_port.unwrap_or(0)) {
        Ok(port) => Ok(json!({ "ok": true, "port": port, "url": BROKER.url() })),
        Err(e) => Ok(json!({ "ok": false, "error": e })),
    }
}

/// Stop the broker and clear all capabilities (upstream stop()).
#[tauri::command]
pub fn integrations_broker_stop(app: AppHandle) -> Result<Value, String> {
    let _ = app;
    BROKER.stop_broker();
    Ok(json!({ "ok": true }))
}

/// {running, url, port, capabilities} — secrets are not part of status.
#[tauri::command]
pub fn integrations_broker_status(app: AppHandle) -> Result<Value, String> {
    let _ = app;
    Ok(json!({
        "running": BROKER.running(),
        "url": if BROKER.running() { Value::from(BROKER.url()) } else { Value::Null },
        "port": BROKER.port(),
        "capabilities": BROKER.capability_count(),
    }))
}

/// Issue a per-worker capability token — upstream granted one implicitly at
/// worker spawn (`grant(workerId, enabledIds())` + MD_BROKER_URL/TOKEN env).
/// payload: {workerId, allowedIds?} — allowedIds defaults to the currently
/// enabled integrations. Returns {ok, token} — the token is a handle, never a
/// secret, and lives only in broker memory.
#[tauri::command]
pub fn integrations_broker_token(app: AppHandle, payload: Value) -> Result<Value, String> {
    if !BROKER.running() {
        return Ok(json!({ "ok": false, "error": "broker not running" }));
    }
    let worker_id = payload
        .get("workerId")
        .or_else(|| payload.get("worker_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if worker_id.is_empty() {
        return Ok(json!({ "ok": false, "error": "workerId required" }));
    }
    let home = harness_home(&app);
    let allowed: Vec<String> = payload
        .get("allowedIds")
        .or_else(|| payload.get("allowed_ids"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_else(|| integrations::enabled_ids(&home));
    let token = BROKER.grant(worker_id, allowed.clone());
    Ok(json!({
        "ok": true,
        "token": token,
        "workerId": worker_id,
        "allowedIds": allowed,
        "url": BROKER.url(),
    }))
}
