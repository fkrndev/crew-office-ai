//! App config — port of `src/main/config.ts` (slimmed: the roster lives in the
//! hive's registry.json; this file holds harness-level settings only).

use std::fs;
use std::path::PathBuf;

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::breaker::BreakerConfig;

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
        }
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
