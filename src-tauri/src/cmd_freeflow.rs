//! Tauri commands — the `freeflow:*` IPC surface (port of the
//! `freeflow:setConfig` / `freeflow:transcribe` ipcMain handlers in index.ts).
//!
//! Config is split the way the port spec dictates: the Groq API key lives in
//! the OS credential store (keyring service `agent-office`, key
//! `GROQ_API_KEY`) — upstream kept it in HarnessConfig; here it never touches
//! disk plaintext. The `enabled` flag and `model` live in the persist kv
//! (`freeflow_enabled`, `freeflow_model`).

use crate::lock_ext::LockExt;
use base64::Engine;
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::commands::state;
use crate::freeflow::{TranscribeOptions, DEFAULT_GROQ_MODEL};

const KEYRING_SERVICE: &str = "agent-office";
const GROQ_KEY: &str = "GROQ_API_KEY";
const KV_ENABLED: &str = "freeflow_enabled";
const KV_MODEL: &str = "freeflow_model";

fn groq_key() -> Option<String> {
    keyring::Entry::new(KEYRING_SERVICE, GROQ_KEY)
        .ok()
        .and_then(|e| e.get_password().ok())
        .filter(|k| !k.is_empty())
}

fn freeflow_enabled(app: &AppHandle) -> bool {
    state(&app)
        .persist
        .lock_safe()
        .get_kv(KV_ENABLED)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn freeflow_model(app: &AppHandle) -> String {
    state(&app)
        .persist
        .lock_safe()
        .get_kv(KV_MODEL)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| DEFAULT_GROQ_MODEL.to_string())
}

/// freeflow:setConfig — {enabled?, apiKey?|api_key?, model?}. Trim string
/// fields; an emptied key deletes the credential (upstream: `|| undefined`).
#[tauri::command(async)]
pub fn freeflow_set_config(app: AppHandle, patch: Value) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("freeflow_set_config", || {
    let st = state(&app);
    let p = &patch;

    if let Some(enabled) = p.get("enabled").and_then(|v| v.as_bool()) {
        st.persist
            .lock_safe()
            .set_kv(KV_ENABLED, &json!(enabled));
    }
    let api_key = p
        .get("apiKey")
        .or_else(|| p.get("api_key"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string());
    if let Some(k) = api_key {
        let entry = keyring::Entry::new(KEYRING_SERVICE, GROQ_KEY).map_err(|e| e.to_string())?;
        if k.is_empty() {
            let _ = entry.delete_credential();
        } else {
            entry
                .set_password(&k)
                .map_err(|e| format!("keychain: {e}"))?;
        }
    }
    if let Some(model) = p.get("model").and_then(|v| v.as_str()) {
        let model = model.trim();
        st.persist.lock_safe().set_kv(
            KV_MODEL,
            &json!(if model.is_empty() {
                DEFAULT_GROQ_MODEL
            } else {
                model
            }),
        );
    }
    Ok(json!({ "ok": true }))
    })
}

/// freeflow:transcribe — one captured clip (base64) via Groq. Gated on the
/// flag + a key being present, so a disabled feature can NEVER reach the
/// network. Only the audio bytes cross IPC inbound, the transcript outbound.
#[tauri::command(async)]
pub fn freeflow_transcribe(
    app: AppHandle,
    audio_b64: String,
    mime: Option<String>,
    filename: Option<String>,
    language: Option<String>,
) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("freeflow_transcribe", || {
    if !freeflow_enabled(&app) {
        return Ok(json!({ "ok": false, "error": "Free Flow is disabled" }));
    }
    let Some(api_key) = groq_key() else {
        return Ok(json!({ "ok": false, "error": "no Groq API key set" }));
    };
    let audio = match base64::engine::general_purpose::STANDARD.decode(audio_b64.trim()) {
        Ok(b) => b,
        Err(_) => return Ok(json!({ "ok": false, "error": "no audio" })),
    };
    let out = crate::freeflow::transcribe_with_groq(&TranscribeOptions {
        api_key,
        audio,
        mime_type: mime.filter(|m| !m.is_empty()),
        filename: filename.filter(|f| !f.is_empty()),
        model: Some(freeflow_model(&app)),
        language: language.filter(|l| !l.is_empty()),
    });
    if out.ok {
        crate::analytics::global().track_feature(&app, "voice_dictation");
    }
    Ok(serde_json::to_value(out).unwrap_or_else(|_| json!({ "ok": false, "error": "serialize" })))
    })
}

/// Settings-gate probe — presence of the key + flag, never the key itself.
#[tauri::command(async)]
pub fn freeflow_status(app: AppHandle) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("freeflow_status", || {
    Ok(json!({
        "ok": true,
        "enabled": freeflow_enabled(&app),
        "has_key": groq_key().is_some(),
        "model": freeflow_model(&app),
    }))
    })
}
