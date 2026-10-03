//! ControlRegistry — port of `src/main/control.ts`.
//!
//! Operator control WITHOUT typing into the PTY: decisions ride the agent
//! CLI's own hook-return protocol.
//!   - pause / gateTool → PreToolUse answers permissionDecision:'deny'
//!   - steer → next UserPromptSubmit/PostToolUse returns additionalContext
//!   - halt → next hook boundary answers { continue:false } — a CLEAN stop
//!     vs killing the PTY; overrides everything else at that boundary.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::Serialize;

/// Cap on queued steer notes per agent — FIFO drop-oldest so the newest
/// instruction always wins even if a stalled agent never drains the queue.
const MAX_PENDING_STEERS: usize = 20;
const MAX_STEER_LEN: usize = 10_000;

#[derive(Default)]
struct AgentControl {
    paused: bool,
    halted: bool,
    auto_delivery_paused: bool,
    gated_tools: HashSet<String>,
    steer_queue: VecDeque<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlSnapshot {
    pub paused: bool,
    pub halted: bool,
    pub auto_delivery_paused: bool,
    pub gated_tools: Vec<String>,
    pub pending_steers: usize,
}

#[derive(Default)]
pub struct ControlRegistry {
    map: HashMap<String, AgentControl>,
}

impl ControlRegistry {
    fn ensure(&mut self, id: &str) -> &mut AgentControl {
        self.map.entry(id.to_string()).or_default()
    }

    // ── operator actions (Tauri commands) ──────────────────────────────────

    pub fn pause(&mut self, id: &str, on: bool) {
        self.ensure(id).paused = on;
    }

    pub fn gate_tool(&mut self, id: &str, tool: &str, on: bool) {
        let c = self.ensure(id);
        if on {
            c.gated_tools.insert(tool.into());
        } else {
            c.gated_tools.remove(tool);
        }
    }

    pub fn steer(&mut self, id: &str, text: &str) {
        let t = text.trim();
        if t.is_empty() {
            return;
        }
        let q = &mut self.ensure(id).steer_queue;
        if q.len() >= MAX_PENDING_STEERS {
            q.pop_front(); // drop the oldest — newest instruction wins
        }
        q.push_back(t.chars().take(MAX_STEER_LEN).collect());
    }

    pub fn halt(&mut self, id: &str) {
        self.ensure(id).halted = true;
    }

    /// Drop all queued-but-undelivered steer notes — API parity with upstream;
    /// the consumer (cancel-before-delivery UI) lands with the control strip.
    #[allow(dead_code)]
    pub fn clear_steers(&mut self, id: &str) {
        if let Some(c) = self.map.get_mut(id) {
            c.steer_queue.clear();
        }
    }

    /// Clear pause + halt (keeps tool gates).
    pub fn resume(&mut self, id: &str) {
        let c = self.ensure(id);
        c.paused = false;
        c.halted = false;
    }

    // ── reads (HookServer) ─────────────────────────────────────────────────

    pub fn should_halt(&self, id: &str) -> bool {
        self.map.get(id).is_some_and(|c| c.halted)
    }

    pub fn pause_auto_delivery(&mut self, id: &str, on: bool) {
        self.ensure(id).auto_delivery_paused = on;
    }

    /// Beat-side read (parity with upstream's public API — the snapshot
    /// covers the same flag for UI consumers).
    #[allow(dead_code)]
    pub fn is_auto_delivery_paused(&self, id: &str) -> bool {
        self.map.get(id).is_some_and(|c| c.auto_delivery_paused)
    }

    pub fn tool_decision(&self, id: &str, tool: &str) -> Option<String> {
        let c = self.map.get(id)?;
        if c.paused {
            return Some("Paused by operator — resume from the floor to continue.".into());
        }
        if !tool.is_empty() && c.gated_tools.contains(tool) {
            return Some(format!("Tool {tool} is gated by the operator."));
        }
        None
    }

    pub fn take_steer(&mut self, id: &str) -> Option<String> {
        self.map.get_mut(id)?.steer_queue.pop_front()
    }

    pub fn snapshot(&self, id: &str) -> ControlSnapshot {
        match self.map.get(id) {
            Some(c) => ControlSnapshot {
                paused: c.paused,
                halted: c.halted,
                auto_delivery_paused: c.auto_delivery_paused,
                gated_tools: c.gated_tools.iter().cloned().collect(),
                pending_steers: c.steer_queue.len(),
            },
            None => ControlSnapshot {
                paused: false,
                halted: false,
                auto_delivery_paused: false,
                gated_tools: vec![],
                pending_steers: 0,
            },
        }
    }
}
