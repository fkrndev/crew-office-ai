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

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::AppState;

/// Per-launch shared secret for the hook socket. Handed to every agent as
/// `HIVE_TOKEN` and required on each envelope, so an arbitrary local process
/// that merely finds the loopback port cannot forge agent events.
pub fn hook_token() -> &'static str {
    static TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TOKEN.get_or_init(crate::misc::random_hex32)
}

/// Hard cap on one request line — an envelope is a small JSON object; this
/// bounds what a misbehaving local client can make us buffer.
const MAX_REQUEST_BYTES: u64 = 1 << 20;

/// Concurrent in-flight connections served at once. Hook calls are short; the
/// cap keeps a connect-flood from spawning unbounded threads.
const MAX_CONNECTIONS: usize = 64;

static ACTIVE_CONNS: AtomicUsize = AtomicUsize::new(0);

/// Releases an `ACTIVE_CONNS` slot on drop (also on panic/early return).
struct ConnSlot;
impl Drop for ConnSlot {
    fn drop(&mut self) {
        ACTIVE_CONNS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Reserve a connection slot, or None when the server is at capacity.
fn try_acquire_slot() -> Option<ConnSlot> {
    if ACTIVE_CONNS.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
        ACTIVE_CONNS.fetch_sub(1, Ordering::SeqCst);
        return None;
    }
    Some(ConnSlot)
}

/// Constant-time byte comparison (length is not secret — the token is a fixed
/// width hex string).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

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
                // At capacity → drop (close) the connection without a thread.
                let Some(slot) = try_acquire_slot() else {
                    continue;
                };
                let app = handle.clone();
                thread::spawn(move || {
                    let _slot = slot;
                    serve_conn(app, stream)
                });
            }
        });
        Ok(Self { addr })
    }
}

/// Parse + authenticate one request line. `None` = reject (malformed JSON,
/// missing/wrong `token`, unknown shape) — the caller closes silently.
///
/// Two wire shapes share this socket (hooks.ts handle() reads the
/// UNWRAPPED cth-hook payload — `hook_event_name` is the event):
///   - native hive-hook binary wraps:   { agent_id, event, payload, token }
///   - bundled JS bridges (Pi extension, OpenCode plugin) post the raw
///     cth-hook-shaped payload directly: { hook_event_name, agent_id, …, token }
/// For the latter, event = hook_event_name and the payload IS the whole
/// line (upstream's HookPayload — tool_name/session_id/etc. at top level).
/// BOTH shapes must carry the per-launch token.
fn parse_envelope(line: &str, token: &str) -> Option<HookEnvelope> {
    let mut raw = serde_json::from_str::<Value>(line.trim()).ok()?;
    let presented = raw.get("token").and_then(|v| v.as_str()).unwrap_or("");
    if !constant_time_eq(presented.as_bytes(), token.as_bytes()) {
        return None;
    }
    // The secret is not part of any event payload — never let it flow on.
    if let Some(o) = raw.as_object_mut() {
        o.remove("token");
    }
    if raw.get("hook_event_name").is_some() {
        Some(HookEnvelope {
            agent_id: raw
                .get("agent_id")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            event: raw
                .get("hook_event_name")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            payload: raw,
        })
    } else {
        serde_json::from_value::<HookEnvelope>(raw).ok()
    }
}

fn serve_conn(app: AppHandle, stream: std::net::TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let mut line = String::new();
    // Bounded read: a client that never sends a newline can't grow `line`
    // past the cap.
    if BufReader::new((&stream).take(MAX_REQUEST_BYTES))
        .read_line(&mut line)
        .is_err()
    {
        return;
    }
    let Some(env) = parse_envelope(&line, hook_token()) else {
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
        // The parked-compact drain: mission/context-trigger fires while the
        // agent was mid-work land here at the idle boundary (the same slot
        // upstream's renderer-side queue drained through). Provider verb
        // table: no trustworthy command → leave the terminal alone.
        if status == "idle" && st.compact_queue.lock().unwrap().remove(id) {
            let provider = st
                .hive
                .registry()
                .iter()
                .find(|a| a.id == id)
                .map(|a| a.provider.clone())
                .unwrap_or_default();
            let focus = st
                .config
                .lock()
                .unwrap()
                .context_trigger
                .compact
                .message
                .clone();
            if let Some(cmd) = crate::misc::compact_command(&provider, &focus) {
                // `id` is an AGENT id (the hook payload's AGENT_ID) — the PTY
                // map is keyed by pty id, so translate first.
                if let Some(pty_id) = st.pty_for_agent(id) {
                    crate::pty::write_pty(&st, &pty_id, &format!("{cmd}\r"));
                }
            }
        }
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

    // Upstream's hive:hookEvent payload (hooks.ts emit()) — additive fields
    // from the envelope payload; the channel name stays `hook-event` (the
    // frontend already listens on it). `blocked` is always false here —
    // upstream's emit never sets it true in hooks.ts either.
    let _ = app.emit(
        "hive:hookEvent",
        json!({
            "agentId": agent_id,
            "event": event,
            "tool": p.get("tool_name").and_then(|v| v.as_str()),
            "notificationType": p.get("notification_type").and_then(|v| v.as_str()),
            "source": p.get("source").and_then(|v| v.as_str()),
            "message": p.get("message").and_then(|v| v.as_str()),
            "blocked": false,
        }),
    );

    // Status-line telemetry (context_window) — pure telemetry, early {} exactly
    // like upstream: never feeds status or any gate.
    if event == "Status" {
        let cw = p.get("context_window");
        if let (Some(t), Some(l)) = (
            cw.and_then(|c| c.get("total_input_tokens"))
                .and_then(|v| v.as_u64()),
            cw.and_then(|c| c.get("context_window_size"))
                .and_then(|v| v.as_u64()),
        ) {
            if !agent_id.is_empty() {
                st.context_usage
                    .lock()
                    .unwrap()
                    .insert(agent_id.clone(), (t, l));
            }
            let _ = app.emit(
                "hive:contextUpdate",
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
        "UserPromptSubmit" => st
            .breaker
            .lock()
            .unwrap()
            .record_user_prompt(&agent_id, now_ms),
        // Hermes api_request_error (translated via the shim's --hermes mode)
        // — the breaker's error-storm feed for engines without OTel.
        "ApiError" => st.breaker.lock().unwrap().record_error(&agent_id),
        "PreCompact" => st
            .breaker
            .lock()
            .unwrap()
            .record_compact_start(&agent_id, now_ms),
        "PostCompact" | "SessionStart" => st
            .breaker
            .lock()
            .unwrap()
            .record_compact_end(&agent_id, now_ms),
        _ => {}
    }

    match event.as_str() {
        // A subagent finishing is NOT the parent going idle — marking the
        // parent idle would arm the idle-nudge/compact-drain/finished toast
        // while it is still working.
        "SubagentStop" => json!({}),
        "Stop" => {
            // Upstream: respect an upstream Stop hook that already re-entered.
            if p.get("stop_hook_active").and_then(|v| v.as_bool()) == Some(true) {
                return json!({});
            }
            if !agent_id.is_empty() {
                emit_status(app, &agent_id, "idle", None);
                // Upstream hooks.ts:497 — a finished agent toasts the
                // operator. No forced Stop-time inbox delivery (upstream
                // removed it at 490-499): the workerwake watchdog's guarded
                // idle path is the sole delivery channel.
                notify(app, &st, &agent_id, "finished — idle");
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
                    "control:approvalRequest",
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
            injected_context(&st, &agent_id, &event, p)
        }
        "UserPromptSubmit" => {
            emit_status(app, &agent_id, "working", None);
            injected_context(&st, &agent_id, &event, p)
        }
        "SessionStart" => {
            emit_status(app, &agent_id, "idle", None);
            injected_context(&st, &agent_id, &event, p)
        }
        "Notification" => {
            emit_status(app, &agent_id, "waiting", None);
            // hooks.ts:586-592 — an idle-type Notification ("waiting for your
            // input") is "blocked on the human", worth a toast; a permission
            // request surfaces natively in the agent's own session instead.
            let idle_notify = p.get("notification_type").and_then(|v| v.as_str()) == Some("idle")
                || p.get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_lowercase()
                    .contains("waiting for your input");
            if idle_notify {
                let body = p
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("needs your attention");
                notify(app, &st, &agent_id, body);
            }
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

/// breakerToast/idle-toast — upstream fires `new Notification` gated on the
/// `notifications` config (hooks.ts:602). tauri-plugin-notification gives the
/// real OS toast; `hive-notify` stays as an in-app mirror the renderer can
/// also surface.
fn notify(app: &AppHandle, st: &AppState, agent_id: &str, body: &str) {
    if !st.config.lock().unwrap().notifications {
        return;
    }
    let title = if agent_id.is_empty() {
        "Agent"
    } else {
        agent_id
    };
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title(title).body(body).show();
    let _ = app.emit(
        "hive-notify",
        json!({ "agentId": agent_id, "title": title, "body": body }),
    );
}

/// Standing goal from the shared roster file — port of index.ts
/// `standingGoalFromRoster` (reads `roster.json` agents[].goal; Edit Agent
/// saves land there via roster:write, so a re-read each hook picks changes up
/// without a worker restart).
fn standing_goal(st: &AppState, agent_id: &str) -> Option<String> {
    let snap = st.roster.read()?;
    for a in snap.get("agents")?.as_array()? {
        if a.get("id").and_then(|v| v.as_str()) != Some(agent_id) {
            continue;
        }
        let g = a.get("goal").and_then(|v| v.as_str()).unwrap_or("").trim();
        return if g.is_empty() {
            None
        } else {
            Some(g.to_string())
        };
    }
    None
}

/// hooks.ts:552-570 — the durable standing-goal injection. Delivered once per
/// session (a fresh session_id re-delivers), then only when the roster value
/// changes; clearing it injects the explicit revocation so the old briefing
/// doesn't silently keep steering the model.
fn goal_context(
    st: &AppState,
    agent_id: &str,
    event: &str,
    session_id: Option<&str>,
) -> Option<String> {
    let goal_raw = standing_goal(st, agent_id);
    let session = session_id.map(str::to_string);
    let mut delivered_map = st.delivered_goal.lock().unwrap();
    let delivered = delivered_map.get(agent_id).cloned();
    let new_session = delivered.as_ref().map(|d| d.0 != session).unwrap_or(true);
    let changed = delivered
        .as_ref()
        .is_some_and(|d| d.0 == session && d.1 != goal_raw);
    if event == "SessionStart" || new_session || changed {
        delivered_map.insert(agent_id.to_string(), (session, goal_raw.clone()));
        if let Some(g) = goal_raw {
            return Some(format!("<goal>\n{g}\n</goal>"));
        }
        if changed && delivered.as_ref().and_then(|d| d.1.clone()).is_some() {
            // Silence would leave the old briefing alive in the model's
            // context — revoke it explicitly (upstream verbatim).
            return Some(
                "<goal>\n[Cleared by the operator. Stop following the previous standing goal.]\n</goal>"
                    .to_string(),
            );
        }
    }
    None
}

/// Assemble the live fleet rows roster_context formats — upstream reads the
/// beat-written fleet.json (index.ts writeFleetSnapshot); this port keeps no
/// fleet cache, so the same row shape is built live and a best-effort
/// fleet.json is dropped alongside for agents told to Read it.
pub(crate) fn fleet_rows(st: &AppState) -> Vec<Value> {
    let now_ms = chrono::Utc::now().timestamp_millis() as u64;
    let extras = crate::hive_admin::all_extras(&st.hive);
    // Lifetime usd (reset-corrected fold) first; session usd is the cold
    // fallback — upstream's exact preference (index.ts:1305-1315).
    // (incremental, shared fold — only newly appended ledger bytes are read)
    let totals = st.hive.lifetime_usd();
    st.hive
        .registry()
        .iter()
        .filter(|a| {
            extras
                .get(&a.id)
                .and_then(|e| e.get("archived"))
                .and_then(|v| v.as_bool())
                != Some(true)
        })
        .map(|a| {
            let sample = st
                .telemetry
                .sample(&a.id)
                .or_else(|| st.hive.usage_sample(&a.id))
                .or_else(|| st.usage.sample(&a.cwd, a.session_id.as_deref()));
            let tokens = sample
                .map(|s| s.input + s.output + s.cache_read + s.cache_creation)
                .unwrap_or(0);
            let session_usd = (sample.map(|s| s.usd).unwrap_or(0.0) * 10_000.0).round() / 10_000.0;
            let usd = totals
                .as_ref()
                .map(|t| t.get(&a.id).copied().unwrap_or(0.0))
                .map(|u| (u * 10_000.0).round() / 10_000.0)
                .unwrap_or(session_usd);
            let last_tool = st.telemetry.spans(&a.id).last().map(|s| s.tool.clone());
            let breaker = serde_json::to_value(st.breaker.lock().unwrap().level_for(&a.id))
                .unwrap_or_else(|_| json!("healthy"));
            json!({
                "id": a.id,
                "name": a.name,
                "role": if a.role.is_empty() {
                    if a.is_god { "orchestrator" } else { "agent" }
                } else {
                    a.role.as_str()
                },
                "cwd": a.cwd,
                "isGod": a.is_god,
                "breaker": breaker,
                "tokens": tokens,
                "usd": usd,
                "sessionUsd": session_usd,
                "lastTool": last_tool,
                "lastActiveSecAgo": sample
                    .map(|s| (now_ms.saturating_sub(s.ts)) / 1000),
                "inboxBacklog": st.hive.inbox_backlog(&a.id),
                "onHold": extras
                    .get(&a.id)
                    .and_then(|e| e.get("onHold"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// additionalContext injection — merged into ONE value because a hook can only
/// return a single additionalContext (hooks.ts:523). Order matches upstream's
/// `[roster, goal, steer].filter(Boolean).join('\n\n')`: god's live roster
/// (SessionStart/UserPromptSubmit), the standing goal (same events, once per
/// session or on change), a queued operator steer (UserPromptSubmit/
/// PostToolUse).
fn injected_context(st: &AppState, agent_id: &str, event: &str, p: &Value) -> Value {
    if agent_id.is_empty() {
        return json!({});
    }
    let mut parts: Vec<String> = Vec::new();

    if (event == "SessionStart" || event == "UserPromptSubmit") && st.hive.is_god(agent_id) {
        let agents = fleet_rows(st);
        // Best-effort fleet.json — the roster text points agents at it, and
        // upstream's fleetTimer contract is "fresh when god looks".
        let _ = st.hive.write_fleet_snapshot(&json!({
            "ts": chrono::Utc::now().timestamp_millis() as u64,
            "agents": agents,
        }));
        let ctx_map = st.context_usage.lock().unwrap().clone();
        if let Some(r) = st.hive.roster_context(
            &agents,
            Some(chrono::Utc::now().timestamp_millis() as u64),
            |id| ctx_map.get(id).copied(),
        ) {
            parts.push(r);
        }
    }

    if event == "SessionStart" || event == "UserPromptSubmit" {
        if let Some(g) = goal_context(
            st,
            agent_id,
            event,
            p.get("session_id").and_then(|v| v.as_str()),
        ) {
            parts.push(g);
        }
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
                "additionalContext": parts.join("\n\n")
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOK: &str = "deadbeefdeadbeefdeadbeefdeadbeef";

    #[test]
    fn native_envelope_requires_token() {
        let ok = format!(r#"{{"agent_id":"a1","event":"Stop","payload":{{"x":1}},"token":"{TOK}"}}"#);
        let env = parse_envelope(&ok, TOK).expect("valid token accepted");
        assert_eq!(env.agent_id.as_deref(), Some("a1"));
        assert_eq!(env.event.as_deref(), Some("Stop"));
        let bad = r#"{"agent_id":"a1","event":"Stop","payload":{},"token":"nope"}"#;
        assert!(parse_envelope(bad, TOK).is_none());
        let missing = r#"{"agent_id":"a1","event":"Stop","payload":{}}"#;
        assert!(parse_envelope(missing, TOK).is_none());
        let non_string = format!(r#"{{"agent_id":"a1","event":"Stop","token":123}}"#);
        assert!(parse_envelope(&non_string, TOK).is_none());
    }

    #[test]
    fn raw_bridge_shape_requires_token_and_strips_it() {
        let ok = format!(
            r#"{{"hook_event_name":"PreToolUse","agent_id":"a2","tool_name":"Bash","token":"{TOK}"}}"#
        );
        let env = parse_envelope(&ok, TOK).expect("valid token accepted");
        assert_eq!(env.event.as_deref(), Some("PreToolUse"));
        assert_eq!(env.payload.get("tool_name").and_then(|v| v.as_str()), Some("Bash"));
        assert!(env.payload.get("token").is_none(), "secret must not flow into the payload");
        let bad = r#"{"hook_event_name":"PreToolUse","agent_id":"a2"}"#;
        assert!(parse_envelope(bad, TOK).is_none());
        assert!(parse_envelope("not json", TOK).is_none());
    }

    #[test]
    fn constant_time_eq_semantics() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn connection_slots_are_capped_and_released() {
        let mut held = Vec::new();
        while let Some(s) = try_acquire_slot() {
            held.push(s);
            assert!(held.len() <= MAX_CONNECTIONS);
        }
        assert_eq!(held.len(), MAX_CONNECTIONS);
        held.pop();
        assert!(try_acquire_slot().is_some(), "a released slot is reusable");
    }
}
