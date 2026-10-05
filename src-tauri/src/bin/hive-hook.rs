//! hive-hook — the lifecycle shim, replacing `bin/cth-hook.cjs` + the
//! `hive-node` ELECTRON_RUN_AS_NODE wrapper from the Electron version.
//!
//! A native binary, so hook commands need no runtime at all — which also fixes
//! the original's failure mode (hooks invoked via `sh -c`/`cmd` on a stripped
//! PATH where `node` is absent). Contract:
//!
//!   stdin  : the agent CLI's hook payload JSON (Claude-shaped: has
//!            hook_event_name, tool_name, session_id, stop_hook_active, …)
//!   argv[1]: `--hermes` puts the shim in translator mode for a Hermes
//!            shell-hook invocation (agent/shell_hooks.py): argv[2] is then
//!            the Claude-canonical event name (Hermes `hooks:` entries carry
//!            their own event names — the argv override maps them into our
//!            vocabulary) and the HookServer's Claude-shaped response is
//!            translated back to Hermes directives: a PreToolUse deny or a
//!            {continue:false} halt becomes {"decision":"block","reason"},
//!            additionalContext becomes {"context"}.
//!   argv[1]: optional event override (`PreToolUse`, `--status`, …); when
//!            absent the event is read from payload.hook_event_name
//!   env    : HIVE_SOCK (TCP addr `127.0.0.1:port`) — when unset the shim is a
//!            no-op, same as the agy-shim "not a hive worker → ignore" policy
//!   stdout : the HookServer's JSON response, printed verbatim — that is how
//!            `{decision}`/`{hookSpecificOutput}`/`{continue:false}` answers
//!            reach the CLI
//!
//! Framing: one JSON envelope line, one JSON response line, close. 5s internal
//! cap so a wedged shim can never freeze the agent (the codex-timeout note
//! from hive.ts: a 600s budget makes a hang indistinguishable from a freeze).

use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

fn main() {
    let args: Vec<String> = env::args().collect();
    // `--<provider> <CanonicalEvent>` — translator modes (Hermes/Gemini):
    // their `hooks:` entries carry provider event names; the argv override
    // supplies ours, and the response is re-shaped per provider contract.
    let mode = args.get(1).cloned().unwrap_or_default();
    let hermes_mode = mode == "--hermes";
    let gemini_mode = mode == "--gemini";
    let grok_mode = mode == "--grok";
    let agy_mode = mode == "--agy";
    let arg_event = if hermes_mode || gemini_mode || agy_mode {
        args.get(2).cloned().unwrap_or_default()
    } else if grok_mode {
        String::new() // event arrives via the normalized payload
    } else {
        args.get(1).cloned().unwrap_or_default()
    };

    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);

    let sock = env::var("HIVE_SOCK").unwrap_or_default();
    if sock.is_empty() {
        return; // not a hive worker — same early-exit as the upstream shims
    }

    let mut payload: serde_json::Value =
        serde_json::from_str(&input).unwrap_or(serde_json::json!({}));
    if grok_mode {
        normalize_grok(&mut payload);
    } else if agy_mode {
        normalize_agy(&mut payload);
    }
    if payload.get("agent_id").is_none() {
        if let Ok(id) = env::var("AGENT_ID") {
            payload["agent_id"] = serde_json::json!(id);
        }
    }

    let event = if arg_event == "--status" {
        "Status".to_string()
    } else if !arg_event.is_empty() {
        arg_event
    } else {
        payload
            .get("hook_event_name")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string()
    };

    let envelope = serde_json::json!({
        "agent_id": payload.get("agent_id"),
        "event": event,
        "payload": payload,
    });

    let timeout = Duration::from_secs(5);
    let Ok(addr) = sock.parse::<std::net::SocketAddr>() else {
        return;
    };
    let Ok(stream) = TcpStream::connect_timeout(&addr, timeout) else {
        return;
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let mut stream = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut line = serde_json::to_string(&envelope).unwrap_or_default();
    line.push('\n');
    if stream.write_all(line.as_bytes()).is_err() {
        return;
    }
    let _ = stream.shutdown(std::net::Shutdown::Write);

    let mut response = String::new();
    if BufReader::new(stream).read_line(&mut response).is_ok() {
        let trimmed = response.trim();
        if trimmed.is_empty() || trimmed == "{}" {
            return;
        }
        if hermes_mode {
            println!("{}", to_hermes(trimmed));
        } else if gemini_mode || grok_mode {
            println!("{}", to_gemini(trimmed));
        } else if agy_mode {
            // CRITICAL (AGY_HOOK_SHIM): agy treats ANY stdout object as a
            // decision and FAIL-CLOSES on a decision-less one — emit only a
            // real directive; silence means allow.
            let out = to_agy(trimmed);
            if !out.is_empty() {
                println!("{out}");
            }
        } else {
            println!("{trimmed}");
        }
    }
}

/// Claude-shaped HookServer answer → Hermes shell-hook directive. Hermes
/// reads `{"decision":"block"|"modify","reason"}` / `{"context":...}` /
/// `{"action":...}` — NOT `hookSpecificOutput`. Unknown content fails open.
fn to_hermes(resp: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(resp) else {
        return "{}".into();
    };
    // Operator deny at the tool boundary.
    if let Some(reason) = v
        .pointer("/hookSpecificOutput/permissionDecisionReason")
        .and_then(|r| r.as_str())
    {
        return serde_json::json!({"decision": "block", "reason": reason}).to_string();
    }
    // Operator HALT ({continue:false} in Claude-land).
    if v.get("continue").and_then(|c| c.as_bool()) == Some(false) {
        let reason = v
            .get("stopReason")
            .and_then(|r| r.as_str())
            .unwrap_or("Halted by the operator.");
        return serde_json::json!({"decision": "block", "reason": reason}).to_string();
    }
    // Injected context (steer / roster) rides Hermes' "context" channel.
    if let Some(ctx) = v
        .get("additionalContext")
        .or_else(|| v.pointer("/hookSpecificOutput/additionalContext"))
        .and_then(|c| c.as_str())
    {
        if !ctx.is_empty() {
            return serde_json::json!({"context": ctx}).to_string();
        }
    }
    // Already Hermes-shaped (decision/action keys pass through verbatim).
    if v.get("decision").is_some() || v.get("action").is_some() || v.get("context").is_some() {
        return resp.to_string();
    }
    "{}".into()
}

/// Grok's wire payload is camelCase with snake_case event values — normalize
/// to the Claude-shaped contract the HookServer owns (GROK_HOOK_SHIM's input
/// translation, minus the JS).
fn normalize_grok(p: &mut serde_json::Value) {
    let names = [
        ("pre_tool_use", "PreToolUse"),
        ("post_tool_use", "PostToolUse"),
        ("post_tool_use_failure", "PostToolUseFailure"),
        ("permission_denied", "PermissionDenied"),
        ("stop", "Stop"),
        ("stop_failure", "StopFailure"),
        ("session_start", "SessionStart"),
        ("session_end", "SessionEnd"),
        ("user_prompt_submit", "UserPromptSubmit"),
        ("notification", "Notification"),
        ("subagent_start", "SubagentStart"),
        ("subagent_stop", "SubagentStop"),
        ("pre_compact", "PreCompact"),
        ("post_compact", "PostCompact"),
    ];
    let raw = p
        .get("hookEventName")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let mapped = names
        .iter()
        .find(|(g, _)| *g == raw)
        .map(|(_, c)| c.to_string())
        .unwrap_or(if raw.is_empty() {
            "Unknown".into()
        } else {
            raw
        });
    if let Some(obj) = p.as_object_mut() {
        let mv = |o: &mut serde_json::Map<String, serde_json::Value>, from: &str, to: &str| {
            if let Some(v) = o.remove(from) {
                o.insert(to.to_string(), v);
            }
        };
        mv(obj, "hookEventName", "hook_event_name");
        obj["hook_event_name"] = serde_json::json!(mapped);
        mv(obj, "sessionId", "session_id");
        mv(obj, "toolName", "tool_name");
        mv(obj, "toolInput", "tool_input");
        mv(obj, "stopHookActive", "stop_hook_active");
        mv(obj, "notificationType", "notification_type");
        if !obj.contains_key("cwd") {
            if let Some(v) = obj.remove("workspaceRoot") {
                obj.insert("cwd".into(), v);
            }
        }
    }
}

/// Antigravity (`agy`) input normalization (AGY_HOOK_SHIM): its payload nests
/// tool context and renames fields; the event name arrives via argv.
fn normalize_agy(p: &mut serde_json::Value) {
    let Some(obj) = p.as_object_mut() else { return };
    let tc = obj.remove("toolCall").unwrap_or(serde_json::Value::Null);
    if let Some(v) = tc.get("name") {
        obj.insert("tool_name".into(), v.clone());
    }
    if let Some(v) = tc.get("args") {
        obj.insert("tool_input".into(), v.clone());
    }
    let mv = |o: &mut serde_json::Map<String, serde_json::Value>, from: &str, to: &str| {
        if let Some(v) = o.remove(from) {
            o.insert(to.to_string(), v);
        }
    };
    mv(obj, "conversationId", "session_id");
    mv(obj, "transcriptPath", "transcript_path");
    if let Some(serde_json::Value::Array(paths)) = obj.get("workspacePaths").cloned() {
        if let Some(first) = paths.into_iter().next() {
            obj.insert("cwd".into(), first);
        }
        obj.remove("workspacePaths");
    }
}

/// Claude-shaped answer → agy directive. EMPTY STRING unless a real
/// directive exists — agy fail-closes on any decision-less JSON.
fn to_agy(resp: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(resp) else {
        return String::new();
    };
    if v.get("decision").and_then(|d| d.as_str()) == Some("block") {
        let reason = v.get("reason").and_then(|r| r.as_str()).unwrap_or("");
        return serde_json::json!({
            "decision": "block", "reason": reason,
            "stopReason": reason, "systemMessage": reason
        })
        .to_string();
    }
    if v.get("continue").and_then(|c| c.as_bool()) == Some(false) {
        let reason = v.get("stopReason").and_then(|r| r.as_str()).unwrap_or("");
        return serde_json::json!({"decision": "block", "stopReason": reason}).to_string();
    }
    if let Some(reason) = v
        .pointer("/hookSpecificOutput/permissionDecisionReason")
        .and_then(|r| r.as_str())
    {
        return serde_json::json!({"decision": "deny", "reason": reason}).to_string();
    }
    if let Some(ctx) = v
        .get("additionalContext")
        .or_else(|| v.pointer("/hookSpecificOutput/additionalContext"))
        .and_then(|c| c.as_str())
    {
        if !ctx.is_empty() {
            return serde_json::json!({"systemMessage": ctx}).to_string();
        }
    }
    String::new()
}

/// Claude-shaped answer → Gemini CLI hook directive (GEMINI_HOOK_SHIM's
/// `done()`): deny→{"decision":"deny"}, {continue:false}→same shape,
/// additionalContext passes through under hookSpecificOutput.
fn to_gemini(resp: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(resp) else {
        return "{}".into();
    };
    if v.get("continue").and_then(|c| c.as_bool()) == Some(false) {
        return serde_json::json!({
            "continue": false,
            "stopReason": v.get("stopReason").and_then(|r| r.as_str()).unwrap_or("")
        })
        .to_string();
    }
    let deny_reason = v
        .pointer("/hookSpecificOutput/permissionDecisionReason")
        .and_then(|r| r.as_str())
        .or_else(|| {
            if v.get("decision").and_then(|d| d.as_str()) == Some("block") {
                v.get("reason").and_then(|r| r.as_str())
            } else {
                None
            }
        });
    if let Some(reason) = deny_reason {
        return serde_json::json!({"decision": "deny", "reason": reason}).to_string();
    }
    if let Some(ctx) = v
        .get("additionalContext")
        .or_else(|| v.pointer("/hookSpecificOutput/additionalContext"))
        .and_then(|c| c.as_str())
    {
        if !ctx.is_empty() {
            return serde_json::json!({
                "hookSpecificOutput": { "additionalContext": ctx }
            })
            .to_string();
        }
    }
    "{}".into()
}
