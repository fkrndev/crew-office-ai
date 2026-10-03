//! Closing Time — the graceful, data-loss-free shutdown protocol
//! (port of `closingTime.ts`).
//!
//! Killing PTYs mid-thought loses whatever the agents were holding in working
//! memory. Closing time closes the floor like a real office: the human
//! announces it, every worker packs up and confirms, the manager locks the
//! door. Everything rides the existing hive rails (inbox delivery, steer
//! notes) — this module only injects the kickoff mail and watches routed
//! traffic; it never types into terminals.

use std::collections::HashSet;
use std::sync::Mutex;

/// Subject markers — deliberately forgiving (case, -/_/space) because agents
/// write these by hand.
fn is_ack(subject: &str) -> bool {
    let s = subject.to_uppercase();
    let normalized: String = s.chars().filter(|c| !matches!(c, '-' | '_' | ' ')).collect();
    normalized.starts_with("CLOSINGTIMEACK")
}
fn is_complete(subject: &str) -> bool {
    let s = subject.to_uppercase();
    let normalized: String = s.chars().filter(|c| !matches!(c, '-' | '_' | ' ')).collect();
    normalized.starts_with("CLOSINGTIMECOMPLETE")
}

/// Wait before surfacing "taking long — force quit?" (a compaction or long
/// tool call can hold an ACK for minutes).
pub const TIMEOUT_MS: u64 = 6 * 60_000;
/// Grace after COMPLETE before teardown, so god's final commits land.
pub const TEARDOWN_GRACE_MS: u64 = 2_500;

struct Active {
    god_id: String,
    workers: HashSet<String>,
    acked: HashSet<String>,
    deadline_ms: u64,
    /// Emitted once; a re-press re-arms (start() while active).
    timeout_emitted: bool,
    /// COMPLETE honored — teardown after the grace.
    concluded: bool,
}

#[derive(Default)]
pub struct ClosingTime {
    inner: Mutex<Option<Active>>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl ClosingTime {
    pub fn is_active(&self) -> bool {
        self.inner.lock().unwrap().is_some()
    }

    pub fn progress(&self) -> (usize, usize) {
        self.inner
            .lock()
            .unwrap()
            .as_ref()
            .map(|a| (a.acked.len(), a.workers.len()))
            .unwrap_or((0, 0))
    }

    /// Kick off the protocol — mails god the shutdown brief + steers every
    /// live agent (the deeply-busy rail). Returns Err when no live god can
    /// collect the reports (the UI falls back to hard quit).
    pub fn start(
        &self,
        hive: &crate::hive::Hive,
        control: &mut crate::control::ControlRegistry,
        live_ids: &HashSet<String>,
    ) -> Result<usize, String> {
        let god_id = hive.god_id().unwrap_or_else(|| "god".to_string());
        {
            let mut g = self.inner.lock().unwrap();
            if let Some(a) = g.as_mut() {
                // Re-pressed while running — re-arm, keep waiting.
                a.deadline_ms = now_ms() + TIMEOUT_MS;
                a.timeout_emitted = false;
                return Ok(a.workers.len());
            }
        }
        let reg = hive.registry();
        if !live_ids.contains(&god_id) || !reg.iter().any(|a| a.id == god_id) {
            return Err("No orchestrator is running — closing time needs the god agent to collect the reports.".into());
        }
        // Only agents with a live terminal are waited on — the registry alone
        // would wait on ghosts that can never ACK.
        let workers: HashSet<String> = live_ids
            .iter()
            .filter(|id| id.as_str() != god_id && reg.iter().any(|a| a.id == id.as_str() && !a.is_god))
            .cloned()
            .collect();
        let names = workers
            .iter()
            .map(|id| {
                format!(
                    "{} ({})",
                    reg.iter().find(|a| &a.id == id).map(|a| a.name.as_str()).unwrap_or(id),
                    id
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let names = if names.is_empty() { "(none — the floor is just you)".into() } else { names };

        let _ = hive.send_virtual(
            "human",
            "god",
            "request",
            "CLOSING TIME — run the shutdown protocol now",
            &format!(
                "The human pressed \"closing time\": the harness will close as soon as you confirm the floor is safe. Run this protocol now, before anything else:\n\
                 \n\
                 1. BROADCAST closing time to the team (message with \"to\":\"broadcast\"). Current workers: {names}.\n   \
                 Tell each worker to immediately: park or commit any work-in-progress safely, append its current state + concrete next steps to its memory.md, and then reply to you with a message whose subject is exactly \"CLOSING-TIME-ACK\".\n\
                 2. WAIT and keep draining your inbox until EVERY worker above has sent its CLOSING-TIME-ACK. Nudge stragglers once if needed.\n\
                 3. Save your own state: update board.md and append your shift summary to your memory.md.\n\
                 4. CONCLUDE by sending a message with \"to\":\"human\" and the subject exactly \"CLOSING-TIME-COMPLETE\" — the harness watches for it and closes the app. Do not send it before every worker has acked: the harness independently verifies the ACKs and will reject a premature conclusion.\n\
                 \n\
                 {}\
                 This is a shutdown: do not start new work and do not accept new tasks.",
                if workers.is_empty() {
                    "There are no workers on the floor right now — do steps 3 and 4 immediately.\n"
                } else {
                    "The prep assistant saves its own memory separately — do NOT wait for it and do not message it.\n"
                }
            ),
        );

        // Graceful interrupt (#7C.2): inbox mail only lands at the next Stop —
        // a steer note rides the next hook boundary instead.
        control.steer(
            &god_id,
            "CLOSING TIME was pressed by the human: pause your current work at the next sensible point and drain your inbox NOW — a shutdown brief is waiting there. Coordinate the floor shutdown before anything else.",
        );
        for id in &workers {
            control.steer(
                id,
                "CLOSING TIME — the office is shutting down. Finish your current step but do NOT start new work. Park or commit your work-in-progress safely, append your current state + concrete next steps to your memory.md, then reply to god with a message whose subject is exactly \"CLOSING-TIME-ACK\".",
            );
        }

        *self.inner.lock().unwrap() = Some(Active {
            god_id,
            workers,
            acked: HashSet::new(),
            deadline_ms: now_ms() + TIMEOUT_MS,
            timeout_emitted: false,
            concluded: false,
        });
        Ok(self.inner.lock().unwrap().as_ref().map(|a| a.workers.len()).unwrap_or(0))
    }

    /// Stand the floor back up — clear pending steers and tell god.
    pub fn cancel(&self, hive: &crate::hive::Hive, control: &mut crate::control::ControlRegistry) {
        let Some(a) = self.inner.lock().unwrap().take() else { return };
        control.clear_steers(&a.god_id);
        for id in &a.workers {
            control.clear_steers(id);
        }
        let _ = hive.send_virtual(
            "human",
            "god",
            "inform",
            "CLOSING TIME CANCELLED",
            "The human cancelled the shutdown — disregard the closing-time protocol and resume normal operation. Any memory saves already done are a bonus, not a problem.",
        );
    }

    /// Router observer — one routed message. Returns Some(phase) when the
    /// state advanced ("progress" | "conclusion-rejected" | "complete").
    pub fn on_routed(
        &self,
        hive: &crate::hive::Hive,
        live_ids: &HashSet<String>,
        from: &str,
        subject: &str,
        targets: &[String],
    ) -> Option<&'static str> {
        let mut g = self.inner.lock().unwrap();
        let a = g.as_mut()?;
        if is_ack(subject) && a.workers.contains(from) && targets.contains(&a.god_id) {
            if a.acked.insert(from.to_string()) {
                return Some("progress");
            }
            return None;
        }
        if is_complete(subject) && from == a.god_id {
            // Trust but VERIFY: a premature COMPLETE must not close the floor.
            // Dead workers are excused — their ACK can never arrive.
            let reg = hive.registry();
            let pending: Vec<String> = a
                .workers
                .iter()
                .filter(|id| {
                    !a.acked.contains(*id)
                        && live_ids.contains(*id)
                        && reg.iter().any(|r| &r.id == *id)
                })
                .cloned()
                .collect();
            if !pending.is_empty() {
                let names = pending.join(", ");
                let _ = hive.send_virtual(
                    "human",
                    "god",
                    "refuse",
                    "CLOSING TIME — conclusion rejected, workers still missing",
                    &format!(
                        "The harness is still missing a CLOSING-TIME-ACK from: {names}.\n\
                         The app stays open until every worker has confirmed its memory is saved.\n\
                         Chase the stragglers (re-send the closing-time instruction to each), wait for their ACKs, then send CLOSING-TIME-COMPLETE again."
                    ),
                );
                return Some("conclusion-rejected");
            }
            a.concluded = true;
            return Some("complete");
        }
        None
    }

    /// Tick check — Some("timeout") once past the deadline (until re-armed).
    pub fn poll_timeout(&self) -> Option<&'static str> {
        let mut g = self.inner.lock().unwrap();
        let a = g.as_mut()?;
        if !a.timeout_emitted && now_ms() >= a.deadline_ms {
            a.timeout_emitted = true;
            return Some("timeout");
        }
        None
    }

    /// The concluded flag — lib.rs schedules teardown after the grace.
    pub fn concluded(&self) -> bool {
        self.inner.lock().unwrap().as_ref().is_some_and(|a| a.concluded)
    }
}
