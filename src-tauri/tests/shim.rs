//! Shim contract tests — the real `hive-hook` binary against a fake Hive
//! socket, asserting the wire contract both ways: the envelope the shim
//! writes, and the provider-shaped response it prints back to the agent CLI.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_hive-hook");

/// A fake HookServer: accept one connection, read the envelope line, hand it
/// to the test, reply with `response`, return what was received.
fn serve_once(response: &str) -> (std::thread::JoinHandle<serde_json::Value>, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let resp = response.to_string();
    let h = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(s.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        s.write_all(resp.as_bytes()).unwrap();
        s.write_all(b"\n").unwrap();
        serde_json::from_str(&line).unwrap()
    });
    (h, addr)
}

fn run_shim(args: &[&str], env: &[(&str, &str)], stdin_data: &str) -> String {
    let mut cmd = Command::new(BIN);
    cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped());
    // Guard against a stray HIVE_SOCK/AGENT_ID in the parent env (full
    // env_clear() on Windows would strip SystemRoot and break spawn).
    cmd.env_remove("HIVE_SOCK").env_remove("AGENT_ID");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin_data.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn claude_mode_envelope_and_verbatim_response() {
    let (h, addr) =
        serve_once(r#"{"hookSpecificOutput":{"permissionDecisionReason":"operator denied"}}"#);
    let out = run_shim(
        &[],
        &[("HIVE_SOCK", &addr), ("AGENT_ID", "w1")],
        r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","session_id":"s1"}"#,
    );
    let env = h.join().unwrap();
    assert_eq!(env["event"], "PreToolUse");
    assert_eq!(env["agent_id"], "w1");
    assert_eq!(env["payload"]["tool_name"], "Bash");
    assert_eq!(
        out.trim(),
        r#"{"hookSpecificOutput":{"permissionDecisionReason":"operator denied"}}"#
    );
}

#[test]
fn hermes_mode_maps_event_and_translates_deny() {
    let (h, addr) =
        serve_once(r#"{"hookSpecificOutput":{"permissionDecisionReason":"operator denied"}}"#);
    let out = run_shim(
        &["--hermes", "PreToolUse"],
        &[("HIVE_SOCK", &addr), ("AGENT_ID", "h1")],
        r#"{"hook_event_name":"pre_tool_call","tool_name":"terminal","session_id":"s"}"#,
    );
    let env = h.join().unwrap();
    // The argv override supplies the canonical event (hermes names differ).
    assert_eq!(env["event"], "PreToolUse");
    // The denial translates to hermes' {"decision":"block"} shape.
    assert_eq!(
        out.trim(),
        r#"{"decision":"block","reason":"operator denied"}"#
    );
}

#[test]
fn hermes_context_rides_context_channel() {
    let (h, addr) = serve_once(r#"{"additionalContext":"steer text"}"#);
    let out = run_shim(
        &["--hermes", "Stop"],
        &[("HIVE_SOCK", &addr)],
        r#"{"hook_event_name":"agent_loop_stopped"}"#,
    );
    h.join().unwrap();
    assert_eq!(out.trim(), r#"{"context":"steer text"}"#);
}

#[test]
fn agy_emits_nothing_on_empty_decision() {
    // Fail-closed boundary: agy treats ANY stdout object as a decision, so
    // silence means allow — never emit "{}".
    let (h, addr) = serve_once("{}");
    let out = run_shim(
        &["--agy", "Stop"],
        &[("HIVE_SOCK", &addr)],
        r#"{"hook_event_name":"stop"}"#,
    );
    h.join().unwrap();
    assert_eq!(out.trim(), "");
}

#[test]
fn grok_camelcase_normalizes_to_canonical_event() {
    let (h, addr) = serve_once("{}");
    let _ = run_shim(
        &["--grok"],
        &[("HIVE_SOCK", &addr)],
        r#"{"hookEventName":"pre_tool_use","toolName":"Bash"}"#,
    );
    let env = h.join().unwrap();
    assert_eq!(env["event"], "PreToolUse");
}

#[test]
fn no_hive_sock_is_a_noop() {
    // "Not a hive worker → ignore" — the same early-exit every upstream shim
    // applies; no socket, no output, clean exit.
    let out = run_shim(&[], &[], "{}");
    assert_eq!(out, "");
}

#[test]
fn unreachable_socket_fails_open() {
    // A dead harness must never freeze the agent: shim exits quietly.
    let out = run_shim(&[], &[("HIVE_SOCK", "127.0.0.1:1")], "{}");
    assert_eq!(out, "");
}

#[test]
fn stdin_without_event_falls_to_payload_name() {
    let (h, addr) = serve_once("{}");
    let _ = run_shim(
        &[],
        &[("HIVE_SOCK", &addr)],
        r#"{"hook_event_name":"SubagentStop"}"#,
    );
    let env = h.join().unwrap();
    assert_eq!(env["event"], "SubagentStop");
}
