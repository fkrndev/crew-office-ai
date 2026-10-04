//! Scheduled missions â€” port of the scheduler half of `index.ts` (syncMissions
//! + fire) and the mission constants of `config.ts`.
//!
//! A recurring auto-dispatched mission: when `now - last_fired_at >=
//! interval_ms`, fire â€” a `kind:'compact'` mission emits the compact trigger
//! instead of mailing (its `to`/`body` are empty), every other kind mails
//! `body` to `to` from the virtual 'scheduler' sender. `kind:'heartbeat'` is
//! additionally quiet-gated: it only fires when the floor has been silent for
//! `quiet_threshold_ms` (log.jsonl + mailbox mtimes + PTY output â€” the same
//! file/activity signals as upstream's isFloorQuiet, never registry.status).
//!
//! Deviation: upstream arms per-mission setTimeout/interval handles and
//! re-arms on every config save. This port polls every 15s â€” same due-time
//! semantics, no re-arm plumbing, and a config edit takes effect on the next
//! tick for free. Weekly schedules port the full upstream shared/weeklySchedule.ts contract below..

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
    /// shared/weeklySchedule.ts WeeklySchedule {days:[0..6], minute:0..1439}
    /// â€” when present and valid it REPLACES the interval cadence.
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
               is still running (not stalled or idle-stale). Check the task board â€” are \
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
/// PTY â€” the user turns it on explicitly (upstream ships it off too).
pub fn heartbeat() -> ScheduledMission {
    ScheduledMission {
        id: "heartbeat".into(),
        label: "Floor heartbeat".into(),
        interval_ms: 120_000,
        to: "god".into(),
        body: "Floor heartbeat: the team has gone quiet. Review the digest in your inbox, \
               re-engage anyone stalled or blocked, and keep the board accurate â€” or rest \
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
    // On an ambiguous/nonexistent local time (DST fold/gap) .earliest() is
    // the platform-standard resolution the upstream constructor also gives.
    match naive.and_local_timezone(chrono::Local).earliest() {
        Some(dt) => dt.timestamp_millis().max(0) as u64,
        None => 0,
    }
}

/// nextWeeklyFireMs — first matching slot STRICTLY after now.
fn next_weekly_fire_ms(w: Weekly, now_ms: u64) -> Option<u64> {
    let from = chrono::DateTime::<chrono::Local>::from(
        chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now_ms as i64)?,
    );
    for offset in 0..=7i64 {
        let slot = slot_at_ms(from, offset, w.minute);
        let wd = (chrono::Datelike::weekday(
            &(chrono::DateTime::<chrono::Utc>::from_timestamp_millis(slot as i64)?)
                .with_timezone(&chrono::Local)
                .naive_local()
                .date(),
        ))
        .num_days_from_sunday() as usize;
        if !w.days[wd] {
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
    for offset in (0..=7i64).rev().map(|o| -o) {
        let slot = slot_at_ms(from, offset, w.minute);
        let wd = (chrono::Datelike::weekday(
            &(chrono::DateTime::<chrono::Utc>::from_timestamp_millis(slot as i64)?)
                .with_timezone(&chrono::Local)
                .naive_local()
                .date(),
        ))
        .num_days_from_sunday() as usize;
        if !w.days[wd] {
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
