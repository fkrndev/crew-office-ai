//! Circuit breaker — port of `src/main/breaker.ts` (runaway/cost guardrail).
//!
//! Policy only, no side effects: `tick()` reads signals and returns decisions;
//! the beat in lib.rs performs enforcement (steer mail / constrain / kill)
//! and emits `breaker-state` events. Escalates one level per tripping beat,
//! de-escalates one per healthy beat, and `hard_stop` OFF caps the ladder at
//! `constrained` — never kills.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BreakerLevel {
    Healthy = 0,
    Steering = 1,
    Constrained = 2,
    Stopped = 3,
}

#[derive(Clone, Serialize)]
pub struct BreakerState {
    pub agent_id: String,
    pub level: BreakerLevel,
    pub reason: String,
    pub ts: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BreakerAction {
    None,
    Steer,
    Constrain,
    Stop,
}

pub struct BreakerDecision {
    pub state: BreakerState,
    pub action: BreakerAction,
    /// Level changed since the previous beat (escalation OR recovery) — part of
    /// the upstream contract; consumers arrive with the fleet dashboard.
    #[allow(dead_code)]
    pub changed: bool,
}

/// Cumulative usage snapshot — fed from our cost ledger accumulation
/// (upstream: Oscar's UsageProvider; same pull seam).
#[derive(Clone, Copy, Default)]
pub struct UsageSample {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
    pub usd: f64,
    pub ts: u64,
}

pub struct BreakerInput {
    pub agent_id: String,
    pub sample: Option<UsageSample>,
    /// Coordination progress recently (hive file mtimes).
    pub progressing: bool,
    /// Agent's working-dir last change (git markers). None = caller can't tell.
    pub last_work_at: Option<u64>,
}

fn tokens_of(s: Option<&UsageSample>) -> u64 {
    s.map(|x| x.input + x.output + x.cache_read + x.cache_creation)
        .unwrap_or(0)
}

/// Work tokens (excludes cache_read — re-billed context is a session-length
/// timer, not work; upstream #189).
fn work_tokens_of(s: Option<&UsageSample>) -> u64 {
    s.map(|x| x.input + x.output + x.cache_creation).unwrap_or(0)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BreakerConfig {
    pub enabled: Option<bool>,
    pub hard_stop: Option<bool>,
    pub repeated_tool_limit: Option<u32>,
    pub error_storm_limit: Option<u32>,
    pub token_velocity_per_min: Option<u64>,
}

const COMPACT_GRACE_MS: u64 = 5 * 60_000;
const POST_COMPACT_GRACE_MS: u64 = 90_000;
const PROGRESS_TOOL_WINDOW_MS: u64 = 300_000;
const NO_PROGRESS_BEATS: u32 = 2;

#[derive(Default)]
struct AgentBreakerState {
    level: BreakerLevel,
    reason: String,
    last_sample: Option<UsageSample>,
    repeat_key: Option<u64>,
    repeat_count: u32,
    error_count: u32,
    compacting_until: u64,
    last_distinct_tool_at: u64,
    last_user_prompt_at: u64,
    no_progress_beats: u32,
}

impl Default for BreakerLevel {
    fn default() -> Self {
        Self::Healthy
    }
}

#[derive(Default)]
pub struct CircuitBreaker {
    agents: HashMap<String, AgentBreakerState>,
}

impl CircuitBreaker {
    pub fn forget(&mut self, agent_id: &str) {
        self.agents.remove(agent_id);
    }

    pub fn level_for(&self, agent_id: &str) -> BreakerLevel {
        self.agents
            .get(agent_id)
            .map(|a| a.level)
            .unwrap_or(BreakerLevel::Healthy)
    }

    // ── event-driven inputs (fed by HookServer) ────────────────────────────

    pub fn record_tool_use(&mut self, agent_id: &str, tool_name: Option<&str>, tool_input: &serde_json::Value, now: u64) {
        let key = tool_key(tool_name, tool_input);
        let s = self.agents.entry(agent_id.into()).or_default();
        if s.repeat_key == Some(key) {
            s.repeat_count += 1;
        } else {
            s.repeat_key = Some(key);
            s.repeat_count = 1;
            s.error_count = 0; // distinct call = progress; clear the error storm
            s.last_distinct_tool_at = now;
        }
    }

    /// Fed by the api_error-storm source (OTel/telemetry — not yet ported).
    #[allow(dead_code)]
    pub fn record_error(&mut self, agent_id: &str) {
        self.agents.entry(agent_id.into()).or_default().error_count += 1;
    }

    pub fn record_user_prompt(&mut self, agent_id: &str, now: u64) {
        self.agents.entry(agent_id.into()).or_default().last_user_prompt_at = now;
    }

    pub fn record_compact_start(&mut self, agent_id: &str, now: u64) {
        self.agents.entry(agent_id.into()).or_default().compacting_until = now + COMPACT_GRACE_MS;
    }

    pub fn record_compact_end(&mut self, agent_id: &str, now: u64) {
        let s = self.agents.entry(agent_id.into()).or_default();
        if s.compacting_until > now {
            s.compacting_until = now + POST_COMPACT_GRACE_MS;
        }
    }

    // ── periodic evaluation (the beat) ──────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        inputs: &[BreakerInput],
        now_ms: u64,
        cfg: &BreakerConfig,
        cost_cap_usd: Option<f64>,
        cost_cap_tokens: Option<u64>,
        agent_token_caps: &HashMap<String, u64>,
    ) -> Vec<BreakerDecision> {
        let enabled = cfg.enabled.unwrap_or(true);
        let hard_stop = cfg.hard_stop.unwrap_or(false);
        let repeated_tool_limit = cfg.repeated_tool_limit.unwrap_or(8);
        let error_storm_limit = cfg.error_storm_limit.unwrap_or(5);
        let token_velocity = cfg.token_velocity_per_min.unwrap_or(60_000);

        if !enabled {
            return inputs
                .iter()
                .map(|i| {
                    let s = self.agents.entry(i.agent_id.clone()).or_default();
                    let changed = s.level != BreakerLevel::Healthy;
                    s.level = BreakerLevel::Healthy;
                    s.reason.clear();
                    BreakerDecision {
                        state: BreakerState {
                            agent_id: i.agent_id.clone(),
                            level: BreakerLevel::Healthy,
                            reason: String::new(),
                            ts: now_ms,
                        },
                        action: BreakerAction::None,
                        changed,
                    }
                })
                .collect();
        }

        // Floor-wide caps: blame the single biggest spender.
        let mut top_spender: Option<&str> = None;
        if let Some(cap) = cost_cap_usd.filter(|c| *c > 0.0) {
            let (mut total, mut max) = (0.0f64, -1.0f64);
            for i in inputs {
                let usd = i.sample.map(|s| s.usd).unwrap_or(0.0);
                total += usd;
                if usd > max {
                    max = usd;
                    top_spender = Some(&i.agent_id);
                }
            }
            if total <= cap {
                top_spender = None;
            }
        }
        let mut top_token_spender: Option<&str> = None;
        if let Some(cap) = cost_cap_tokens.filter(|c| *c > 0) {
            let (mut total, mut max) = (0u64, 0u64);
            let mut found = false;
            for i in inputs {
                let tok = tokens_of(i.sample.as_ref());
                total += tok;
                if !found || tok > max {
                    max = tok;
                    top_token_spender = Some(&i.agent_id);
                    found = true;
                }
            }
            if total <= cap {
                top_token_spender = None;
            }
        }

        inputs
            .iter()
            .map(|input| {
                let s = self.agents.entry(input.agent_id.clone()).or_default();
                let trip = evaluate(
                    input,
                    s,
                    now_ms,
                    repeated_tool_limit,
                    error_storm_limit,
                    token_velocity,
                    agent_token_caps.get(&input.agent_id).copied(),
                    top_spender == Some(input.agent_id.as_str()),
                    cost_cap_usd,
                    top_token_spender == Some(input.agent_id.as_str()),
                    cost_cap_tokens,
                );
                if let Some(sample) = input.sample {
                    s.last_sample = Some(sample);
                }
                let ceiling = if hard_stop {
                    BreakerLevel::Stopped
                } else {
                    BreakerLevel::Constrained
                };
                let target = if trip.is_some() {
                    s.level.min(ceiling).succ().min(ceiling)
                } else {
                    s.level.pred()
                };
                let changed = target != s.level;
                let escalated = target > s.level;
                s.level = target;
                s.reason = match trip {
                    Some(r) => r,
                    None if changed => "recovering — signals cleared".into(),
                    None => s.reason.clone(),
                };
                BreakerDecision {
                    state: BreakerState {
                        agent_id: input.agent_id.clone(),
                        level: target,
                        reason: s.reason.clone(),
                        ts: now_ms,
                    },
                    action: if escalated {
                        match target {
                            BreakerLevel::Steering => BreakerAction::Steer,
                            BreakerLevel::Constrained => BreakerAction::Constrain,
                            BreakerLevel::Stopped => BreakerAction::Stop,
                            BreakerLevel::Healthy => BreakerAction::None,
                        }
                    } else {
                        BreakerAction::None
                    },
                    changed,
                }
            })
            .collect()
    }
}

impl BreakerLevel {
    fn succ(self) -> Self {
        match self {
            Self::Healthy => Self::Steering,
            Self::Steering => Self::Constrained,
            Self::Constrained | Self::Stopped => Self::Stopped,
        }
    }
    fn pred(self) -> Self {
        match self {
            Self::Stopped => Self::Constrained,
            Self::Constrained => Self::Steering,
            Self::Steering | Self::Healthy => Self::Healthy,
        }
    }
}

/// Port of evaluate() — returns Some(reason) when tripping.
#[allow(clippy::too_many_arguments)]
fn evaluate(
    input: &BreakerInput,
    s: &mut AgentBreakerState,
    now_ms: u64,
    repeated_tool_limit: u32,
    error_storm_limit: u32,
    token_velocity: u64,
    per_agent_cap: Option<u64>,
    is_top_spender: bool,
    cost_cap_usd: Option<f64>,
    is_top_token_spender: bool,
    cost_cap_tokens: Option<u64>,
) -> Option<String> {
    if s.repeat_count >= repeated_tool_limit {
        return Some(format!(
            "looping: {}× identical tool call",
            s.repeat_count
        ));
    }
    if s.error_count >= error_storm_limit {
        return Some(format!(
            "error storm: {} consecutive api errors/retries",
            s.error_count
        ));
    }
    if let Some(cap) = per_agent_cap.filter(|c| *c > 0) {
        let work = work_tokens_of(input.sample.as_ref());
        if work > cap {
            return Some(format!(
                "token limit: {work} work tokens over the agent cap of {cap} ({} total incl. cache reads)",
                tokens_of(input.sample.as_ref())
            ));
        }
    }
    if is_top_spender {
        if let Some(cap) = cost_cap_usd {
            return Some(format!(
                "cost cap: floor total over ${cap:.2} (top spender ${:.2})",
                input.sample.map(|x| x.usd).unwrap_or(0.0)
            ));
        }
    }
    if is_top_token_spender {
        if let Some(cap) = cost_cap_tokens {
            return Some(format!(
                "token cap: floor total over {cap} tokens (top spender {})",
                tokens_of(input.sample.as_ref())
            ));
        }
    }
    if let (Some(cur), Some(prev)) = (input.sample, s.last_sample) {
        if now_ms >= s.compacting_until {
            let d_out = cur.output.saturating_sub(prev.output);
            let d_ms = cur.ts.saturating_sub(prev.ts);
            if d_out > 0 && d_ms > 0 {
                let velocity = d_out.saturating_mul(60_000) / d_ms;
                if velocity > token_velocity {
                    return Some(format!(
                        "token velocity {velocity}/min > {token_velocity}/min"
                    ));
                }
                let tool_active = now_ms.saturating_sub(s.last_distinct_tool_at) < PROGRESS_TOOL_WINDOW_MS;
                let work_active = input
                    .last_work_at
                    .is_some_and(|t| t > 0 && now_ms.saturating_sub(t) < PROGRESS_TOOL_WINDOW_MS);
                let human_active = now_ms.saturating_sub(s.last_user_prompt_at) < PROGRESS_TOOL_WINDOW_MS;
                if !input.progressing && !tool_active && !work_active && !human_active {
                    s.no_progress_beats += 1;
                    if s.no_progress_beats >= NO_PROGRESS_BEATS {
                        return Some(
                            "no-progress: generating tokens without coordinating (stale log/files)"
                                .into(),
                        );
                    }
                } else {
                    s.no_progress_beats = 0;
                }
            }
        }
    }
    None
}

/// Loop-detection key: a hash of the capped (name+input) serialization —
/// upstream #377: hashing the whole capped payload avoids slice collisions on
/// shared command preambles; per-string cap 4096 keeps late-differing inputs
/// distinguishable. DefaultHasher is session-local, which is all a repeat-key
/// needs (upstream used sha256 only because it already had crypto imported).
fn tool_key(tool_name: Option<&str>, tool_input: &serde_json::Value) -> u64 {
    let mut capped = String::new();
    capped_json(tool_input, &mut capped);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    tool_name.unwrap_or("?").hash(&mut h);
    capped.hash(&mut h);
    h.finish()
}

fn capped_json(v: &serde_json::Value, out: &mut String) {
    match v {
        serde_json::Value::String(s) => {
            out.push('"');
            out.push_str(&s.chars().take(4096).collect::<String>());
            out.push('"');
        }
        serde_json::Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                capped_json(x, out);
            }
            out.push(']');
        }
        serde_json::Value::Object(o) => {
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push('"');
                out.push_str(k);
                out.push_str("\":");
                capped_json(x, out);
            }
            out.push('}');
        }
        _ => out.push_str(&v.to_string()),
    }
}
