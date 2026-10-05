//! App config — port of `src/main/config.ts` (slimmed: the roster lives in the
//! hive's registry.json; this file holds harness-level settings only).

use std::fs;
use std::path::PathBuf;

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::breaker::BreakerConfig;
use crate::missions::ScheduledMission;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// Floor-wide autonomy toggle — gates each provider's autoModeFlag.
    pub auto_mode: bool,
    /// Display name of the god/orchestrator agent ("Michael" upstream).
    pub god_name: String,
    /// Circuit-breaker policy knobs (breaker.rs).
    pub circuit_breaker: BreakerConfig,
    /// Floor-wide $ cap — trips the single biggest spender when total exceeds.
    pub cost_cap_usd: Option<f64>,
    /// Floor-wide token cap (all kinds) — same top-spender logic.
    pub cost_cap_tokens: Option<u64>,
    /// Per-agent work-token caps (cache_read excluded, upstream #189).
    pub agent_token_caps: HashMap<String, u64>,
    /// Recurring auto-dispatch missions (missions.rs + scheduler loop).
    pub missions: Vec<ScheduledMission>,
    /// One-time seeding guards — a user who deletes a built-in mission does
    /// not get it re-added every boot (upstream *_seeded flags).
    pub ops_standup_seeded: bool,
    pub heartbeat_seeded: bool,
    pub compact_seeded: bool,
    /// Inbound webhook endpoints (webhook.rs) + the loopback port it binds.
    /// Secrets live here (harness-local file) and never leave the backend.
    pub webhooks: Vec<crate::webhook::WebhookEndpoint>,
    pub webhook_port: u16,
    /// Names of OS-keychain secrets (Windows Credential Manager via the
    /// keyring crate) injected into every spawned agent's env — e.g.
    /// ANTHROPIC_API_KEY, OPENAI_API_KEY. Values never live in this file.
    pub secret_env_keys: Vec<String>,
    /// Context trigger (shared/triggers.ts ContextTriggerConfig): two rules —
    /// `compact` and `clear` — each with its own cadence + pressure gate.
    /// The backend owns only the CADENCE; the renderer applies the per-agent
    /// pressure gate and queues the provider-specific verb.
    pub context_trigger: ContextTriggerCfg,
    /// Let the orchestrator hire ephemeral workers by dropping JSON into
    /// hive/spawn-requests/ (default OFF — a spend decision the operator
    /// opts into; index.ts orchestratorMaySpawn).
    pub orchestrator_may_spawn: bool,
    /// Concurrency ceiling for live ephemeral workers (upstream
    /// maxConcurrentWorkers).
    pub max_concurrent_workers: u32,
    /// Idle minutes after which a worker that never signaled done is reaped
    /// (upstream workerIdleTimeoutMinutes).
    pub worker_idle_timeout_minutes: u32,
    /// Per-worker token cap when a request doesn't carry one. 0 = unlimited
    /// (the default — wired but never throttles, upstream
    /// defaultWorkerTokenCap).
    pub default_worker_token_cap: u64,
    /// OS toast notifications for agent idle/blocked events (upstream
    /// notifications; hooks.ts gates every toast on it).
    pub notifications: bool,
}

/// shared/triggers.ts ContextTriggerConfig — {compact, clear} rules.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContextTriggerCfg {
    pub compact: ContextRule,
    pub clear: ContextRule,
}

/// shared/triggers.ts ContextRule. min_context_pct_large_window relaxes the
/// pressure gate for ~1M-token windows.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContextRule {
    pub enabled: bool,
    /// Min wall-clock gap between fires of this rule.
    pub every_ms: u64,
    /// Percent (0-100) of the context window that must be used.
    pub min_context_pct: u64,
    pub min_context_pct_large_window: u64,
    /// Focus text appended to the compact verb, or the verbatim clear override.
    pub message: String,
}

pub const DEFAULT_COMPACTION_FOCUS: &str =
    "Keep the current task, recent decisions, open questions, and file paths in play. Drop resolved tangents.";

impl Default for ContextRule {
    fn default() -> Self {
        Self {
            enabled: false,
            every_ms: 7_200_000,
            min_context_pct: 60,
            min_context_pct_large_window: 40,
            message: DEFAULT_COMPACTION_FOCUS.into(),
        }
    }
}

impl Default for ContextTriggerCfg {
    fn default() -> Self {
        // Upstream DEFAULT_CONTEXT_TRIGGER (shared/triggers.ts).
        Self {
            compact: ContextRule {
                enabled: true,
                every_ms: 7_200_000,
                min_context_pct: 60,
                min_context_pct_large_window: 40,
                message: DEFAULT_COMPACTION_FOCUS.into(),
            },
            clear: ContextRule {
                enabled: false,
                every_ms: 7_200_000,
                min_context_pct: 90,
                min_context_pct_large_window: 80,
                message: String::new(),
            },
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            auto_mode: false,
            god_name: "Michael".into(),
            circuit_breaker: BreakerConfig::default(),
            cost_cap_usd: None,
            cost_cap_tokens: None,
            agent_token_caps: HashMap::new(),
            missions: Vec::new(),
            ops_standup_seeded: false,
            heartbeat_seeded: false,
            compact_seeded: false,
            webhooks: Vec::new(),
            webhook_port: 8787,
            secret_env_keys: Vec::new(),
            context_trigger: ContextTriggerCfg::default(),
            orchestrator_may_spawn: false,
            max_concurrent_workers: 4,
            worker_idle_timeout_minutes: 20,
            default_worker_token_cap: 0,
            notifications: false,
        }
    }
}

impl Config {
    /// Seed the built-in missions once (upstream ensureDefaultMissions):
    /// stamped with last_fired_at = now so a first launch waits a full
    /// cadence rather than opening with an interruption.
    pub fn seed_missions(&mut self, now_ms: u64) -> bool {
        let mut changed = false;
        let mut ensure = |m: ScheduledMission, seeded: &mut bool| {
            if !*seeded {
                let mut m = m;
                m.last_fired_at = now_ms;
                if !self.missions.iter().any(|x| x.id == m.id) {
                    self.missions.push(m);
                }
                *seeded = true;
                changed = true;
            }
        };
        ensure(crate::missions::ops_standup(), &mut self.ops_standup_seeded);
        ensure(crate::missions::heartbeat(), &mut self.heartbeat_seeded);
        ensure(
            crate::missions::compact_maintenance(),
            &mut self.compact_seeded,
        );
        changed
    }
}

pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            path: dir.join("config.json"),
        }
    }

    pub fn load(&self) -> Config {
        fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Raw on-disk JSON, unknown keys preserved (upstream's untyped config
    /// object — the renderer reads keys the typed `Config` doesn't model,
    /// e.g. officeTheme/terminalTheme, and voice settings can write them).
    pub fn raw_value(&self) -> serde_json::Value {
        fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    }

    /// Merge a top-level patch into the raw file without dropping unknown
    /// keys (upstream `writeConfig(patch)` semantics), then return the
    /// reloaded typed Config.
    pub fn patch_raw(&self, patch: &serde_json::Map<String, serde_json::Value>) -> Result<Config> {
        let mut raw = self.raw_value();
        if !raw.is_object() {
            raw = serde_json::json!({});
        }
        let dst = raw.as_object_mut().unwrap();
        for (k, v) in patch {
            dst.insert(k.clone(), v.clone());
        }
        fs::write(&self.path, serde_json::to_string_pretty(&raw)?)?;
        Ok(self.load())
    }

    /// Serialize the typed config but merge it over the raw file so keys the
    /// struct doesn't model survive a save round-trip — upstream's
    /// `writeConfig` writes the whole parsed object, unknown keys included.
    pub fn save(&self, cfg: &Config) -> Result<()> {
        if let Some(p) = self.path.parent() {
            fs::create_dir_all(p)?;
        }
        let mut merged = self.raw_value();
        if !merged.is_object() {
            merged = serde_json::json!({});
        }
        if let (Some(dst), Some(src)) = (
            merged.as_object_mut(),
            serde_json::to_value(cfg)?.as_object().cloned(),
        ) {
            for (k, v) in src {
                dst.insert(k, v);
            }
        }
        fs::write(&self.path, serde_json::to_string_pretty(&merged)?)?;
        Ok(())
    }
}
