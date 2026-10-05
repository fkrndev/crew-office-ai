//! Onboarding finish-path coverage — the exact backend sequence the wizard's
//! Finish button drives:
//!
//!   1. `config:ensureHome`  → mkdir harness home + hive subdir (cmd_misc)
//!   2. `config:update`      → merge patch → typed re-parse → persist
//!                             (commands::config_update → ConfigStore::update)
//!   3. `config:get`         → typed serialization overlaid with the raw file
//!
//! The renderer pins completion to config_update's RETURN VALUE
//! (`onComplete(next)` → `config.onboardingComplete`), so the patch keys MUST
//! survive to disk AND appear in the merged read. The regression this pins:
//! `save(&cfg)` re-read the pre-patch file and silently dropped every key the
//! typed `Config` doesn't model — onboardingComplete, godProvider,
//! harnessHome, officeTheme — so Finish wrote a valid config that simply never
//! contained `onboardingComplete`, and the wizard reopened.
//!
//! commands::* take an AppHandle, so the tests drive the same store-level
//! write path (`ConfigStore::update`) the command now calls.

use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

use agent_office_lib::config::{Config, ConfigStore};

fn tmpdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ao-onboard-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn read_raw(dir: &std::path::Path) -> Value {
    serde_json::from_str(&fs::read_to_string(dir.join("config.json")).unwrap()).unwrap()
}

/// get_config's merge, replicated: typed serialization first, raw file overlaid
/// (raw wins for keys it carries — same precedence as commands::get_config).
fn get_config_merged(cfg: &Config, store: &ConfigStore) -> Value {
    let typed = serde_json::to_value(cfg).unwrap();
    let mut merged = typed.as_object().cloned().unwrap_or_default();
    if let Some(raw) = store.raw_value().as_object() {
        for (k, v) in raw {
            merged.insert(k.clone(), v.clone());
        }
    }
    Value::Object(merged)
}

/// The wizard's Finish writes onboardingComplete + several other keys the
/// typed Config does not model. All of them must land on disk and be visible
/// in the merged get_config shape — a drop anywhere reopens the wizard.
#[test]
fn finish_patch_persists_unknown_keys() {
    let dir = tmpdir("finish");
    let store = ConfigStore::new(dir.clone());

    // Same patch OnboardingWizard.finish() sends.
    let patch = json!({
        "onboardingComplete": true,
        "audience": "technical",
        "harnessHome": dir.display().to_string(),
        "registeredRepos": ["D:\\repo"],
        "autoMode": true,
        "godProvider": "hermes",
        "godModel": "anthropic/claude-sonnet-4.6",
        "telemetryEnabled": true
    });

    let (cfg, _raw) = store.update(patch.as_object().unwrap()).unwrap();
    assert!(cfg.auto_mode, "typed field must apply");

    let on_disk = read_raw(&dir);
    assert_eq!(on_disk["onboardingComplete"], true, "dropped = wizard reopens");
    assert_eq!(on_disk["godProvider"], "hermes");
    assert_eq!(on_disk["harnessHome"], json!(dir.display().to_string()));
    assert_eq!(on_disk["telemetryEnabled"], true);
    assert_eq!(on_disk["autoMode"], true);

    // What the renderer receives back (get_config merge) must carry the flag.
    let merged = get_config_merged(&cfg, &store);
    assert_eq!(merged["onboardingComplete"], true);
    assert_eq!(merged["godProvider"], "hermes");
}

/// The regression: two sequential writes must not clobber each other's
/// unknown keys — Finish followed by a voice-settings write, etc.
#[test]
fn sequential_updates_preserve_each_others_unknowns() {
    let dir = tmpdir("seq");
    let store = ConfigStore::new(dir.clone());

    store
        .update(json!({ "onboardingComplete": true, "godProvider": "openclaw" }).as_object().unwrap())
        .unwrap();
    // A later write with only typed keys must not drop the earlier unknowns.
    store.update(json!({ "autoMode": false }).as_object().unwrap()).unwrap();

    let raw = read_raw(&dir);
    assert_eq!(raw["onboardingComplete"], true);
    assert_eq!(raw["godProvider"], "openclaw");
    assert_eq!(raw["autoMode"], false);
}

/// A config written by an older build — snake_case twins, null Options, a flat
/// legacy `context_trigger`, missions with null `weekly` — must still parse.
/// config_update re-parses the whole file, so one stale key would reject EVERY
/// write (Finish throws, the button never completes).
#[test]
fn real_world_config_from_older_build_parses() {
    let text = include_str!("fixtures/real-config.json");
    let raw: Value = serde_json::from_str(text).expect("fixture is valid JSON");
    let cfg: Config = serde_json::from_value(raw.clone())
        .expect("older-build config must parse into typed Config");
    // Missions survive; the flat legacy context_trigger maps to defaults.
    assert_eq!(cfg.missions.len(), 3);
    assert!(cfg.notifications);
    // camelCase autoMode is the read key; the snake twin is inert.
    assert!(cfg.auto_mode);
}

/// The same older-build file must accept the Finish patch end-to-end:
/// update → typed parse → file still contains onboardingComplete.
#[test]
fn finish_patch_survives_older_build_file() {
    let dir = tmpdir("older");
    let store = ConfigStore::new(dir.clone());
    fs::write(
        dir.join("config.json"),
        include_str!("fixtures/real-config.json"),
    )
    .unwrap();

    let patch = json!({ "onboardingComplete": true, "godProvider": "claude" });
    let (cfg, _raw) = store.update(patch.as_object().unwrap()).unwrap();

    let raw = read_raw(&dir);
    assert_eq!(raw["onboardingComplete"], true);
    let merged = get_config_merged(&cfg, &store);
    assert_eq!(merged["onboardingComplete"], true);
    // Older unknown keys were never dropped.
    assert_eq!(merged["godName"], "Michael");
}

/// A patch that breaks a KNOWN typed field must still fail the write — silent
/// acceptance would corrupt the file for every reader.
#[test]
fn bad_typed_field_rejects_the_write() {
    let dir = tmpdir("bad");
    let store = ConfigStore::new(dir.clone());
    store.update(json!({ "autoMode": true }).as_object().unwrap()).unwrap();

    let err = store
        .update(json!({ "missions": "not-an-array" }).as_object().unwrap())
        .expect_err("a wrong-typed known field must reject");
    assert!(!err.is_empty());

    // The file keeps its last good state.
    let raw = read_raw(&dir);
    assert_eq!(raw["autoMode"], true);
    assert!(raw["missions"].is_array() || raw.get("missions").is_none());
}
