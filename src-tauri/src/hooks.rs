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

use crate::lock_ext::LockExt;
use crate::panic_guard;
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

/// Live state of the hook endpoint — the ONE socket every lifecycle hook,
/// proxy-bridge emit and cost sample travels through (upstream
/// HookSocketHealth). Written into fleet.json (`hooks`) by
/// `Hive::write_fleet_snapshot` so an operator or god can see a dead control
/// plane without a debugger. `orphans` is always 0 on this transport (it
/// counts abandoned unix-socket listeners).
#[derive(Clone, Default)]
struct HookHealth {
    path: Option<String>,
    listening: bool,
    since: Option<u64>,
    last_error: Option<String>,
    attempts: u64,
}

static HOOK_HEALTH: std::sync::Mutex<HookHealth> = std::sync::Mutex::new(HookHealth {
    path: None,
    listening: false,
    since: None,
    last_error: None,
    attempts: 0,
});

/// fleet.json's `hooks` field: `{path, listening, since, lastError, attempts,
/// orphans}`.
pub fn health() -> Value {
    let h = HOOK_HEALTH.lock_safe().clone();
    json!({
        "path": h.path,
        "listening": h.listening,
        "since": h.since,
        "lastError": h.last_error,
        "attempts": h.attempts,
        "orphans": 0,
    })
}

/// The `log.jsonl` entry for a bind outcome (pure, so it is testable without
/// the process-global health record).
fn bind_outcome_entry(h: &HookHealth) -> Value {
    if h.listening {
        let mut e = json!({ "kind": "hooks", "state": "listening", "path": h.path });
        // Bound only after earlier attempts failed: say so (the retry worked).
        if h.attempts > 0 {
            e["retries"] = json!(h.attempts);
        }
        e
    } else {
        json!({
            "kind": "hooks", "state": "bind-failed", "path": h.path,
            "code": h.last_error, "attempt": h.attempts
        })
    }
}

/// Record the bind outcome in the hive's own log.jsonl (upstream logs both
/// `listening` and `bind-failed` there — a packaged app has no console).
/// Called once the hive exists; the bind itself happens before AppState. A
/// failed bind leaves its record in HOOK_HEALTH (attempts + last error), so a
/// hive that becomes available later still gets its `hooks bind-failed` line.
pub fn log_bind_outcome(hive: &crate::hive::Hive) {
    let h = HOOK_HEALTH.lock_safe().clone();
    if h.listening {
        log::info!("[hive] hook server listening on {}", h.path.as_deref().unwrap_or("?"));
    } else {
        log::error!("[hive] hook server bind FAILED: every agent hook is allowed until this recovers");
    }
    let _ = hive.log(&bind_outcome_entry(&h));
}

/// Bind attempts before giving up, and the pause before each retry (a loopback
/// bind only fails transiently — a port-exhaustion blip, a security product
/// holding the stack for a moment — so a short backoff is enough).
const BIND_ATTEMPTS: u32 = 3;
const BIND_BACKOFF: [Duration; 2] = [Duration::from_millis(150), Duration::from_millis(500)];

/// Run `bind` up to `attempts` times, sleeping `backoff[i]` before retry `i+1`.
/// `on_fail(n, err)` records every failure; the final error is returned.
fn bind_with_retry<T>(
    mut bind: impl FnMut() -> std::io::Result<T>,
    attempts: u32,
    backoff: &[Duration],
    mut on_fail: impl FnMut(u32, &std::io::Error),
    sleep: impl Fn(Duration),
) -> std::io::Result<T> {
    let mut n = 0u32;
    loop {
        n += 1;
        match bind() {
            Ok(v) => return Ok(v),
            Err(e) => {
                on_fail(n, &e);
                if n >= attempts {
                    return Err(std::io::Error::new(
                        e.kind(),
                        format!("{e} (after {n} attempt{})", if n == 1 { "" } else { "s" }),
                    ));
                }
                sleep(backoff.get((n - 1) as usize).copied().unwrap_or_default());
            }
        }
    }
}

impl HookServer {
    pub fn start(app: &AppHandle) -> std::io::Result<Self> {
        let bound = bind_with_retry(
            || TcpListener::bind("127.0.0.1:0").and_then(|l| Ok((l.local_addr()?.to_string(), l))),
            BIND_ATTEMPTS,
            &BIND_BACKOFF,
            |n, e| {
                {
                let mut h = HOOK_HEALTH.lock_safe();
                    h.listening = false;
                    h.attempts = n as u64;
                    h.last_error = Some(e.kind().to_string());
                }
                log::warn!("[hive] hook server bind attempt {n}/{BIND_ATTEMPTS} failed: {e}");
            },
            std::thread::sleep,
        );
        // Still failing: the caller shows the startup dialog; the record stays
        // in HOOK_HEALTH for `log_bind_outcome`.
        let (addr, listener) = bound?;
        {
            let mut h = HOOK_HEALTH.lock_safe();
            // `attempts` keeps the count of failures that preceded this success.
            let retries = h.attempts;
            *h = HookHealth {
                path: Some(addr.clone()),
                listening: true,
                since: Some(chrono::Utc::now().timestamp_millis() as u64),
                last_error: None,
                attempts: retries,
            };
        }
        let handle = app.clone();
        thread::spawn(move || {
            for conn in listener.incoming() {
                panic_guard::guarded("hook-server-accept", || {
                    let Ok(stream) = conn else { return };
                    // At capacity → drop (close) the connection without a thread.
                    let Some(slot) = try_acquire_slot() else {
                        return;
                    };
                    let app = handle.clone();
                    thread::spawn(move || {
                        let _slot = slot;
                        serve_conn(app, stream)
                    });
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

/// Port of shared/hookEvents.ts `validateHookEvent`: `event` is a non-empty
/// string; `agentId`, when present, a non-empty string; `tool` /
/// `notificationType` / `source` / `message`, when present, strings; `blocked`,
/// when present, a bool. Provider event names are NOT restricted.
pub(crate) fn validate_hook_event(v: &Value) -> bool {
    let Some(o) = v.as_object() else {
        return false;
    };
    if !o.get("event").and_then(|e| e.as_str()).is_some_and(|s| !s.is_empty()) {
        return false;
    }
    if let Some(a) = o.get("agentId") {
        if !a.as_str().is_some_and(|s| !s.is_empty()) {
            return false;
        }
    }
    for f in ["tool", "notificationType", "source", "message"] {
        if o.get(f).is_some_and(|x| !x.is_string()) {
            return false;
        }
    }
    o.get("blocked").is_none_or(|b| b.is_boolean())
}

fn emit_status(app: &AppHandle, id: &str, status: &str, detail: Option<&str>) {
    if let Some(st) = app.try_state::<AppState>() {
        let _ = st.hive.set_status(id, status);
        // The parked-compact drain: mission/context-trigger fires while the
        // agent was mid-work land here at the idle boundary (the same slot
        // upstream's renderer-side queue drained through). Provider verb
        // table: no trustworthy command → leave the terminal alone.
        if status == "idle" && st.compact_queue.lock_safe().remove(id) {
            let provider = st
                .hive
                .registry()
                .iter()
                .find(|a| a.id == id)
                .map(|a| a.provider.clone())
                .unwrap_or_default();
            let focus = st
                .config
                .lock_safe()
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
    // The payload is validated before it crosses the IPC boundary
    // (shared/hookEvents.ts validateHookEvent); a malformed one is dropped
    // from the feed — hook handling below carries on regardless.
    let mut feed = serde_json::Map::new();
    feed.insert("event".into(), json!(event));
    for (key, field) in [
        ("agentId", None),
        ("tool", Some("tool_name")),
        ("notificationType", Some("notification_type")),
        ("source", Some("source")),
        ("message", Some("message")),
    ] {
        let v = match field {
            None => (!agent_id.is_empty()).then(|| json!(agent_id)),
            Some(f) => p.get(f).filter(|v| !v.is_null()).cloned(),
        };
        if let Some(v) = v {
            feed.insert(key.into(), v);
        }
    }
    feed.insert("blocked".into(), json!(false));
    let feed = Value::Object(feed);
    if validate_hook_event(&feed) {
        let _ = app.emit("hive:hookEvent", feed);
    } else {
        log::warn!("[hive] rejected invalid hook event: {event}");
    }

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
                    .lock_safe()
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
    if !agent_id.is_empty() && st.control.lock_safe().should_halt(&agent_id) {
        return json!({ "continue": false, "stopReason": "Halted by the operator from the floor." });
    }

    // Breaker inputs + the wake watchdog's hook channel (its only turn-proof
    // evidence for engines without telemetry).
    let now_ms = chrono::Utc::now().timestamp_millis() as u64;
    st.wake.lock_safe().note_hook(
        &agent_id,
        &event,
        p.get("message").and_then(|v| v.as_str()).unwrap_or(""),
        now_ms,
    );
    match event.as_str() {
        "PostToolUse" => st.breaker.lock_safe().record_tool_use(
            &agent_id,
            p.get("tool_name").and_then(|v| v.as_str()),
            p.get("tool_input").unwrap_or(&Value::Null),
            now_ms,
        ),
        "UserPromptSubmit" => st
            .breaker
            .lock_safe()
            .record_user_prompt(&agent_id, now_ms),
        // Hermes api_request_error (translated via the shim's --hermes mode)
        // — the breaker's error-storm feed for engines without OTel.
        "ApiError" => st.breaker.lock_safe().record_error(&agent_id),
        "PreCompact" => st
            .breaker
            .lock_safe()
            .record_compact_start(&agent_id, now_ms),
        "PostCompact" | "SessionStart" => st
            .breaker
            .lock_safe()
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
            let deny = st.control.lock_safe().tool_decision(
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
    if !st.config.lock_safe().notifications {
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
    let mut delivered_map = st.delivered_goal.lock_safe();
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
            let breaker = serde_json::to_value(st.breaker.lock_safe().level_for(&a.id))
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
        let ctx_map = st.context_usage.lock_safe().clone();
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
        if let Some(note) = st.control.lock_safe().take_steer(agent_id) {
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
    fn bind_retries_with_backoff_then_succeeds() {
        use std::cell::RefCell;
        let calls = RefCell::new(0u32);
        let sleeps = RefCell::new(Vec::new());
        let fails = RefCell::new(Vec::new());
        let r = bind_with_retry(
            || {
                *calls.borrow_mut() += 1;
                if *calls.borrow() < 3 {
                    Err(std::io::Error::new(std::io::ErrorKind::AddrInUse, "busy"))
                } else {
                    Ok("bound")
                }
            },
            BIND_ATTEMPTS,
            &BIND_BACKOFF,
            |n, e| fails.borrow_mut().push((n, e.kind())),
            |d| sleeps.borrow_mut().push(d),
        );
        assert_eq!(r.unwrap(), "bound");
        assert_eq!(*calls.borrow(), 3);
        assert_eq!(*sleeps.borrow(), BIND_BACKOFF.to_vec());
        assert_eq!(fails.borrow().len(), 2);
    }

    #[test]
    fn bind_gives_up_after_three_attempts_and_records_each() {
        use std::cell::RefCell;
        let calls = RefCell::new(0u32);
        let last = RefCell::new(0u32);
        let r: std::io::Result<()> = bind_with_retry(
            || {
                *calls.borrow_mut() += 1;
                Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "nope"))
            },
            BIND_ATTEMPTS,
            &BIND_BACKOFF,
            |n, _| *last.borrow_mut() = n,
            |_| {},
        );
        let e = r.unwrap_err();
        assert_eq!(*calls.borrow(), 3);
        assert_eq!(*last.borrow(), 3);
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(e.to_string().contains("after 3 attempts"), "{e}");
        // the first try succeeding never sleeps or records a failure
        let r = bind_with_retry(|| Ok(1), 3, &BIND_BACKOFF, |_, _| panic!("no failure"), |_| panic!("no sleep"));
        assert_eq!(r.unwrap(), 1);
    }

    #[test]
    fn bind_outcome_entry_keeps_the_failure_record_for_the_hive_log() {
        let failed = HookHealth {
            path: None,
            listening: false,
            since: None,
            last_error: Some("address in use".into()),
            attempts: 3,
        };
        let e = bind_outcome_entry(&failed);
        assert_eq!(e["kind"], "hooks");
        assert_eq!(e["state"], "bind-failed");
        assert_eq!(e["code"], "address in use");
        assert_eq!(e["attempt"], 3);
        let ok = HookHealth {
            path: Some("127.0.0.1:1".into()),
            listening: true,
            since: Some(1),
            last_error: None,
            attempts: 0,
        };
        let e = bind_outcome_entry(&ok);
        assert_eq!(e["state"], "listening");
        assert!(e.get("retries").is_none());
        let recovered = HookHealth { attempts: 2, ..ok };
        assert_eq!(bind_outcome_entry(&recovered)["retries"], 2);
    }

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
