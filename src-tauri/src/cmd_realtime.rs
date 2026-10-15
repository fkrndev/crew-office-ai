//! Tauri commands — the sessionless `realtime:*` IPC surface (ports of
//! `realtime:setSessionLive`, `realtime:drainCompletions`, `realtime:waitFor`,
//! plus a dispatch tracker and a voice-tool catalog probe).
//!
//! The OpenAI Realtime WebRTC transport is not ported; everything here works
//! without a live voice session. The watcher (realtime.rs) is driven on demand:
//! every command call below polls it, and `poll(&st.hive, Some(&app))` can be
//! hooked into the lib.rs router_tick loop for continuous watching.

use crate::lock_ext::LockExt;
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::commands::state;
use crate::realtime::{PendingDispatch, RealtimeCompletion, WATCHER};

/// persist kv mirror of the session-live flag (upstream kept it in-memory on
/// the watcher + floor watcher).
const KV_SESSION_LIVE: &str = "realtime_session_live";

fn read_live_flag(app: &AppHandle) -> bool {
    state(&app)
        .persist
        .lock_safe()
        .get_kv(KV_SESSION_LIVE)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

// ─── realtime:hasKey / realtime:mintToken (realtime.ts) ─────────────────────
// MAIN-only OpenAI BYOK key — the SAME slot Settings writes through
// provider_key_set("openai") (PROVIDER_KEY_OPENAI, keychain with DPAPI-file
// fallback), with upstream's raw `apikey:openai` ref as a legacy fallback.
// hasKey reports presence only; mintToken decrypts once to mint a
// SHORT-LIVED ephemeral client secret — only the token + session config
// cross IPC, the real key is never returned or logged.

const OPENAI_KEY_SLOT: &str = "PROVIDER_KEY_OPENAI";
const OPENAI_KEY_LEGACY_REF: &str = "apikey:openai";
const REALTIME_MODEL: &str = "gpt-realtime-2.1";
const CLIENT_SECRETS_URL: &str = "https://api.openai.com/v1/realtime/client_secrets";
const LEGACY_SESSIONS_URL: &str = "https://api.openai.com/v1/realtime/sessions";

fn openai_key(app: &AppHandle) -> Option<String> {
    let dir = state(app).config_store.dir().to_path_buf();
    crate::misc::secret_get_persist(OPENAI_KEY_SLOT, &dir)
        .or_else(|| crate::misc::secret_get(OPENAI_KEY_LEGACY_REF))
        .filter(|k| !k.trim().is_empty())
}

/// realtime:hasKey — boolean presence only; gates the voice toggle.
#[tauri::command(async)]
pub fn realtime_has_key(app: AppHandle) -> Result<bool, String> {
    crate::panic_guard::cmd_guard("realtime_has_key", || {
    Ok(openai_key(&app).is_some())
    })
}

/// realtime:mintToken — GA client_secrets shape first, legacy sessions
/// fallback on 404; both response shapes normalized to {value, expires_at}.
#[tauri::command(async)]
pub fn realtime_mint_token(app: AppHandle, model: Option<String>) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("realtime_mint_token", || {
    let model = model
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| REALTIME_MODEL.into());
    let Some(key) = openai_key(&app) else {
        return Ok(json!({
            "ok": false,
            "error": "no OpenAI API key set — add one in Settings → Voice",
            "code": "no_key",
        }));
    };
    let post = |url: &str, body: &Value| -> Result<(u16, Value, String), String> {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(15)))
            .http_status_as_error(false)
            .build()
            .new_agent();
        let mut r = agent
            .post(url)
            .header("Authorization", &format!("Bearer {key}"))
            .header("Content-Type", "application/json")
            .send_json(body)
            .map_err(|e| format!("{e}"))?;
        let status = r.status().as_u16();
        let text = r.body_mut().read_to_string().unwrap_or_default();
        let j: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
        Ok((status, j, text))
    };
    let mut res = post(
        CLIENT_SECRETS_URL,
        &json!({ "session": { "type": "realtime", "model": model } }),
    );
    if let Ok((404, ..)) = res {
        res = post(LEGACY_SESSIONS_URL, &json!({ "model": model }));
    }
    match res {
        Err(e) => Ok(json!({ "ok": false, "error": e, "code": "network" })),
        Ok((status, j, text)) if !(200..300).contains(&status) => {
            let msg = j["error"]["message"]
                .as_str()
                .map(str::to_string)
                .or_else(|| Some(text.chars().take(200).collect()))
                .unwrap_or_else(|| format!("HTTP {status}"));
            Ok(
                json!({ "ok": false, "error": format!("token mint failed ({status}): {msg}"), "code": "mint_failed" }),
            )
        }
        Ok((_status, j, _text)) => {
            let token = j["value"]
                .as_str()
                .or_else(|| j["client_secret"]["value"].as_str())
                .unwrap_or("");
            if token.is_empty() {
                return Ok(
                    json!({ "ok": false, "error": "mint returned no ephemeral token", "code": "no_token" }),
                );
            }
            let expires_at = j["expires_at"]
                .as_u64()
                .or_else(|| j["client_secret"]["expires_at"].as_u64());
            Ok(json!({
                "ok": true,
                "token": token,
                "expiresAt": expires_at,
                "sessionConfig": { "model": model },
            }))
        }
    }
    })
}

/// realtime:setSessionLive — emit vs queue for detected completions.
#[tauri::command(async)]
pub fn realtime_set_session_live(app: AppHandle, live: bool) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("realtime_set_session_live", || {
    state(&app)
        .persist
        .lock_safe()
        .set_kv(KV_SESSION_LIVE, &json!(live));
    WATCHER.lock_safe().set_session_live(live);
    // Upstream toggles BOTH watchers on the same flag — the floor delta
    // watcher re-primes off a fresh snapshot either way.
    crate::realtime::FLOOR_WATCHER
        .lock_safe()
        .set_session_live(live);
    Ok(json!({ "ok": true }))
    })
}

/// Track a voice dispatch so the watcher can detect its completion (upstream
/// `trackDispatch` dep → completionWatcher.track). The port has no live
/// session to dispatch from, so the UI/hive callers register dispatches here.
/// `dispatch` is upstream's PendingDispatch minus kind (forced "dispatch").
#[tauri::command(async)]
pub fn realtime_track_dispatch(_app: AppHandle, dispatch: Value) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("realtime_track_dispatch", || {
    let mut rec: PendingDispatch =
        serde_json::from_value(dispatch).map_err(|e| format!("bad dispatch record: {e}"))?;
    rec.kind = crate::realtime::PendingKind::Dispatch;
    if rec.correlation_id.is_empty() {
        return Err("correlationId required".into());
    }
    WATCHER.lock_safe().track(rec);
    Ok(json!({ "ok": true }))
    })
}

/// realtime:drainCompletions — poll once, then return + clear the queue of
/// completions that landed while no session was live (upstream warm-start).
#[tauri::command(async)]
pub fn realtime_drain_completions(app: AppHandle) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("realtime_drain_completions", || {
    let st = state(&app);
    let mut w = WATCHER.lock_safe();
    w.set_session_live(read_live_flag(&app));
    w.poll(&st.hive, Some(&app));
    let out = w.drain();
    Ok(serde_json::to_value(&out).unwrap_or_else(|_| json!([])))
    })
}

/// realtime:waitFor — the `wait_for(taskId)` tool path. Resolves with the
/// completion event, or `{ timedOut: true, taskId }` after the timeout
/// (upstream default 120s; the renderer clamps to 600s max). The subject may
/// be a task id, a tracked correlation id, or a registry agent id.
#[tauri::command(async)]
pub async fn realtime_wait_for(
    app: AppHandle,
    task_id_or_agent: String,
    timeout_ms: Option<u64>,
) -> Result<Value, String> {
    crate::panic_guard::cmd_guard_async("realtime_wait_for", async move {
    let subject = task_id_or_agent.trim().to_string();
    if subject.is_empty() {
        return Ok(json!({ "timedOut": true, "taskId": "" }));
    }
    let timeout = timeout_ms
        .filter(|t| *t > 0)
        .unwrap_or(120_000)
        .min(600_000);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout);
    loop {
        let found: Option<RealtimeCompletion> = {
            let st = state(&app);
            let mut w = WATCHER.lock_safe();
            w.set_session_live(read_live_flag(&app));
            // Register an untracked subject so detection (and a later drain
            // after a timeout) still sees it — upstream's syntheticPending.
            w.ensure_wait(&subject, &st.hive);
            let events = w.poll(&st.hive, Some(&app));
            events
                .into_iter()
                .find(|e| crate::realtime::CompletionWatcher::matches_subject(e, &subject))
        };
        if let Some(ev) = found {
            return Ok(serde_json::to_value(&ev).unwrap_or_else(|_| json!({})));
        }
        if std::time::Instant::now() >= deadline {
            return Ok(json!({ "timedOut": true, "taskId": subject }));
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    }).await
}

/// The voice-tool catalog — which realtimeActions.ts verbs are backed by
/// ported commands. Mirrors the upstream tool list (names, verb, soft vs
/// destructive tiering); `backing` names the Tauri command that carries the
/// equivalent behavior. Session-bound pieces (the two-phase verbal confirm
/// spine, the renderer enqueue path) are listed but flagged unavailable.
#[tauri::command(async)]
pub fn realtime_actions_status(app: AppHandle) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("realtime_actions_status", || {
    let tools = json!([
        // ── soft writes (upstream executed these immediately) ──────────────
        { "name": "ping_agent", "verb": "ping", "tier": "soft", "available": true,
          "backing": "hive_send",
          "description": "Send a short message to one agent." },
        { "name": "dispatch_agent", "verb": "dispatch", "tier": "soft", "available": true,
          "backing": "hive_send + realtime_track_dispatch",
          "description": "Give an agent a structured work order delivered to their inbox." },
        { "name": "steer_agent", "verb": "steer", "tier": "soft", "available": true,
          "backing": "control_steer",
          "description": "Inject live guidance into a running agent." },
        { "name": "create_task", "verb": "create_task", "tier": "soft", "available": true,
          "backing": "hive_add_task",
          "description": "Add a new card to the task board." },
        { "name": "assign_task", "verb": "assign_task", "tier": "soft", "available": true,
          "backing": "task_update",
          "description": "Assign an existing task to an agent." },
        { "name": "update_task", "verb": "update_task", "tier": "soft", "available": true,
          "backing": "task_update",
          "description": "Change a task's status / result / assignee." },
        { "name": "delete_task", "verb": "delete_task", "tier": "soft", "available": true,
          "backing": "hive_delete_task",
          "description": "Delete one task card from the board." },
        { "name": "resume_agent", "verb": "resume", "tier": "soft", "available": true,
          "backing": "control_resume",
          "description": "Resume a paused or halted agent." },
        { "name": "set_auto_delivery", "verb": "auto_delivery", "tier": "soft", "available": true,
          "backing": "control_pause_delivery",
          "description": "Pause or resume automatic message-queue delivery to one agent." },
        { "name": "gate_tool", "verb": "gate_tool", "tier": "soft", "available": true,
          "backing": "control_gate_tool",
          "description": "Block or unblock one named tool for one agent." },
        { "name": "unarchive_agent", "verb": "unarchive", "tier": "soft", "available": true,
          "backing": "hive_set_archived",
          "description": "Bring an archived agent back onto the roster." },
        { "name": "wait_for", "verb": "wait_for", "tier": "soft", "available": true,
          "backing": "realtime_wait_for",
          "description": "Wait until a dispatched task completes (bounded by a timeout)." },
        // ── destructive / expensive (upstream required the verbal echo-back
        //    confirm spine — see confirm_action below) ──────────────────────
        { "name": "spawn_agent", "verb": "spawn", "tier": "destructive", "available": true,
          "backing": "spawn_agent",
          "description": "Hire a new agent worker." },
        { "name": "kill_agent", "verb": "kill", "tier": "destructive", "available": true,
          "backing": "kill_agent",
          "description": "Terminate a running agent. God/mass-ops are forbidden upstream." },
        { "name": "pause_agent", "verb": "pause", "tier": "destructive", "available": true,
          "backing": "control_pause",
          "description": "Pause a running agent." },
        { "name": "halt_agent", "verb": "halt", "tier": "destructive", "available": true,
          "backing": "control_halt",
          "description": "Hard-stop an agent's current work." },
        { "name": "archive_agent", "verb": "archive", "tier": "destructive", "available": true,
          "backing": "hive_set_archived",
          "description": "Archive an agent — off the floor, history kept." },
        { "name": "edit_schedule", "verb": "edit_schedule", "tier": "destructive", "available": true,
          "backing": "missions_save",
          "description": "Enable, disable, or delete a recurring scheduled mission." },
        { "name": "create_schedule", "verb": "create_schedule", "tier": "destructive", "available": true,
          "backing": "missions_save",
          "description": "Create a recurring schedule that messages an agent on an interval." },
        // ── not portable without the live session / renderer seam ─────────
        { "name": "clear_agent_context", "verb": "clear_context", "tier": "destructive", "available": false,
          "backing": null,
          "description": "Upstream rode the renderer's per-agent message queue (realtime:enqueue) — not ported." },
        { "name": "update_setting", "verb": "update_setting", "tier": "destructive", "available": false,
          "backing": null,
          "description": "Upstream gated config writes through a per-key voice policy table — not ported; use the typed set_* commands." },
        { "name": "confirm_action", "verb": "confirm", "tier": "confirm", "available": false,
          "backing": null,
          "description": "Commits the pending destructive action — the single-slot echo-back spine lived in realtimeActions.ts and is session-bound." },
        { "name": "cancel_action", "verb": "cancel", "tier": "confirm", "available": false,
          "backing": null,
          "description": "Drops the pending destructive action (same session-bound spine)." }
    ]);
    let w = WATCHER.lock_safe();
    Ok(json!({
        "ok": true,
        "voice_actor": crate::realtime::VOICE_ACTOR,
        "session_live": read_live_flag(&app),
        "pending_dispatches": w.pending_count(),
        "tools": tools,
    }))
    })
}
