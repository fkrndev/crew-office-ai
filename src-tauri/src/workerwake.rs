//! WorkerWakeWatchdog — port of `src/main/workerWake.ts` (#151).
//!
//! The backstop for a worker that went quiet at its prompt: on a cadence the
//! backend finds live workers that are genuinely idle, have newly arrived
//! inbox mail, are not paused/halted/HITL-held, and have not been nudged
//! recently — then types the guarded nudge directly into the PTY. Message ids
//! make it edge-triggered: unchanged undrained mail is never re-announced.
//!
//! The stall rule (WORKER_WAKE_STALL_MS): mail older than 90s with NO session
//! activity since it landed means the worker never took the first turn — a
//! lost boot nudge. PTY output can't vouch there (a TUI redraws chrome without
//! working), so past that age the quiet-output/booting rules are bypassed.
//! The rule stays OFF for an agent with no hook events at all — a Codex-style
//! engine with no turn-proof channel would read as stalled forever (#368).

use std::collections::{HashMap, HashSet};

/// The exact nudge the idle-wake path types.
pub const WORKER_WAKE_NUDGE: &str = "You have new hive inbox message(s) — read your inbox, \
    act on them now, and move handled ones to inbox/.done/. Act autonomously; \
    only message god if you genuinely need a decision.";

pub const WORKER_WAKE_IDLE_MS: u64 = 12_000;
pub const WORKER_WAKE_BOOT_GRACE_MS: u64 = 35_000;
pub const WORKER_WAKE_COOLDOWN_MS: u64 = 60_000;
pub const WORKER_WAKE_HITL_REARM_MS: u64 = 5 * 60_000;
pub const WORKER_WAKE_STALL_MS: u64 = 90_000;

/// 'needsHuman' | 'idle' | none — a Notification that means "the human is
/// deciding" (permission/approve/confirm) vs the idle-waiting shape.
pub fn classify_hook(event: &str, message: &str) -> Option<&'static str> {
    if event != "Notification" {
        return None;
    }
    let msg = message.to_lowercase();
    let idle_waiting = msg.is_empty()
        || msg.contains("waiting for your input")
        || msg.contains("is idle")
        || msg.contains("waiting for input");
    let needs_human = msg.contains("permission")
        || msg.contains("approve")
        || msg.contains("confirm")
        || msg.contains("needs your");
    if needs_human && !idle_waiting {
        Some("needsHuman")
    } else {
        Some("idle")
    }
}

/// A hook event that proves the CLI took a turn — SessionStart is the CLI
/// coming up (a lost-nudge worker has exactly that), Notification is waiting.
pub fn is_turn_hook(event: &str) -> bool {
    matches!(
        event,
        "UserPromptSubmit"
            | "PreToolUse"
            | "PostToolUse"
            | "PostToolUseFailure"
            | "Stop"
            | "StopFailure"
            | "SubagentStart"
            | "SubagentStop"
    )
}

/// One worker's live facts, gathered by the beat each tick.
pub struct WorkerWakeFacts {
    pub agent_id: String,
    pub is_god: bool,
    /// Live PTY id — None when the agent has no terminal.
    pub pty_id: Option<String>,
    /// PTY last-output timestamp (0 = never output).
    pub last_output_at: u64,
    /// IDs of undrained inbox messages.
    pub inbox_ids: Vec<String>,
    pub auto_delivery_paused: bool,
    pub paused: bool,
    pub halted: bool,
    /// Telemetry-evidenced last activity (transcript usage ts), or 0.
    pub last_activity_at: u64,
    /// True when the usage channel produced ANY sample (its absence means
    /// "no data", not "no work") — claude transcript coverage.
    pub has_telemetry: bool,
    /// created_at of the OLDEST undrained mail (0 = unknown → stall rule off).
    pub oldest_mail_at: u64,
}

fn live_ids(f: &WorkerWakeFacts) -> HashSet<String> {
    f.inbox_ids.iter().filter(|s| !s.is_empty()).cloned().collect()
}

fn is_stalled_worker(f: &WorkerWakeFacts, now: u64) -> bool {
    if f.oldest_mail_at == 0 || live_ids(f).is_empty() {
        return false;
    }
    if now.saturating_sub(f.oldest_mail_at) < WORKER_WAKE_STALL_MS {
        return false;
    }
    f.last_activity_at < f.oldest_mail_at
}

#[derive(Default)]
pub struct WorkerWakeWatchdog {
    spawned_at: HashMap<String, u64>,
    last_nudge_at: HashMap<String, u64>,
    announced_inbox_ids: HashMap<String, HashSet<String>>,
    last_human_needs_at: HashMap<String, u64>,
    hook_seen_at: HashMap<String, u64>,
    last_turn_hook_at: HashMap<String, u64>,
    last_hold_report_at: HashMap<String, u64>,
}

impl WorkerWakeWatchdog {
    pub fn note_spawn(&mut self, pty_id: &str, at: u64) {
        self.spawned_at.insert(pty_id.to_string(), at);
    }

    /// Feed hook events — a HITL prompt blocks nudges; any turn-proving event
    /// is activity the stall rule credits.
    pub fn note_hook(&mut self, agent_id: &str, event: &str, message: &str, at: u64) {
        if agent_id.is_empty() {
            return;
        }
        self.hook_seen_at.insert(agent_id.to_string(), at);
        if is_turn_hook(event) {
            let e = self.last_turn_hook_at.entry(agent_id.to_string()).or_insert(0);
            if at > *e {
                *e = at;
            }
        }
        if classify_hook(event, message) == Some("needsHuman") {
            self.last_human_needs_at.insert(agent_id.to_string(), at);
        }
    }

    pub fn turn_hook_at(&self, agent_id: &str) -> u64 {
        *self.last_turn_hook_at.get(agent_id).unwrap_or(&0)
    }

    fn is_stalled(&self, f: &WorkerWakeFacts, now: u64) -> bool {
        let observable = f.has_telemetry || self.hook_seen_at.contains_key(&f.agent_id);
        if !observable {
            return false;
        }
        let last_activity_at = f.last_activity_at.max(self.turn_hook_at(&f.agent_id));
        let g = WorkerWakeFacts {
            agent_id: f.agent_id.clone(),
            is_god: f.is_god,
            pty_id: f.pty_id.clone(),
            last_output_at: f.last_output_at,
            inbox_ids: f.inbox_ids.clone(),
            auto_delivery_paused: f.auto_delivery_paused,
            paused: f.paused,
            halted: f.halted,
            last_activity_at,
            has_telemetry: f.has_telemetry,
            oldest_mail_at: f.oldest_mail_at,
        };
        is_stalled_worker(&g, now)
    }

    pub fn forget(&mut self, agent_id: &str, pty_id: Option<&str>) {
        self.last_nudge_at.remove(agent_id);
        self.announced_inbox_ids.remove(agent_id);
        self.last_human_needs_at.remove(agent_id);
        self.hook_seen_at.remove(agent_id);
        self.last_turn_hook_at.remove(agent_id);
        self.last_hold_report_at.remove(agent_id);
        if let Some(p) = pty_id {
            self.spawned_at.remove(p);
        }
    }

    /// Why this worker is held right now, or None → nudge. Pure: never
    /// touches the announcement/cooldown memory.
    pub fn explain(&self, f: &WorkerWakeFacts, now: u64) -> Option<&'static str> {
        let inbox_ids = live_ids(f);
        if inbox_ids.is_empty() {
            return Some("no-mail");
        }
        if f.is_god {
            return Some("god");
        }
        if f.pty_id.is_none() {
            return Some("no-pty");
        }
        if f.auto_delivery_paused {
            return Some("delivery-paused");
        }
        if f.paused {
            return Some("paused");
        }
        if f.halted {
            return Some("halted");
        }
        let stalled = self.is_stalled(f, now);
        if f.last_output_at == 0 && !stalled {
            return Some("booting"); // never produced output → still booting
        }
        if now.saturating_sub(f.last_output_at) < WORKER_WAKE_IDLE_MS && !stalled {
            return Some("mid-turn");
        }
        let spawned = *self.spawned_at.get(f.pty_id.as_deref().unwrap_or("")).unwrap_or(&0);
        if spawned > 0 && now.saturating_sub(spawned) < WORKER_WAKE_BOOT_GRACE_MS {
            return Some("boot-grace");
        }
        let last_human = *self.last_human_needs_at.get(&f.agent_id).unwrap_or(&0);
        if last_human > 0 && now.saturating_sub(last_human) < WORKER_WAKE_HITL_REARM_MS {
            return Some("hitl");
        }
        // Edge trigger (#358): already-announced mail is not announced again —
        // unless stalled (the announcement demonstrably never landed).
        if !stalled {
            if let Some(announced) = self.announced_inbox_ids.get(&f.agent_id) {
                if inbox_ids.iter().all(|id| announced.contains(id)) {
                    return Some("announced");
                }
            }
        }
        let last_nudge = *self.last_nudge_at.get(&f.agent_id).unwrap_or(&0);
        if last_nudge > 0 && now.saturating_sub(last_nudge) < WORKER_WAKE_COOLDOWN_MS {
            return Some("cooldown");
        }
        None
    }

    /// The ids to nudge right now — remembers what it announced (edge) and
    /// when (cooldown). A drained inbox forgets the announcement.
    pub fn decide(&mut self, facts: &[WorkerWakeFacts], now: u64) -> Vec<String> {
        let mut out = Vec::new();
        for f in facts {
            let inbox_ids = live_ids(f);
            if inbox_ids.is_empty() {
                self.announced_inbox_ids.remove(&f.agent_id);
                continue;
            }
            if self.explain(f, now).is_some() {
                continue;
            }
            self.last_nudge_at.insert(f.agent_id.clone(), now);
            self.announced_inbox_ids.insert(f.agent_id.clone(), inbox_ids);
            out.push(f.agent_id.clone());
        }
        out
    }

    /// True once per cooldown per worker — the beat's hold-log gate.
    pub fn should_report_hold(&mut self, agent_id: &str, now: u64) -> bool {
        let last = *self.last_hold_report_at.get(agent_id).unwrap_or(&0);
        if last > 0 && now.saturating_sub(last) < WORKER_WAKE_COOLDOWN_MS {
            return false;
        }
        self.last_hold_report_at.insert(agent_id.to_string(), now);
        true
    }
}
