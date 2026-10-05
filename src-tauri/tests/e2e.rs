//! End-to-end floor lifecycle — one harness home through the whole operator
//! journey.
//!
//! Two layers, matching how Tauri apps are meant to be tested (official
//! guidance: thin commands, testable cores — the `tauri::test` MockRuntime
//! path would need every command generic over `R: Runtime` for no extra
//! behavioural coverage):
//!
//!   - module/store level: ConfigStore::update, Hive, RosterStore, missions —
//!     the functions the commands delegate to;
//!   - AppState level: `commands::AppState::new(harness_home)` is the REAL
//!     state object every `#[tauri::command]` resolves via `state(&app)` —
//!     constructing it exercises boot, hive seeding, hook staging, DB open,
//!     and every store exactly as the app does.
//!
//! The IPC boundary itself stays untested by design: the command fns are
//! 1–5 line delegations, and a true renderer↔backend pass is what the
//! optional WebDriver suite (@wdio/tauri-service) is for, if we ever want it.

use std::fs;
use std::path::{Path, PathBuf};

use agent_office_lib::commands::AppState;
use agent_office_lib::config::ConfigStore;
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

/// The REAL state object — `state(&app)` in every command resolves to this.
/// Booting it on a fresh home exercises the entire init path the app takes:
/// hive seeding, config load + mission seeding, hook-bin staging, DB open.
/// Then the whole journey runs against the same fields commands touch.
#[test]
fn app_state_boots_and_serves_the_journey() {
    let home = tmp("appstate");
    let st = AppState::new(home.clone()).unwrap();

    // Boot side-effects a command-level test would otherwise fake.
    assert!(home.join("hive/PROTOCOL.md").exists());
    assert!(home.join("harness.db").exists());
    // Fresh-home seed: default missions landed through the config store.
    let cfg_missions = st.config.lock().unwrap().missions.len();
    assert!(cfg_missions >= 1, "seed_missions should populate defaults");

    // Wizard finish through the same store the command calls.
    let patch = json!({
        "onboardingComplete": true,
        "harnessHome": home.display().to_string(),
        "godProvider": "claude"
    });
    let (cfg, _raw) = st.config_store.update(patch.as_object().unwrap()).unwrap();
    *st.config.lock().unwrap() = cfg;
    let on_disk: Value =
        serde_json::from_str(&fs::read_to_string(home.join("config.json")).unwrap()).unwrap();
    assert_eq!(on_disk["onboardingComplete"], true);

    // Agents + mail through st.hive — the Arc the commands share.
    st.hive
        .register_agent(&RegistryEntry {
            id: "god-1".into(),
            name: "god-1".into(),
            provider: "claude".into(),
            role: "orchestrator".into(),
            cwd: home.join("god-1").display().to_string(),
            is_god: true,
            status: "idle".into(),
            session_id: None,
            command: String::new(),
            created_at: "2026-01-01T00:00:00Z".into(),
            ..Default::default()
        })
        .unwrap();
    st.hive
        .send_partial(
            &PartialMessage {
                to: Some("god-1".into()),
                act: Some("done".into()),
                body: Some("worker reporting".into()),
                ..Default::default()
            },
            "worker-1",
        )
        .unwrap();
    assert!(st.hive.inbox("god-1").unwrap().iter().any(|m| m.body == "worker reporting"));

    // Roster mirror + persist store — same stores the commands wrap.
    st.roster
        .write(&json!({
            "agents": [{ "id": "a1" }], "archived": [], "restorable": [],
            "queues": {}, "selectedId": "a1"
        }))
        .unwrap();
    assert!(st.roster.read().is_some());

    // Breaker state defaults — the floor starts un-paused.
    assert_eq!(
        st.breaker.lock().unwrap().level_for("a1"),
        agent_office_lib::breaker::BreakerLevel::Healthy
    );
}
