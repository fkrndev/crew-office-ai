//! Parity suite — observable-behavior tests that pin the Tauri port to the
//! original Electron app's contracts. Each test names the upstream file +
//! invariant it covers. Where a feature is architecture-bound (Monaco,
//! Pixi, OS keychain, GitHub releases) the contract is pinned on the ported
//! half that actually moved.
//!
//! Run with the whole suite: `cargo test`.

use std::path::{Path, PathBuf};

use agent_office_lib::hive::{Hive, Message, RegistryEntry};
use agent_office_lib::pricing;
use agent_office_lib::skills;
use agent_office_lib::trigger_history::{classify_inbound_kind, TriggerHistory, TriggerHistoryEntry};
use serde_json::{json, Value};

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("oa-parity-{tag}-{}", uuid()));
    std::fs::create_dir_all(&d).unwrap();
    d
}
fn uuid() -> String {
    // avoid pulling the crate in tests; a coarse unique suffix is enough
    format!(
        "{:x}-{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        std::process::id()
    )
}

fn hive_at(dir: &Path) -> Hive {
    Hive::ensure(&dir.join("hive")).unwrap()
}

fn reg(hive: &Hive, id: &str, god: bool, dir: &Path) {
    hive.register_agent(&RegistryEntry {
        id: id.into(),
        name: id.into(),
        provider: "claude".into(),
        role: "x".into(),
        cwd: dir.join(id).display().to_string(),
        is_god: god,
        status: "idle".into(),
        session_id: None,
        command: String::new(),
        created_at: chrono_now(),
    })
    .unwrap();
}
fn chrono_now() -> String {
    // RFC3339-ish stamp; hive only needs a string.
    "2026-01-01T00:00:00Z".into()
}

/* ── hive.ts: the file protocol ───────────────────────────────────────────── */

#[test]
fn hive_init_seeds_protocol_files() {
    let d = tmp("init");
    let _ = hive_at(&d);
    for f in ["PROTOCOL.md", "board.md", "tasks.json", "registry.json", "log.jsonl"] {
        assert!(d.join("hive").join(f).exists(), "missing {f}");
    }
    // init commits — the hive repo exists (single-committer rule).
    assert!(d.join("hive").join(".git").exists());
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn register_creates_agent_workspace_and_registry_entry() {
    let d = tmp("reg");
    let h = hive_at(&d);
    reg(&h, "w1", false, &d);
    let a = d.join("hive/agents/w1");
    for f in ["identity.md", "memory.md", "cursor.json"] {
        assert!(a.join(f).exists(), "missing {f}");
    }
    for sub in ["inbox", "inbox/.done", "outbox"] {
        assert!(a.join(sub).is_dir(), "missing {sub}");
    }
    assert_eq!(h.registry().len(), 1);
    assert_eq!(h.registry()[0].id, "w1");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn routing_contract_god_human_broadcast_bounce() {
    let d = tmp("route");
    let h = hive_at(&d);
    reg(&h, "god", true, &d);
    reg(&h, "w1", false, &d);
    reg(&h, "w2", false, &d);

    // 'human' routes through god (spend/destructive/scope escalation path).
    h.send(&Message::new("w1", "human", "request", "s", "b")).unwrap();
    // broadcast → everyone but the sender.
    h.send(&Message::new("god", "broadcast", "inform", "s", "b")).unwrap();
    // unknown id → bounces to god (alive god ⇒ not back to sender).
    h.send(&Message::new("w2", "nonexistent", "request", "s", "b")).unwrap();

    let ds = h.router_tick().unwrap();
    let god_in = h.inbox("god").unwrap();
    assert!(god_in.iter().any(|m| m.from == "w1")); // human → god
    assert!(god_in.iter().any(|m| m.subject.starts_with("[bounced]"))); // unknown → bounced to god
    assert_eq!(h.inbox("w1").unwrap().len(), 1); // broadcast skip sender=god
    assert_eq!(h.inbox("w2").unwrap().len(), 1);
    assert!(ds.len() >= 3);
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn malformed_outbox_message_quarantines_not_wedges() {
    let d = tmp("quar");
    let h = hive_at(&d);
    reg(&h, "w1", false, &d);
    // Garbage in the outbox must not jam the router — upstream policy:
    // quarantine into inbox/.done so the file can't re-trip every tick.
    let bad = d.join("hive/agents/w1/outbox/bad.json");
    std::fs::write(&bad, "not json at all").unwrap();
    let ds = h.router_tick().unwrap();
    assert!(ds.is_empty());
    assert!(!bad.exists());
    assert!(d.join("hive/agents/w1/inbox/.done/bad.json").exists());
    // And a valid message afterwards still routes.
    h.send(&Message::new("w1", "w1", "inform", "s", "b")).unwrap();
    assert_eq!(h.router_tick().unwrap().len(), 1);
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn status_and_cost_sample_persist() {
    let d = tmp("status");
    let h = hive_at(&d);
    reg(&h, "w1", false, &d);
    h.set_status("w1", "working").unwrap();
    assert_eq!(h.registry()[0].status, "working");
    // append_cost_sample → the proxy/hook cost ledger line lands.
    // The #56 dedup gate: only sessioned samples hit the ledger.
    h.append_cost_sample("w1", &json!({
        "session_id": "s1", "model": "claude-sonnet-4",
        "input": 100, "output": 50
    }))
    .unwrap();
    let sample = h.usage_sample("w1");
    assert!(sample.is_some());
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn record_session_survives_for_resume() {
    let d = tmp("sess");
    let h = hive_at(&d);
    reg(&h, "w1", false, &d);
    h.record_session("w1", "sess-42").unwrap();
    assert_eq!(h.registry()[0].session_id.as_deref(), Some("sess-42"));
    // A second identical write is a no-op (dedup for cost samples).
    h.record_session("w1", "sess-42").unwrap();
    assert_eq!(h.registry()[0].session_id.as_deref(), Some("sess-42"));
    std::fs::remove_dir_all(&d).ok();
}

/* ── taskLedger.ts: patch semantics ───────────────────────────────────────── */

#[test]
fn task_patch_preserves_unknown_fields_and_clears_with_null() {
    let d = tmp("taskp");
    let h = hive_at(&d);
    std::fs::write(
        d.join("hive/tasks.json"),
        r#"{"tasks":[{"id":"t1","status":"review","scope":"api","dependsOn":["t0"]},
                    {"id":"t2","status":"blocked","humanQA":[{"q":"ok?"}]}]}"#,
    )
    .unwrap();
    let mut p = serde_json::Map::new();
    p.insert("status".into(), Value::String("done".into()));
    let c1 = h.update_task("t1", &p).unwrap();
    assert_eq!(c1["status"], "done");
    assert_eq!(c1["scope"], "api"); // unknown fields survive
    // Answering t2's ask keeps t1's card byte-identical in spirit.
    let c2 = h.task_answer("t2", "yes").unwrap();
    assert_eq!(c2["humanQA"][0]["a"], "yes");
    assert_eq!(c2["status"], "in-progress");
    let doc = h.read_tasks().unwrap();
    assert_eq!(doc["tasks"][0]["dependsOn"], json!(["t0"]));
    std::fs::remove_dir_all(&d).ok();
}

/* ── triggerHistory.ts + triggers.ts ──────────────────────────────────────── */

#[test]
fn trigger_ledger_full_cycle_pending_to_verdict() {
    let d = tmp("trig");
    let th = TriggerHistory::new(d.clone());
    let mk = |decision: Option<&str>| TriggerHistoryEntry {
        id: String::new(),
        source: "webhook".into(),
        source_id: "ep".into(),
        source_name: "hook".into(),
        direction: "inbound".into(),
        peer: "sender".into(),
        title: None,
        body: "deploy".into(),
        kind: classify_inbound_kind("deploy the release").into(),
        decision: decision.map(|d| d.into()),
        correlation_id: None,
        task_id: None,
        at: 0,
    };
    let e = th.append(mk(Some("pending")));
    assert!(!e.id.is_empty() && e.at > 0, "id+at stamped");
    assert_eq!(e.kind, "directive");
    let out = th
        .update(&e.id, Some("approved".into()), Some("t-9".into()), None, None)
        .unwrap();
    assert_eq!(out.decision.as_deref(), Some("approved"));
    assert_eq!(th.list().len(), 1);
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn ledger_survives_corrupt_file() {
    let d = tmp("trigc");
    let th = TriggerHistory::new(d.clone());
    std::fs::write(d.join("trigger-history.json"), "[[[not json").unwrap();
    assert!(th.list().is_empty()); // degrades to empty, never throws
    // Partially-corrupt rows drop individually.
    std::fs::write(
        d.join("trigger-history.json"),
        r#"[{"id":"ok","source":"webhook","direction":"inbound","body":"x","kind":"directive","at":1},
            {"bad":true}]"#,
    )
    .unwrap();
    let all = th.list();
    assert_eq!(all.len(), 1);
    std::fs::remove_dir_all(&d).ok();
}

/* ── pricing.ts ───────────────────────────────────────────────────────────── */

#[test]
fn pricing_golden_costs_and_fallback() {
    // Golden numbers from pricing.ts (per-million-token rates).
    let sonnet = pricing::price_for("claude-sonnet-4-5");
    assert!((sonnet.input_per_m - 3.0).abs() < 1e-9 && (sonnet.output_per_m - 15.0).abs() < 1e-9);
    let opus = pricing::price_for("claude-opus-4-1");
    assert!((opus.input_per_m - 15.0).abs() < 1e-9 && (opus.cache_write_per_m - 18.75).abs() < 1e-9);
    let haiku = pricing::price_for("claude-haiku-4-5");
    assert!((haiku.input_per_m - 0.8).abs() < 1e-9);
    // Unknown models default to Sonnet; bracket suffixes normalize away.
    let unk = pricing::price_for("totally-unknown-model[1m]");
    assert_eq!(unk.input_per_m, sonnet.input_per_m);
    let suff = pricing::price_for("claude-sonnet-4-5[1m]");
    assert_eq!(suff.input_per_m, sonnet.input_per_m);
}

/* ── skills.ts pure logic ─────────────────────────────────────────────────── */

#[test]
fn skill_frontmatter_and_safe_name_contracts() {
    let (n, d) = skills::parse_skill_frontmatter(
        "---\nname: api-tools\ndescription: |\n  multi\n  line\n---\n",
    );
    assert_eq!(n.as_deref(), Some("api-tools"));
    assert_eq!(d.as_deref(), Some("multi line"));
    // Traversal can't name a folder; only the basename is ever used.
    assert!(skills::safe_skill_dir_name("../../../etc").is_some());
    assert!(skills::safe_skill_dir_name("..").is_none());
}

/* ── The remaining upstream surface — explicit N/A coverage note ────────────
 *
 * Not exercised here (architecture-bound; see PORTING.md "not applicable"):
 *   - Monaco/CodeEditor, Pixi OfficeFloor, multi-window — renderer-only.
 *   - OS keychain secrets — real credential store; command surface exists.
 *   - updater.ts (GitHub releases) — env-dependent; tauri-updater replaces it.
 *   - slack.ts/realtime.ts — third-party SaaS bridges; org triggers are the
 *     ledger'd half that moved.
 *   - nodeInstall/cliInstall — Electron had to bootstrap Node; Tauri ships
 *     a native binary (the porting win).
 */
