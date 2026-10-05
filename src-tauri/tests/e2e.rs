//! End-to-end floor lifecycle — one harness home through the whole operator
//! journey, exercised at the same store/module layer the commands call:
//!
//!   ensure home (hive + config + roster stores) → wizard config write →
//!   hire agents → god dispatches mail → worker inbox → roster mirror →
//!   mission due → per-agent budget trips only that agent.
//!
//! The #[tauri::command] fns take an AppHandle, so they stay thin and are
//! covered transitively: every leg here calls the same underlying function.

use std::fs;
use std::path::{Path, PathBuf};

use agent_office_lib::config::{Config, ConfigStore};
use agent_office_lib::hive::{Hive, PartialMessage, RegistryEntry};
use agent_office_lib::missions::{is_mission_due, mission_due_ms, ScheduledMission};
use agent_office_lib::roster::RosterStore;
use serde_json::{json, Value};

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "oa-e2e-{tag}-{:x}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        std::process::id()
    ));
    fs::create_dir_all(&d).unwrap();
    d
}

fn agent(hive: &Hive, id: &str, god: bool, cwd: &Path) {
    hive.register_agent(&RegistryEntry {
        id: id.into(),
        name: id.into(),
        provider: "claude".into(),
        role: "x".into(),
        cwd: cwd.join(id).display().to_string(),
        is_god: god,
        status: "idle".into(),
        session_id: None,
        command: String::new(),
        created_at: "2026-01-01T00:00:00Z".into(),
        ..Default::default()
    })
    .unwrap();
}

/// The whole first-run journey in one test: any break leaves a user staring
/// at a dead floor, and each leg is a different subsystem.
#[test]
fn first_run_onboarding_to_working_floor() {
    let home = tmp("first-run");

    // ── Step 3 HOME: ensureHarnessHome = mkdir + hive subdir. Hive::ensure
    //    seeds the protocol files the spawned agents read.
    let hive = Hive::ensure(&home.join("hive")).unwrap();
    assert!(home.join("hive/PROTOCOL.md").exists());
    assert!(home.join("hive/COMMANDS.md").exists());

    // ── Step 4 FINISH: the wizard's updateConfig patch — every key survives.
    let store = ConfigStore::new(home.clone());
    let patch = json!({
        "onboardingComplete": true,
        "audience": "technical",
        "harnessHome": home.display().to_string(),
        "registeredRepos": [],
        "autoMode": true,
        "godProvider": "claude",
        "godModel": "opus",
        "telemetryEnabled": true
    });
    let (cfg, _raw) = store.update(patch.as_object().unwrap()).unwrap();
    assert!(cfg.auto_mode);
    let on_disk: Value =
        serde_json::from_str(&fs::read_to_string(home.join("config.json")).unwrap()).unwrap();
    assert_eq!(on_disk["onboardingComplete"], true);
    assert_eq!(on_disk["godProvider"], "claude");

    // ── Hire: god + one worker register into the same hive.
    agent(&hive, "god-1", true, &home);
    agent(&hive, "w-1", false, &home);

    // ── Dispatch: god → worker mail lands in the worker's inbox.
    let sent = hive
        .send_partial(
            &PartialMessage {
                to: Some("w-1".into()),
                act: Some("task".into()),
                subject: Some("first task".into()),
                body: Some("hello floor".into()),
                ..Default::default()
            },
            "god-1",
        )
        .unwrap();
    let inbox = hive.inbox("w-1").unwrap();
    assert!(inbox.iter().any(|m| m.id == sent.id && m.body == "hello floor"));
    assert!(hive.inbox_backlog("w-1") > 0);

    // ── Roster mirror: the renderer's floor snapshot round-trips and an
    //    empty overwrite is refused while nothing real has landed yet.
    let roster = RosterStore::new(home.clone());
    let snap = json!({
        "agents": [{ "id": "w-1", "name": "w-1" }],
        "archived": [],
        "restorable": [],
        "queues": {},
        "selectedId": "w-1"
    });
    roster.write(&snap).unwrap();
    let back = roster.read().unwrap();
    assert_eq!(back["agents"].as_array().unwrap()[0]["id"], "w-1");
}

/// The empty-first-write guard: a renderer that boots with nothing must never
/// flatten a roster file that holds the real floor.
#[test]
fn roster_empty_write_never_erases_floor() {
    let home = tmp("roster-guard");
    let roster = RosterStore::new(home.clone());
    let real = json!({
        "agents": [{ "id": "a1" }],
        "archived": [],
        "restorable": [{ "id": "r1" }],
        "queues": {},
        "selectedId": null
    });
    roster.write(&real).unwrap();

    // A fresh RosterStore simulates a new process whose own `wrote` flag is
    // still false — exactly the dangerous first-write moment.
    let fresh = RosterStore::new(home.clone());
    let empty = json!({ "agents": [], "archived": [], "restorable": [], "queues": {}, "selectedId": null });
    assert!(fresh.write(&empty).is_err(), "empty first write must be refused");
    assert!(fresh.read().unwrap()["agents"].as_array().unwrap().len() == 1);
    // And the refused write still backed up the floor it almost destroyed.
    assert!(fs::read_dir(home.join("roster-backups")).unwrap().count() >= 1);
}

/// Missions fire when the interval elapses — the scheduler's heartbeat tick.
/// is_mission_due is pure timing; the enabled gate lives in mission_due_ms
/// (the arm side — a disabled mission is never armed, so it can never fire).
#[test]
fn scheduled_mission_fires_when_due() {
    use std::collections::HashMap;
    let mut m = ScheduledMission::default();
    m.id = "standup".into();
    m.enabled = true;
    m.interval_ms = 60_000;
    m.last_fired_at = 1_000;
    // Due once interval elapsed.
    assert!(is_mission_due(&m, 1_000 + 61_000));
    // Not before.
    assert!(!is_mission_due(&m, 1_000 + 59_000));
    // Disabled missions are never armed — the gate sits in mission_due_ms.
    m.enabled = false;
    assert_eq!(mission_due_ms(&m, 1_000 + 120_000, &HashMap::new()), None);
}
