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
//! tick for free. Weekly schedules port the full upstream shared/weeklySchedule.ts contract below..

use crate::lock_ext::LockExt;
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
    /// 'heartbeat' | 'compact' | 'shardx' | None(regular) — 'shardx' runs a
    /// launcher automation project (`body` = project id) instead of mailing.
    pub kind: Option<String>,
    pub quiet_threshold_ms: Option<u64>,
    #[serde(default)]
    pub auto_compact: bool,
    /// shared/weeklySchedule.ts WeeklySchedule {days:[0..6], minute:0..1439}
    /// — when present and valid it REPLACES the interval cadence.
    pub weekly: Option<serde_json::Value>,
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
            weekly: None,
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
        weekly: None,
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
        weekly: None,
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
        weekly: None,
        last_fired_at: 0,
    }
}

// --- weeklySchedule.ts -------------------------------------------------------
//
// "every Monday and Thursday at 09:00" — an interval cannot say that, so a
// mission carrying a valid `weekly` schedule fires on calendar slots instead.
// ALL times are LOCAL, built from calendar fields so a DST boundary lands on
// the same wall-clock slot, never an hour off (upstream's deliberate choice).

/// 6h — a slot MISSED this long ago still fires (laptop asleep at 09:00,
/// opened 10:30); one from Friday when opened Monday does not.
const WEEKLY_CATCHUP_MS: u64 = 6 * 60 * 60 * 1000;

/// normalizeWeekly — null for anything that is not a usable schedule: no
/// days, a day outside 0..6, or a minute outside the day.
#[derive(Clone, Copy)]
struct Weekly {
    days: [bool; 7],
    minute: u32,
}

fn normalize_weekly(w: &serde_json::Value) -> Option<Weekly> {
    let days_raw = w.get("days")?.as_array()?;
    let mut days = [false; 7];
    let mut n = 0;
    for d in days_raw {
        let d = d.as_u64()?;
        if d > 6 {
            return None;
        }
        if !days[d as usize] {
            days[d as usize] = true;
            n += 1;
        }
    }
    if n == 0 {
        return None;
    }
    let minute = w.get("minute")?.as_u64()? as u32;
    if minute > 1439 {
        return None;
    }
    Some(Weekly { days, minute })
}

/// Build the local instant for `minute` on the date `offset` days from `from`
/// — chrono's Local calendar fields do the DST roll the same way the JS
/// `new Date(y, m, d, h, min)` constructor does.
fn slot_at_ms(from: chrono::DateTime<chrono::Local>, offset: i64, minute: u32) -> u64 {
    let date = from.date_naive() + chrono::Duration::days(offset);
    let naive = date
        .and_hms_opt(minute / 60, minute % 60, 0)
        .unwrap_or_else(|| date.and_hms_opt(0, 0, 0).unwrap());
    // An ambiguous (DST fold) local time resolves to .earliest(), the same
    // as the upstream constructor. A NONEXISTENT one (spring-forward gap)
    // rolls forward past the gap — JS `new Date(y,m,d,2,30)` lands on 03:30 —
    // rather than collapsing to epoch 0, which would make every weekly slot
    // on that day look "missed ~55 years ago".
    for shift_min in [0i64, 60, 30, 120, 180] {
        let shifted = naive + chrono::Duration::minutes(shift_min);
        if let Some(dt) = shifted.and_local_timezone(chrono::Local).earliest() {
            return dt.timestamp_millis().max(0) as u64;
        }
    }
    // Unreachable for real zones; keep it non-zero and monotonic anyway.
    naive.and_utc().timestamp_millis().max(0) as u64
}

/// Weekday (0 = Sunday) of the CALENDAR date `offset` days from `from` — the
/// slot's own date, not the date its rolled-forward instant lands on.
fn slot_weekday(from: chrono::DateTime<chrono::Local>, offset: i64) -> usize {
    use chrono::Datelike;
    (from.date_naive() + chrono::Duration::days(offset))
        .weekday()
        .num_days_from_sunday() as usize
}

/// nextWeeklyFireMs — first matching slot STRICTLY after now.
fn next_weekly_fire_ms(w: Weekly, now_ms: u64) -> Option<u64> {
    let from = chrono::DateTime::<chrono::Local>::from(
        chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now_ms as i64)?,
    );
    for offset in 0..=7i64 {
        let slot = slot_at_ms(from, offset, w.minute);
        if !w.days[slot_weekday(from, offset)] {
            continue;
        }
        if slot > now_ms {
            return Some(slot);
        }
    }
    None
}

/// previousWeeklyFireMs — most recent matching slot at or before now.
fn previous_weekly_fire_ms(w: Weekly, now_ms: u64) -> Option<u64> {
    let from = chrono::DateTime::<chrono::Local>::from(
        chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now_ms as i64)?,
    );
    // Most recent first: today (0), yesterday (-1), … -7. (A reversed range
    // here walks oldest-first and returns a week-old slot, so weekly
    // missions never fired.)
    for offset in (0..=7i64).map(|o| -o) {
        let slot = slot_at_ms(from, offset, w.minute);
        if !w.days[slot_weekday(from, offset)] {
            continue;
        }
        if slot <= now_ms {
            return Some(slot);
        }
    }
    None
}

/// weeklyDelayMs — upstream semantics: 0 means "fire now" (a recent missed
/// slot we have not run yet); otherwise ms until the next slot; None when the
/// schedule is unusable. `last_fired` stops a scheduler re-arm from re-firing
/// a slot that already ran minutes ago.
pub fn weekly_delay_ms(w: &serde_json::Value, now_ms: u64, last_fired: u64) -> Option<u64> {
    let w = normalize_weekly(w)?;
    if let Some(prev) = previous_weekly_fire_ms(w, now_ms) {
        if prev > last_fired && now_ms.saturating_sub(prev) <= WEEKLY_CATCHUP_MS {
            return Some(0);
        }
    }
    next_weekly_fire_ms(w, now_ms).map(|t| t.saturating_sub(now_ms))
}

/// Is this mission due right now? Interval cadence or weekly slots — a valid
/// weekly schedule REPLACES the interval (upstream rule).
/// Upstream ignores `weekly` on heartbeat missions (they self-cadence).
pub fn is_mission_due(m: &ScheduledMission, now_ms: u64) -> bool {
    if m.kind.as_deref() != Some("heartbeat") {
        if let Some(w) = &m.weekly {
            if normalize_weekly(w).is_some() {
                return weekly_delay_ms(w, now_ms, m.last_fired_at) == Some(0);
            }
        }
    }
    m.interval_ms > 0 && now_ms.saturating_sub(m.last_fired_at) >= m.interval_ms
}

// --- heartbeat beat (index.ts armHeartbeat) ---------------------------------
//
// Upstream's heartbeat is NOT a plain interval mission: it self-reschedules
// with an adaptive cadence ? base normally, base*2.5 right after a
// re-engage, max(30s, base/4) when an agent looks wedged. It re-engages god
// when the floor is quiet OR god has actionable (non-system) inbox mail ?
// a worker's reply must not sit unread just because other agents keep the
// floor loud. The beat ALWAYS stamps lastFiredAt, even when it doesn't
// re-engage ? the stamp is what the re-arm math hangs off.

/// SYSTEM_SENDERS ? the scheduler's own noise never wakes god.
const SYSTEM_SENDERS: &[&str] = &["heartbeat", "scheduler", "breaker", "system"];

/// godActionableInboxCount ? unread non-system mail in god's inbox.
pub fn god_actionable_inbox_count(hive: &crate::hive::Hive) -> usize {
    let Some(god) = hive.god_id() else {
        return 0;
    };
    hive.inbox(&god)
        .map(|msgs| {
            msgs.iter()
                .filter(|m| !SYSTEM_SENDERS.contains(&m.from.as_str()))
                .count()
        })
        .unwrap_or(0)
}

/// buildHeartbeatDigest ? the bounded digest (paths + counts, never full
/// files) dropped into god's inbox when the floor needs a nudge.
pub fn heartbeat_digest(hive: &crate::hive::Hive, quiet_ms: u64, actionable: usize) -> String {
    let reg = hive.registry();
    let god = hive.god_id();
    let active: Vec<_> = reg
        .iter()
        .filter(|a| !a.archived && Some(&a.id) != god.as_ref())
        .collect();
    let names = active
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let names = if names.is_empty() {
        "?".to_string()
    } else {
        names
    };
    let board_head = hive
        .read_board()
        .unwrap_or_default()
        .lines()
        .take(10)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    let log = crate::hive_admin::hive_log(hive, 8)
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let with_inbox: Vec<&str> = active
        .iter()
        .filter(|a| hive.inbox_backlog(&a.id) > 0)
        .map(|a| a.name.as_str())
        .collect();
    let header = if actionable > 0 {
        format!("Floor heartbeat ? {actionable} actionable inbox message(s) awaiting you (worker/human mail). Drain your inbox NOW and act on them.")
    } else {
        format!("Floor heartbeat ? quiet ~{}m.", quiet_ms / 60_000)
    };
    let inbox_line = if with_inbox.is_empty() {
        "No undrained inboxes.".to_string()
    } else {
        format!("Undrained inbox: {}.", with_inbox.join(", "))
    };
    format!(
        "{header}\nActive agents ({}): {names}.\n{inbox_line}\n\nBoard (head):\n{}\n\nRecent log:\n{}\n\nRe-engage anyone stalled or blocked and keep the board accurate — or rest if the work is genuinely done.",
        active.len(),
        if board_head.is_empty() { "(empty)" } else { board_head.as_str() },
        if log.is_empty() { "(none)" } else { log.as_str() }
    )
}

/// looksStuck (index.ts) ? an agent printing PTY output <15s ago whose
/// coordination files have not moved in `window_ms`: it is busy but not
/// coordinating, so the heartbeat tightens its cadence to watch it.
pub fn looks_stuck(st: &crate::commands::AppState, window_ms: u64, now: u64) -> bool {
    let reg = st.hive.registry();
    let god = st.hive.god_id();
    // Registry ids are agent ids; the PTY map is pty-keyed — snapshot the
    // mapping up front rather than locking it inside the ptys hold.
    let pty_for = st.agent_to_pty.lock_safe().clone();
    let ptys = st.ptys.lock_safe();
    for a in &reg {
        if a.archived || Some(&a.id) == god.as_ref() {
            continue;
        }
        let Some(h) = pty_for.get(&a.id).and_then(|pid| ptys.get(pid)) else {
            continue;
        };
        let idle = now.saturating_sub(h.last_output_at.load(std::sync::atomic::Ordering::Relaxed));
        if idle < 15_000 && now.saturating_sub(st.hive.last_coordination_at(&a.id)) > window_ms {
            return true;
        }
    }
    false
}

/// One heartbeat beat ? returns the delay until the NEXT beat, mirroring
/// upstream's adaptive cadence exactly.
pub fn heartbeat_beat(st: &crate::commands::AppState, m: &ScheduledMission, now: u64) -> u64 {
    let base = m.interval_ms;
    let quiet = m.quiet_threshold_ms.unwrap_or(300_000);
    let actionable = god_actionable_inbox_count(&st.hive);
    let clocks: Vec<u64> = st
        .ptys
        .lock_safe()
        .values()
        .map(|h| h.last_output_at.load(std::sync::atomic::Ordering::Relaxed))
        .collect();
    let mut next = base;
    if st.hive.is_floor_quiet(quiet, &clocks) || actionable > 0 {
        let digest = heartbeat_digest(&st.hive, quiet, actionable);
        let to = if m.to.is_empty() {
            "god"
        } else {
            m.to.as_str()
        };
        let _ = st
            .hive
            .send_virtual("heartbeat", to, "request", "Heartbeat", &digest);
        next = base * 5 / 2; // back off after re-engaging
    } else if looks_stuck(st, quiet, now) {
        next = (base / 4).max(30_000); // tighten when an agent is wedged
    }
    next
}

/// ms until this mission is next due ? the arm-side of upstream's
/// `setTimeout(remaining)`. Heartbeat rides its adaptive cadence map;
/// weekly rides the slot math; everything else rides the interval.
pub fn mission_due_ms(
    m: &ScheduledMission,
    now: u64,
    heartbeat_cadence: &std::collections::HashMap<String, u64>,
) -> Option<u64> {
    if !m.enabled {
        return None;
    }
    if m.kind.as_deref() == Some("heartbeat") {
        let c = heartbeat_cadence
            .get(&m.id)
            .copied()
            .unwrap_or(m.interval_ms);
        return Some(m.last_fired_at.saturating_add(c).saturating_sub(now));
    }
    if let Some(w) = &m.weekly {
        if let Some(d) = weekly_delay_ms(w, now, m.last_fired_at) {
            return Some(d);
        }
    }
    if m.interval_ms > 0 {
        return Some(
            m.last_fired_at
                .saturating_add(m.interval_ms)
                .saturating_sub(now),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// kind:'shardx' is a plain cadence mission — no heartbeat adaptivity,
    /// no weekly special-casing. Interval elapses -> due; not yet -> not.
    #[test]
    fn shardx_mission_uses_plain_interval() {
        let m = ScheduledMission {
            id: "sx".into(),
            label: "replay".into(),
            interval_ms: 60_000,
            to: "shardx".into(),
            body: "proj-1".into(),
            enabled: true,
            kind: Some("shardx".into()),
            ..Default::default()
        };
        assert!(!is_mission_due(&m, 30_000));
        assert!(is_mission_due(&m, 61_000));
        // And the "when" math agrees — next fire is last+interval.
        let no_cadence = std::collections::HashMap::new();
        assert_eq!(mission_due_ms(&m, 0, &no_cadence), Some(60_000));
        // Disabled never fires — replay must not run behind a dead switch.
        // (is_mission_due leaves enabled-filtering to the caller loop; the
        // due_ms form is what the loop actually consults.)
        let off = ScheduledMission { enabled: false, ..m };
        assert_eq!(mission_due_ms(&off, 999_999, &no_cadence), None);
    }

    /// Local wall-clock instant → epoch ms (2024-01-01 is a Monday; early
    /// January is DST-free in every zone this test could plausibly run in).
    fn local_ms(day: u32, h: u32, m: u32, s: u32) -> u64 {
        use chrono::TimeZone;
        chrono::Local
            .with_ymd_and_hms(2024, 1, day, h, m, s)
            .earliest()
            .unwrap()
            .timestamp_millis() as u64
    }

    fn weekly(days: &[u32], minute: u32) -> ScheduledMission {
        ScheduledMission {
            id: "w".into(),
            to: "god".into(),
            enabled: true,
            weekly: Some(serde_json::json!({ "days": days, "minute": minute })),
            ..Default::default()
        }
    }

    #[test]
    fn weekly_monday_slot_fires_just_after_and_last_fired_suppresses() {
        let m = weekly(&[1], 9 * 60);
        // Mon 09:00:05 — the 09:00 slot is 5s old and unfired → due.
        let now = local_ms(1, 9, 0, 5);
        assert!(is_mission_due(&m, now));
        let no_cadence = std::collections::HashMap::new();
        assert_eq!(mission_due_ms(&m, now, &no_cadence), Some(0));
        // Already fired at the slot → not due; next slot is a week out.
        let fired = ScheduledMission { last_fired_at: local_ms(1, 9, 0, 1), ..m.clone() };
        assert!(!is_mission_due(&fired, now));
        let wait = mission_due_ms(&fired, now, &no_cadence).unwrap();
        assert!(wait > 6 * 24 * 3_600_000 && wait <= 7 * 24 * 3_600_000, "{wait}");
        // Tuesday noon: the Monday slot is > 6h old → no catch-up, not due.
        assert!(!is_mission_due(&m, local_ms(2, 12, 0, 0)));
    }

    #[test]
    fn weekly_previous_slot_is_most_recent_not_oldest() {
        let w = normalize_weekly(&serde_json::json!({ "days": [1, 4], "minute": 540 })).unwrap();
        // Fri Jan 5 12:00 → most recent matching slot is Thu Jan 4 09:00.
        assert_eq!(
            previous_weekly_fire_ms(w, local_ms(5, 12, 0, 0)),
            Some(local_ms(4, 9, 0, 0))
        );
        // Thu Jan 4 10:00 → today's 09:00 (offset 0), not Monday's.
        assert_eq!(
            previous_weekly_fire_ms(w, local_ms(4, 10, 0, 0)),
            Some(local_ms(4, 9, 0, 0))
        );
    }

    #[test]
    fn slot_never_collapses_to_epoch() {
        use chrono::TimeZone;
        // Whatever zone the host uses, every minute of a day resolves to a
        // real instant (the DST-gap path used to return 0).
        let from = chrono::Local.with_ymd_and_hms(2024, 3, 10, 12, 0, 0).earliest().unwrap();
        for minute in (0..1440).step_by(15) {
            assert!(slot_at_ms(from, 0, minute) > 1_000_000_000_000);
        }
    }
}

