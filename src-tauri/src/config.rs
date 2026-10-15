//! App config — port of `src/main/config.ts` (slimmed: the roster lives in the
//! hive's registry.json; this file holds harness-level settings only).

use std::fs;
use std::path::{Path, PathBuf};

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
    /// Per-agent USD spend caps — trips the breaker for that agent alone.
    /// Distinct from cost_cap_usd (a floor-wide budget that trips only the
    /// single biggest spender): this bounds ONE agent regardless of the
    /// floor total. Set per hire or in Command Center.
    pub agent_cost_caps: HashMap<String, f64>,
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
    /// Per-CLI-provider local/custom base URLs (upstream HarnessConfig
    /// .providerBaseUrls) — non-secret; how "use my DeepSeek endpoint" reaches
    /// the qwen/crush proxy upstream and OpenCode's local provider block.
    /// Keyed by CLI provider id, not backend.
    pub provider_base_urls: HashMap<String, String>,
    /// Per-CLI-provider default models (upstream providerDefaultModels).
    pub provider_default_models: HashMap<String, String>,
    /// MCP default-server consent (shared/mcpCatalog.ts): catalog id → whether
    /// it is enabled for newly spawned agents. Safe-readonly ids default ON;
    /// write/secret ids require an explicit true — the merge never lets a
    /// keyed server ride in on a default.
    pub mcp_defaults: HashMap<String, McpDefaultCfg>,
    /// Opt-in "strong keep-alive" (upstream strongKeepalive): while >=1 agent
    /// PTY is live, escalate the power request from prevent-app-suspension to
    /// prevent-display-sleep (the display stays on too — battery cost).
    pub strong_keepalive: bool,
    // ─── Memory reflection (the janitor's condense half; reflect.rs) ────────
    // Upstream defaults: on, 30-min scan, condense past 50% of the 128KB
    // budget OR >50 sections (above a 16KB floor), newest 12 sections kept.
    /// Master toggle for the periodic MemoryReflector.
    pub reflect_enabled: bool,
    /// How often to scan for oversized memory files (floored at 60s).
    pub reflect_interval_ms: u64,
    pub reflect_byte_trigger_pct: u64,
    pub reflect_section_trigger: usize,
    pub reflect_recent_keep: usize,
    pub reflect_min_bytes: u64,
    /// Elapsed-time carry-over from a retired `compact-maintenance` mission
    /// (see `seed_missions`): the boot code hands it to the context-trigger
    /// cadence map so retiring mid-cycle doesn't restart the clock. In-memory
    /// only — never written to config.json.
    #[serde(skip)]
    pub retired_compact_last_fired: u64,
}

/// One mcpDefaults row — `{enabled}` object, same wire shape as the renderer's
/// config.mcpDefaults.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct McpDefaultCfg {
    pub enabled: bool,
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
            agent_cost_caps: HashMap::new(),
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
            provider_base_urls: HashMap::new(),
            provider_default_models: HashMap::new(),
            mcp_defaults: HashMap::new(),
            strong_keepalive: false,
            reflect_enabled: true,
            reflect_interval_ms: 1_800_000,
            reflect_byte_trigger_pct: 50,
            reflect_section_trigger: 50,
            reflect_recent_keep: 12,
            reflect_min_bytes: 16_384,
            retired_compact_last_fired: 0,
        }
    }
}

impl Config {
    /// Seed the built-in missions once (upstream ensureDefaultMissions):
    /// stamped with last_fired_at = now so a first launch waits a full
    /// cadence rather than opening with an interruption. Then the two
    /// compaction migrations, so contextTrigger.compact is the ONLY schedule
    /// that compacts:
    ///  - maint-1 RETIREMENT: a `compact-maintenance` mission (older builds
    ///    seeded one) is folded into contextTrigger.compact — its enabled flag
    ///    and a usable interval carry over — and removed; nothing re-seeds it.
    ///    Its last-fired time is parked in `retired_compact_last_fired`.
    ///  - autoCompact RETIREMENT: the legacy per-mission flag is stripped
    ///    wherever it survives (it made a default install compact on two
    ///    cadences). Idempotent — one no-op scan per boot once clean.
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
        if let Some(pos) = self
            .missions
            .iter()
            .position(|m| m.id == "compact-maintenance")
        {
            let retiring = self.missions.remove(pos);
            self.context_trigger.compact.enabled = retiring.enabled;
            // A hand-tuned interval is a decision; only a missing/absurd one
            // falls back to what the trigger already carries.
            if retiring.interval_ms > 0 {
                self.context_trigger.compact.every_ms = retiring.interval_ms;
            }
            self.retired_compact_last_fired = retiring.last_fired_at;
            self.compact_seeded = true;
            changed = true;
        }
        for m in self.missions.iter_mut().filter(|m| m.auto_compact) {
            m.auto_compact = false;
            changed = true;
        }
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

    /// The harness-home directory this store persists into — secrets' DPAPI
    /// fallback file lives alongside config.json there.
    pub fn dir(&self) -> &Path {
        self.path.parent().unwrap_or(&self.path)
    }

    /// Typed config for this run. A missing file → defaults. A CORRUPT file
    /// also yields defaults so the app can start, but every WRITER goes through
    /// `read_raw`, which refuses a corrupt file — so the defaults are never
    /// persisted over the user's real config (a `.bak` copy is kept).
    pub fn load(&self) -> Config {
        match self.read_raw() {
            Ok(v) => serde_json::from_value(v).unwrap_or_default(),
            Err(e) => {
                log::warn!("[config] {e:#} — running on defaults, file left untouched");
                Config::default()
            }
        }
    }

    /// Raw on-disk JSON, unknown keys preserved (upstream's untyped config
    /// object — the renderer reads keys the typed `Config` doesn't model,
    /// e.g. officeTheme/terminalTheme, and voice settings can write them).
    /// Read-only callers get `{}` for a missing/corrupt file; writers must use
    /// `read_raw` so a corrupt file is never treated as empty.
    pub fn raw_value(&self) -> serde_json::Value {
        self.read_raw().unwrap_or_else(|_| serde_json::json!({}))
    }

    /// The store lock — held across every read-modify-write of config.json.
    fn lock(&self) -> std::sync::Arc<std::sync::Mutex<()>> {
        crate::hive::store_lock(&self.path)
    }

    /// Raw config for a read-modify-write: missing/empty → `{}`; unparseable
    /// or non-object → Err (copy kept as `config.json.bak`), never `{}`.
    fn read_raw(&self) -> Result<serde_json::Value> {
        let v = crate::hive::read_json_strict(&self.path, serde_json::json!({}))?;
        if v.is_null() {
            return Ok(serde_json::json!({}));
        }
        if !v.is_object() {
            anyhow::bail!("{} is not a JSON object", self.path.display());
        }
        Ok(v)
    }

    fn write_raw(&self, raw: &serde_json::Value) -> Result<()> {
        if let Some(p) = self.path.parent() {
            fs::create_dir_all(p)?;
        }
        crate::hive::atomic_write_file(&self.path, serde_json::to_string_pretty(raw)?.as_bytes())?;
        Ok(())
    }

    /// Merge a top-level patch into the raw file without dropping unknown
    /// keys (upstream `writeConfig(patch)` semantics), then return the
    /// reloaded typed Config.
    pub fn patch_raw(&self, patch: &serde_json::Map<String, serde_json::Value>) -> Result<Config> {
        {
            let l = self.lock();
            let _g = crate::hive::lock_store(&l);
            let mut raw = self.read_raw()?;
            if let Some(dst) = raw.as_object_mut() {
                for (k, v) in patch {
                    dst.insert(k.clone(), v.clone());
                }
            }
            self.write_raw(&raw)?;
        }
        Ok(self.load())
    }

    /// config_update's write path, factored so integration tests can drive it.
    /// Merge the patch into the raw object, validate the result parses to the
    /// typed Config (Err on a known field with a wrong type — the renderer
    /// sees the rejection), then persist the MERGED object verbatim.
    ///
    /// Writing `raw` — not `save(&cfg)` — is the point: `save` re-reads the
    /// on-disk file, so patch keys the typed Config doesn't model
    /// (onboardingComplete, godProvider, harnessHome, themes…) would vanish.
    /// That is exactly what the onboarding Finish button relied on.
    ///
    /// Returns the typed Config and the merged object as written — the same
    /// shape get_config returns, so callers do not re-merge.
    pub fn update(
        &self,
        patch: &serde_json::Map<String, serde_json::Value>,
    ) -> std::result::Result<(Config, serde_json::Value), String> {
        let l = self.lock();
        let _g = crate::hive::lock_store(&l);
        let mut raw = self.read_raw().map_err(|e| format!("{e:#}"))?;
        if let Some(dst) = raw.as_object_mut() {
            for (k, v) in patch {
                dst.insert(k.clone(), v.clone());
            }
        }
        let cfg: Config = serde_json::from_value(raw.clone()).map_err(|e| e.to_string())?;
        self.write_raw(&raw).map_err(|e| format!("{e:#}"))?;
        Ok((cfg, raw))
    }

    /// Overwrite the file with first-run defaults (upstream `resetConfig()`)
    /// — every untyped key (onboardingComplete, harnessHome, godProvider,
    /// themes…) drops, which is exactly what makes the relaunch land on
    /// onboarding instead of resurrecting the wiped setup.
    pub fn reset(&self) -> Result<()> {
        let l = self.lock();
        let _g = crate::hive::lock_store(&l);
        self.write_raw(&serde_json::to_value(Config::default())?)
    }

    /// Serialize the typed config but merge it over the raw file so keys the
    /// struct doesn't model survive a save round-trip — upstream's
    /// `writeConfig` writes the whole parsed object, unknown keys included.
    pub fn save(&self, cfg: &Config) -> Result<()> {
        let l = self.lock();
        let _g = crate::hive::lock_store(&l);
        let mut merged = self.read_raw()?;
        if let (Some(dst), Some(src)) = (
            merged.as_object_mut(),
            serde_json::to_value(cfg)?.as_object().cloned(),
        ) {
            for (k, v) in src {
                dst.insert(k, v);
            }
        }
        self.write_raw(&merged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oa-config-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn corrupt_config_is_never_overwritten_with_defaults() {
        let d = dir("corrupt");
        let store = ConfigStore::new(d.clone());
        fs::write(d.join("config.json"), "{ \"autoMode\": tru").unwrap();
        // The app still starts on defaults...
        assert!(!store.load().auto_mode);
        // ...but no writer may replace the user's file with them.
        assert!(store.save(&Config::default()).is_err());
        assert!(store.update(&serde_json::Map::new()).is_err());
        assert!(store.patch_raw(&serde_json::Map::new()).is_err());
        assert_eq!(
            fs::read_to_string(d.join("config.json")).unwrap(),
            "{ \"autoMode\": tru"
        );
        assert!(d.join("config.json.bak").exists());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn seed_retires_compact_maintenance_into_the_context_trigger() {
        let mut cfg = Config::default();
        // An older build's seeded schedule: enabled, hand-tuned to 30 min,
        // plus a stray legacy autoCompact flag on the standup.
        let mut cm = crate::missions::compact_maintenance();
        cm.enabled = true;
        cm.interval_ms = 1_800_000;
        cm.last_fired_at = 4242;
        cfg.missions.push(cm);
        let mut standup = crate::missions::ops_standup();
        standup.auto_compact = true;
        cfg.missions.push(standup);
        cfg.ops_standup_seeded = true;
        cfg.heartbeat_seeded = true;
        assert!(!cfg.context_trigger.compact.enabled || cfg.context_trigger.compact.every_ms != 1_800_000);

        assert!(cfg.seed_missions(1_000));
        assert!(cfg.missions.iter().all(|m| m.id != "compact-maintenance"));
        assert!(cfg.context_trigger.compact.enabled);
        assert_eq!(cfg.context_trigger.compact.every_ms, 1_800_000);
        assert_eq!(cfg.retired_compact_last_fired, 4242);
        assert!(cfg.compact_seeded);
        assert!(cfg.missions.iter().all(|m| !m.auto_compact), "legacy flag stripped");
        // Idempotent: nothing left to migrate, and it is never re-seeded.
        cfg.retired_compact_last_fired = 0;
        assert!(!cfg.seed_missions(2_000));
        assert!(cfg.missions.iter().all(|m| m.id != "compact-maintenance"));

        // A DISABLED retiring mission turns the trigger rule off (the
        // operator's decision carries over).
        let mut off = Config::default();
        off.ops_standup_seeded = true;
        off.heartbeat_seeded = true;
        off.missions.push(crate::missions::compact_maintenance()); // enabled: false
        off.seed_missions(1);
        assert!(!off.context_trigger.compact.enabled);
    }

    #[test]
    fn reflect_and_keepalive_defaults_match_upstream() {
        let c = Config::default();
        assert!(c.reflect_enabled && !c.strong_keepalive);
        assert_eq!(
            (c.reflect_interval_ms, c.reflect_byte_trigger_pct, c.reflect_section_trigger,
             c.reflect_recent_keep, c.reflect_min_bytes),
            (1_800_000, 50, 50, 12, 16_384)
        );
        // A pre-existing config.json without the keys still gets them.
        let parsed: Config = serde_json::from_value(serde_json::json!({ "autoMode": true })).unwrap();
        assert!(parsed.reflect_enabled && parsed.reflect_min_bytes == 16_384);
    }

    #[test]
    fn update_preserves_unknown_keys_and_missing_file_is_fine() {
        let d = dir("update");
        let store = ConfigStore::new(d.clone());
        let mut patch = serde_json::Map::new();
        patch.insert("onboardingComplete".into(), serde_json::json!(true));
        patch.insert("autoMode".into(), serde_json::json!(true));
        let (cfg, raw) = store.update(&patch).unwrap();
        assert!(cfg.auto_mode);
        assert_eq!(raw["onboardingComplete"], true);
        let mut cfg2 = store.load();
        cfg2.god_name = "Zed".into();
        store.save(&cfg2).unwrap();
        assert_eq!(store.raw_value()["onboardingComplete"], true);
        assert_eq!(store.load().god_name, "Zed");
        fs::remove_dir_all(&d).ok();
    }
}
