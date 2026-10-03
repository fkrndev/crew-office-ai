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
#[serde(default)]
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
        ensure(crate::missions::compact_maintenance(), &mut self.compact_seeded);
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

    pub fn save(&self, cfg: &Config) -> Result<()> {
        if let Some(p) = self.path.parent() {
            fs::create_dir_all(p)?;
        }
        fs::write(&self.path, serde_json::to_string_pretty(cfg)?)?;
        Ok(())
    }
}
