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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_gates_every_tool() {
        let mut r = ControlRegistry::default();
        assert_eq!(r.tool_decision("a1", "Bash"), None); // untouched agent is free
        r.pause("a1", true);
        assert!(r.tool_decision("a1", "Bash").is_some());
        // Paused decision rides for ANY tool, not just gated ones.
        assert!(r.tool_decision("a1", "").is_some());
        r.pause("a1", false);
        assert_eq!(r.tool_decision("a1", "Bash"), None);
    }

    #[test]
    fn gate_tool_blocks_only_named_tools() {
        let mut r = ControlRegistry::default();
        r.gate_tool("a1", "Bash", true);
        assert!(r.tool_decision("a1", "Bash").is_some());
        assert_eq!(r.tool_decision("a1", "Edit"), None);
        r.gate_tool("a1", "Bash", false);
        assert_eq!(r.tool_decision("a1", "Bash"), None);
    }

    #[test]
    fn steer_queue_is_fifo_and_capped() {
        let mut r = ControlRegistry::default();
        r.steer("a1", "first");
        r.steer("a1", "second");
        assert_eq!(r.take_steer("a1").as_deref(), Some("first"));
        assert_eq!(r.take_steer("a1").as_deref(), Some("second"));
        assert_eq!(r.take_steer("a1"), None);
        // Empty / whitespace steer is dropped silently.
        r.steer("a1", "   ");
        assert_eq!(r.snapshot("a1").pending_steers, 0);
        // Overflow drops the OLDEST — newest instruction wins.
        for i in 0..25 {
            r.steer("a1", &format!("n{i}"));
        }
        assert_eq!(r.snapshot("a1").pending_steers, MAX_PENDING_STEERS);
        assert_eq!(r.take_steer("a1").as_deref(), Some("n5"));
    }

    #[test]
    fn steer_text_is_length_capped() {
        let mut r = ControlRegistry::default();
        r.steer("a1", &"x".repeat(MAX_STEER_LEN + 500));
        assert_eq!(r.take_steer("a1").unwrap().chars().count(), MAX_STEER_LEN);
    }

    #[test]
    fn halt_then_resume_keeps_tool_gates() {
        let mut r = ControlRegistry::default();
        r.gate_tool("a1", "Bash", true);
        r.pause("a1", true);
        r.halt("a1");
        assert!(r.should_halt("a1"));
        r.resume("a1");
        assert!(!r.should_halt("a1"));
        assert!(r.tool_decision("a1", "Bash").is_some()); // gate survives resume
        assert_eq!(r.tool_decision("a1", "Edit"), None); // pause cleared
    }

    #[test]
    fn snapshot_of_unknown_agent_is_all_clear() {
        let r = ControlRegistry::default();
        let s = r.snapshot("ghost");
        assert!(!s.paused && !s.halted && s.gated_tools.is_empty() && s.pending_steers == 0);
    }
}

