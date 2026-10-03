//! hive-hook — the lifecycle shim, replacing `bin/cth-hook.cjs` + the
//! `hive-node` ELECTRON_RUN_AS_NODE wrapper from the Electron version.
//!
//! A native binary, so hook commands need no runtime at all — which also fixes
//! the original's failure mode (hooks invoked via `sh -c`/`cmd` on a stripped
//! PATH where `node` is absent). Contract:
//!
//!   stdin  : the agent CLI's hook payload JSON (Claude-shaped: has
//!            hook_event_name, tool_name, session_id, stop_hook_active, …)
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
    let arg_event = args.get(1).cloned().unwrap_or_default();

    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);

    let sock = env::var("HIVE_SOCK").unwrap_or_default();
    if sock.is_empty() {
        return; // not a hive worker — same early-exit as the upstream shims
    }

    let mut payload: serde_json::Value =
        serde_json::from_str(&input).unwrap_or(serde_json::json!({}));
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
        if !trimmed.is_empty() && trimmed != "{}" {
            println!("{trimmed}");
        }
    }
}
