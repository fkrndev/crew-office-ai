//! Scheduled missions — port of the scheduler half of `index.ts` (syncMissions
//! + fire) and the mission constants of `config.ts`.
//!
//! A recurring auto-dispatched mission: when `now - last_fired_at >=
//! interval_ms`, fire — a `kind:'compact'` mission emits the compact trigger
//! instead of mailing (its `to`/`body` are empty), every other kind mails
//! `body` to `to` from the virtual 'scheduler' sender. `kind:'heartbeat'` is
//! additionally quiet-gated: it only fires when the floor has been silent for
//! `quiet_threshold_ms` (log.jsonl + mailbox mtimes + PTY output — the same
//! file/activity signals as upstream's isFloorQuiet, never registry.status).
//!
//! Deviation: upstream arms per-mission setTimeout/interval handles and
//! re-arms on every config save. This port polls every 15s — same due-time
//! semantics, no re-arm plumbing, and a config edit takes effect on the next
//! tick for free. Weekly schedules are a TODO (upstream shared/weeklySchedule).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ScheduledMission {
    pub id: String,
    pub label: String,
    pub interval_ms: u64,
    pub to: String,
    pub body: String,
    pub enabled: bool,
    /// 'heartbeat' | 'compact' | None(regular)
    pub kind: Option<String>,
    pub quiet_threshold_ms: Option<u64>,
    #[serde(default)]
    pub auto_compact: bool,
    pub last_fired_at: u64,
}

impl Default for ScheduledMission {
    fn default() -> Self {
        Self {
            id: String::new(),
            label: String::new(),
            interval_ms: 0,
            to: String::new(),
            body: String::new(),
            enabled: false,
            kind: None,
            quiet_threshold_ms: None,
            auto_compact: false,
            last_fired_at: 0,
        }
    }
}

pub fn ops_standup() -> ScheduledMission {
    ScheduledMission {
        id: "ops-standup".into(),
        label: "Hourly ops standup".into(),
        interval_ms: 3_600_000,
        to: "god".into(),
        body: "Hourly ops standup. Review every agent: who is doing what, and confirm each \
               is still running (not stalled or idle-stale). Check the task board — are \
               in-flight tasks on track, and is anything blocked or unowned? Flag stale \
               items and re-engage whoever needs it."
            .into(),
        enabled: true,
        kind: None,
        quiet_threshold_ms: None,
        auto_compact: false,
        last_fired_at: 0,
    }
}

/// Shipped DISABLED (opt-in): unlike the standup, the heartbeat nudges god's
/// PTY — the user turns it on explicitly (upstream ships it off too).
pub fn heartbeat() -> ScheduledMission {
    ScheduledMission {
        id: "heartbeat".into(),
        label: "Floor heartbeat".into(),
        interval_ms: 120_000,
        to: "god".into(),
        body: "Floor heartbeat: the team has gone quiet. Review the digest in your inbox, \
               re-engage anyone stalled or blocked, and keep the board accurate — or rest \
               if the work is genuinely done."
            .into(),
        enabled: false,
        kind: Some("heartbeat".into()),
        quiet_threshold_ms: Some(300_000),
        auto_compact: false,
        last_fired_at: 0,
    }
}

/// maint-1: dedicated auto-compact schedule. `kind:'compact'` means the fire
/// path emits the compact trigger instead of dispatching a message.
pub fn compact_maintenance() -> ScheduledMission {
    ScheduledMission {
        id: "compact-maintenance".into(),
        label: "Auto-compact (maintenance)".into(),
        interval_ms: 7_200_000,
        to: String::new(),
        body: String::new(),
        enabled: false,
        kind: Some("compact".into()),
        auto_compact: true,
        quiet_threshold_ms: None,
        last_fired_at: 0,
    }
}
