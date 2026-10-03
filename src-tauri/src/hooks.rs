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
            json!({})
        }
        "PostToolUse" => {
            emit_status(app, &agent_id, "working", None);
            json!({})
        }
        "UserPromptSubmit" => {
            emit_status(app, &agent_id, "working", None);
            maybe_roster_context(&st, &agent_id, &event)
        }
        "SessionStart" => {
            emit_status(app, &agent_id, "idle", None);
            maybe_roster_context(&st, &agent_id, &event)
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

/// God's live roster, injected as additionalContext on SessionStart and every
/// prompt (hooks.ts:530) — god always sees the current floor instead of a
/// transcript describing an old one.
fn maybe_roster_context(st: &AppState, agent_id: &str, event: &str) -> Value {
    if agent_id.is_empty() || !st.hive.is_god(agent_id) {
        return json!({});
    }
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
    json!({
        "hookSpecificOutput": {
            "hookEventName": event,
            "additionalContext": format!("Live floor roster: {roster}")
        }
    })
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
