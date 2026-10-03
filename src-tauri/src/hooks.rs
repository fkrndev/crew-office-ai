//! HookServer — port of `src/main/hooks.ts`.
//!
//! The Electron version listened on a unix socket / Windows named pipe and
//! was driven by `cth-hook.cjs` shims. This port listens on a TCP loopback
//! (`127.0.0.1:0`, dynamic port — the address goes to agents as `HIVE_SOCK`)
//! and is driven by the native `hive-hook` binary. The envelope/response
//! contract is unchanged.
//!
//! Semantics follow CURRENT upstream (hooks.ts:490): a Stop event marks the
//! agent idle and inbox delivery happens through the guarded idle-wake path —
//! NOT a forced `{decision:'block'}` continuation, which upstream removed for
//! bypassing terminal-draft/HITL safety.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::AppState;

#[derive(Debug, Deserialize)]
pub struct HookEnvelope {
    pub agent_id: Option<String>,
    pub event: Option<String>,
    #[serde(default)]
    pub payload: Value,
}

pub struct HookServer {
    pub addr: String,
}

impl HookServer {
    pub fn start(app: &AppHandle) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?.to_string();
        let handle = app.clone();
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(stream) = conn else { continue };
                let app = handle.clone();
                thread::spawn(move || serve_conn(app, stream));
            }
        });
        Ok(Self { addr })
    }
}

fn serve_conn(app: AppHandle, stream: std::net::TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return;
    }
    let Ok(env) = serde_json::from_str::<HookEnvelope>(line.trim()) else {
        return;
    };
    let response = handle(&app, &env);
    let mut out = serde_json::to_string(&response).unwrap_or_else(|_| "{}".into());
    out.push('\n');
    let mut w = stream;
    let _ = w.write_all(out.as_bytes());
}

fn emit_status(app: &AppHandle, id: &str, status: &str, detail: Option<&str>) {
    if let Some(st) = app.try_state::<AppState>() {
        let _ = st.hive.set_status(id, status);
    }
    let _ = app.emit(
        "agent-status",
        json!({ "id": id, "status": status, "detail": detail }),
    );
}

/// Port of hooks.ts `handle(p)` — pruned to what exists in this codebase:
/// no breaker/control/halt/realtime yet (PORTING.md TODOs), but the response
/// contract ({} default, hookSpecificOutput, additionalContext) is complete.
fn handle(app: &AppHandle, env: &HookEnvelope) -> Value {
    let Some(st) = app.try_state::<AppState>() else {
        return json!({});
    };
    let agent_id = env.agent_id.clone().unwrap_or_default();
    let event = env.event.clone().unwrap_or_else(|| "Unknown".into());
    let p = &env.payload;

    let _ = app.emit(
        "hook-event",
        json!({ "agentId": agent_id, "event": event }),
    );

    // Status-line telemetry (context_window) — pure telemetry, early {} exactly
    // like upstream: never feeds status or any gate.
    if event == "Status" {
        let cw = p.get("context_window");
        if let (Some(t), Some(l)) = (
            cw.and_then(|c| c.get("total_input_tokens")).and_then(|v| v.as_u64()),
            cw.and_then(|c| c.get("context_window_size")).and_then(|v| v.as_u64()),
        ) {
            if !agent_id.is_empty() {
                st.context_usage
                    .lock()
                    .unwrap()
                    .insert(agent_id.clone(), (t, l));
            }
            let _ = app.emit(
                "hive-context",
                json!({ "agentId": agent_id, "tokens": t, "limit": l }),
            );
        }
        return json!({});
    }

    // CostSample — proxy-bridge usage telemetry; ledger row, early {}.
    if event == "CostSample" {
        let _ = st.hive.append_cost_sample(&agent_id, p);
        return json!({});
    }

    // Session id bookkeeping for --resume + cost dedup.
    if let (Some(id), Some(sid)) = (
        (!agent_id.is_empty()).then_some(agent_id.as_str()),
        p.get("session_id").and_then(|v| v.as_str()),
    ) {
        let _ = st.hive.record_session(id, sid);
    }

    // 7C.3 — an operator HALT overrides everything at the next hook boundary:
    // stop CLEANLY ({continue:false}) rather than killing the PTY.
    if !agent_id.is_empty() && st.control.lock().unwrap().should_halt(&agent_id) {
        return json!({ "continue": false, "stopReason": "Halted by the operator from the floor." });
    }

    // Breaker inputs + the wake watchdog's hook channel (its only turn-proof
    // evidence for engines without telemetry).
    let now_ms = chrono::Utc::now().timestamp_millis() as u64;
    st.wake.lock().unwrap().note_hook(
        &agent_id,
        &event,
        p.get("message").and_then(|v| v.as_str()).unwrap_or(""),
        now_ms,
    );
    match event.as_str() {
        "PostToolUse" => st.breaker.lock().unwrap().record_tool_use(
            &agent_id,
            p.get("tool_name").and_then(|v| v.as_str()),
            p.get("tool_input").unwrap_or(&Value::Null),
            now_ms,
        ),
        "UserPromptSubmit" => st.breaker.lock().unwrap().record_user_prompt(&agent_id, now_ms),
        // Hermes api_request_error (translated via the shim's --hermes mode)
        // — the breaker's error-storm feed for engines without OTel.
        "ApiError" => st.breaker.lock().unwrap().record_error(&agent_id),
        "PreCompact" => st.breaker.lock().unwrap().record_compact_start(&agent_id, now_ms),
        "PostCompact" | "SessionStart" => {
            st.breaker.lock().unwrap().record_compact_end(&agent_id, now_ms)
        }
        _ => {}
    }

    match event.as_str() {
        "Stop" | "SubagentStop" => {
            // Upstream: respect an upstream Stop hook that already re-entered.
            if p.get("stop_hook_active").and_then(|v| v.as_bool()) == Some(true) {
                return json!({});
            }
            if !agent_id.is_empty() {
                emit_status(app, &agent_id, "idle", None);
                schedule_idle_delivery(app.clone(), agent_id);
            }
            json!({})
        }
        "PreToolUse" => {
            emit_status(
                app,
                &agent_id,
                "working",
                p.get("tool_name").and_then(|v| v.as_str()),
            );
            // 7C.1 — HITL deny at the tool boundary: race-free (no renderer
            // round-trip); slow human approval stays on the CLI's native prompt.
            let deny = st.control.lock().unwrap().tool_decision(
                &agent_id,
                p.get("tool_name").and_then(|v| v.as_str()).unwrap_or(""),
            );
            if let Some(reason) = deny {
                let _ = app.emit(
                    "control-approval",
                    json!({ "agentId": agent_id, "tool": p.get("tool_name"), "reason": reason }),
                );
                return json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": reason
                    }
                });
            }
            json!({})
        }
        "PostToolUse" => {
            emit_status(app, &agent_id, "working", None);
            // 7C.2 — mid-run steering: a queued operator note rides back as
            // additionalContext once, merged with the god roster below.
            injected_context(&st, &agent_id, &event)
        }
        "UserPromptSubmit" => {
            emit_status(app, &agent_id, "working", None);
            injected_context(&st, &agent_id, &event)
        }
        "SessionStart" => {
            emit_status(app, &agent_id, "idle", None);
            injected_context(&st, &agent_id, &event)
        }
        "Notification" => {
            emit_status(app, &agent_id, "waiting", None);
            json!({})
        }
        "PreCompact" => {
            emit_status(app, &agent_id, "compacting", None);
            json!({})
        }
        "PostCompact" => {
            emit_status(app, &agent_id, "idle", None);
            json!({})
        }
        _ => json!({}),
    }
}

/// additionalContext injection — merged into ONE value because a hook can only
/// return a single additionalContext (hooks.ts:523): god's live roster
/// (SessionStart/UserPromptSubmit, hooks.ts:530) + a queued operator steer
/// (UserPromptSubmit/PostToolUse, hooks.ts:526).
fn injected_context(st: &AppState, agent_id: &str, event: &str) -> Value {
    if agent_id.is_empty() {
        return json!({});
    }
    let mut parts: Vec<String> = Vec::new();

    if (event == "SessionStart" || event == "UserPromptSubmit") && st.hive.is_god(agent_id) {
        let roster = st
            .hive
            .registry()
            .iter()
            .map(|a| {
                format!(
                    "{}{}{}({})",
                    a.name,
                    if a.is_god { " [god] " } else { " " },
                    a.id,
                    a.status
                )
            })
            .collect::<Vec<_>>()
            .join(" | ");
        parts.push(format!("Live floor roster: {roster}"));
    }

    if event == "UserPromptSubmit" || event == "PostToolUse" {
        if let Some(note) = st.control.lock().unwrap().take_steer(agent_id) {
            parts.push(format!("Operator guidance: {note}"));
        }
    }

    if parts.is_empty() {
        json!({})
    } else {
        json!({
            "hookSpecificOutput": {
                "hookEventName": event,
                "additionalContext": parts.join("\n")
            }
        })
    }
}

/// The guarded idle-only delivery path: after a Stop, if the agent is still
/// idle and has unread hive mail, type a work-order into its PTY — the
/// renderer-nudge equivalent. Never fires mid-work; the status check at
/// delivery time is the guard.
fn schedule_idle_delivery(app: AppHandle, agent_id: String) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let Some(st) = app.try_state::<AppState>() else {
            return;
        };
        let still_idle = st
            .hive
            .registry()
            .iter()
            .any(|a| a.id == agent_id && a.status == "idle");
        if !still_idle {
            return;
        }
        // A queued /compact drains at the idle boundary — compacts between
        // steps, never mid-step (upstream's renderer-side queue, backend-side).
        if st.compact_queue.lock().unwrap().remove(&agent_id) {
            let mut ptys = st.ptys.lock().unwrap();
            if let Some(h) = ptys.get_mut(&agent_id) {
                let _ = h.write("/compact\r");
            }
            drop(ptys);
        }
        let Ok(inbox) = st.hive.inbox(&agent_id) else {
            return;
        };
        if inbox.is_empty() {
            return;
        }
        let dir = st.hive.agent_dir(&agent_id);
        let nudge = format!(
            "You have {} unread hive message(s) in {}/inbox — read each one, act on it, \
             move it to inbox/.done when handled, and reply via your outbox/.",
            inbox.len(),
            dir.display()
        );
        let mut ptys = st.ptys.lock().unwrap();
        if let Some(h) = ptys.get_mut(&agent_id) {
            let _ = h.write(&format!("{nudge}\r"));
        }
    });
}
