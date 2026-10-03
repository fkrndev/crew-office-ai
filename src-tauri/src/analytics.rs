//! Product analytics (PostHog) — anonymous, allowlisted, opt-out.
//! Port of `src/main/analytics.ts`; the contract is upstream's TELEMETRY.md.
//!
//! This is the OUTBOUND product-usage counterpart to telemetry.rs (which is the
//! loopback-only OTel collector and never leaves the machine).
//!
//! 🔒 Anonymity is BY CONSTRUCTION, same as upstream:
//!   - Every event carries `$process_person_profile: false` → PostHog stores it
//!     as an anonymous event and never creates a person profile.
//!   - The distinct id is a RANDOM UUID minted at first send and stored in the
//!     persist kv (`analytics_distinct_id`) — not a machine id, not derivable
//!     from anything, gone when the app's data is deleted. (Upstream stored it
//!     in a `telemetry-install-id` file — the kv entry is the same contract.)
//!   - `track()` enforces a per-event property ALLOWLIST (EVENTS below), so a
//!     future call site cannot leak a new property by accident: unknown events
//!     and unknown keys are dropped, never sent.
//!   - No prompts, transcripts, file paths, repo names, hostnames, or agent
//!     output — nothing free-form crosses this seam.
//!
//! Sending is gated on ALL of (checked in this order):
//!   1. A build-time key (POSTHOG_KEY env → option_env!, injected in release
//!      CI). Dev builds and forks compile with '' → this whole module is a
//!      silent no-op for them.
//!   2. The DO_NOT_TRACK env convention (any value except '' / '0') — respected
//!      unconditionally, checked at every send so it can't be raced.
//!   3. The user's opt-in (`analytics_opt_out` persist kv, Settings → Privacy;
//!      default ON, i.e. opt-out — flipped live via analytics_set_opt_in).

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde_json::{json, Map, Value};
use tauri::AppHandle;

use crate::commands::state;

// Injected at BUILD time from POSTHOG_KEY / POSTHOG_HOST env, mirroring
// upstream's electron-vite `define` of __POSTHOG_KEY__ / __POSTHOG_HOST__.
// Empty in dev/fork builds.
const POSTHOG_KEY: &str = match option_env!("POSTHOG_KEY") {
    Some(k) => k,
    None => "",
};
const POSTHOG_HOST: &str = match option_env!("POSTHOG_HOST") {
    Some(h) => h,
    None => "https://us.i.posthog.com",
};

/// persist kv keys (upstream used files in userData; the kv table is the port's
/// userData equivalent — deleted with the app's data, same contract).
const KV_OPT_OUT: &str = "analytics_opt_out";
const KV_DISTINCT_ID: &str = "analytics_distinct_id";
const KV_LAST_VERSION: &str = "analytics_last_version";

const CAPTURE_TIMEOUT: Duration = Duration::from_secs(15);

/// The complete event → allowed-property-keys contract. Mirrors TELEMETRY.md
/// exactly; `track()` refuses anything outside it. Common props (`app_version`,
/// `os`, `arch`) are stamped centrally and are not listed per event.
pub fn event_allowlist(event: &str) -> Option<&'static [&'static str]> {
    Some(match event {
        // Once per install, when the install id is first minted.
        "first_run" => &[],
        // Every app start. The DAU/retention backbone.
        "app_launched" => &[],
        // Once per version change, on the first run after the version moved.
        "update_applied" => &["from_version", "to_version", "via"],
        // An agent PTY spawned. `provider` is the CLI engine name only.
        "agent_spawned" => &["provider"],
        // ── The activation funnel (v0.4.6): app_launched →
        // onboarding_completed → agent_spawn_attempted → {agent_spawned |
        // agent_spawn_failed | agent_install_started → agent_install_finished}.
        "onboarding_completed" => &["provider"],
        "agent_spawn_attempted" => &["provider"],
        // `reason` is a fixed enum: cli_missing | cwd_missing |
        // already_running | spawn_error.
        "agent_spawn_failed" => &["provider", "reason"],
        // `rung`: npm | node-then-npm | native.
        "agent_install_started" => &["provider", "rung"],
        // `outcome`: agent_launched | install_failed.
        "agent_install_finished" => &["provider", "rung", "outcome"],
        // A HUMAN sent a message to an agent — a count and nothing else;
        // `surface` is a closed enum (MESSAGE_SURFACES).
        "message_sent" => &["surface"],
        // `feature` is a fixed enum (FEATURES), at most once per feature per
        // app session.
        "feature_used" => &["feature"],
        // Fired on quit; `duration_bucket` is a coarse label, never raw ms.
        "session_ended" => &["duration_bucket"],
        _ => return None,
    })
}

/// The only values `feature_used.feature` may take.
pub const FEATURES: &[&str] =
    &["slack_trigger", "webhook_trigger", "hire_install", "voice_dictation"];

/// The only values `message_sent.surface` may take — the four places a HUMAN
/// can send a message to an agent.
pub const MESSAGE_SURFACES: &[&str] = &["terminal", "composer", "steer", "hive"];

/// The subset of MESSAGE_SURFACES the renderer may name over IPC. `steer` and
/// `hive` are counted in the backend, at the command handlers that already
/// receive them, so the renderer must not be able to name those two.
const RENDERER_MESSAGE_SURFACES: &[&str] = &["terminal", "composer"];

fn dnt_set() -> bool {
    match std::env::var("DO_NOT_TRACK") {
        Ok(v) => !v.is_empty() && v != "0",
        Err(_) => false,
    }
}

/// `os` / `arch` in the vocabulary TELEMETRY.md promises (darwin/win32/linux,
/// arm64/x64) — Rust's std::env::consts names are mapped, never sent raw.
fn os_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other, // linux / freebsd already match the contract
    }
}
fn arch_name() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        _ => "x64",
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub struct Analytics {
    /// Compile-time-injected project key; '' makes the whole module a no-op.
    pub posthog_key: &'static str,
    /// POSTHOG_HOST + /capture/ — the single-event ingestion endpoint.
    pub endpoint: String,
    started_at_ms: u64,
    session_ended: AtomicBool,
    /// Session-scoped dedup for feature_used.
    features_seen: Mutex<HashSet<String>>,
}

static ANALYTICS: LazyLock<Analytics> = LazyLock::new(Analytics::new);

/// The process-wide singleton, mirroring upstream's `analytics` export.
/// (Constructs lazily on first use — started_at_ms is the first-touch clock,
/// close enough to boot for the coarse duration_bucket.)
pub fn global() -> &'static Analytics {
    &ANALYTICS
}

impl Analytics {
    fn new() -> Self {
        Self {
            posthog_key: POSTHOG_KEY,
            endpoint: format!("{}/capture/", POSTHOG_HOST.trim_end_matches('/')),
            started_at_ms: now_ms(),
            session_ended: AtomicBool::new(false),
            features_seen: Mutex::new(HashSet::new()),
        }
    }

    /// The opt-out flag (persist kv). Absent = enabled — upstream's
    /// `telemetryEnabled` defaults ON, i.e. analytics is opt-OUT by default.
    pub fn opted_out(&self, app: &AppHandle) -> bool {
        state(app)
            .persist
            .lock()
            .unwrap()
            .get_kv(KV_OPT_OUT)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    /// The anonymous install id — a random UUID minted on first use and held in
    /// the persist kv. Never derived from the machine.
    fn distinct_id(&self, app: &AppHandle) -> String {
        let st = state(app);
        let persist = st.persist.lock().unwrap();
        if let Some(id) = persist
            .get_kv(KV_DISTINCT_ID)
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .filter(|s| !s.is_empty())
        {
            return id;
        }
        let id = uuid::Uuid::new_v4().to_string();
        persist.set_kv(KV_DISTINCT_ID, &json!(id));
        id
    }

    /// True only when a send may actually happen right now.
    fn enabled(&self, app: &AppHandle) -> bool {
        !self.posthog_key.is_empty()
            && !dnt_set()
            && !self.opted_out(app)
            && !self.session_ended.load(Ordering::SeqCst)
    }

    /// Capture one allowlisted event. Silently drops anything not in EVENTS,
    /// any prop key not allowlisted for the event, and any non-string prop.
    /// Fire-and-forget: the POST rides a spawned thread and never throws.
    pub fn track(&self, app: &AppHandle, event: &str, props: &Map<String, Value>) {
        if !self.enabled(app) {
            return;
        }
        let Some(allowed) = event_allowlist(event) else {
            return;
        };
        let mut properties = serde_json::Map::new();
        // Common props — the only ones TELEMETRY.md permits on every event.
        properties.insert("app_version".into(), json!(env!("CARGO_PKG_VERSION")));
        properties.insert("os".into(), json!(os_name()));
        properties.insert("arch".into(), json!(arch_name()));
        // anonymous event — no person profile; and pin $ip to null so PostHog
        // never stores the connection IP (belt and braces with the project-level
        // discard setting).
        properties.insert("$process_person_profile".into(), json!(false));
        properties.insert("$ip".into(), Value::Null);
        for (k, v) in props {
            if allowed.contains(&k.as_str()) && v.is_string() {
                properties.insert(k.clone(), v.clone());
            }
        }
        let distinct_id = self.distinct_id(app);
        let body = json!({
            "api_key": self.posthog_key,
            "event": event,
            "distinct_id": distinct_id,
            "properties": properties,
        });
        let url = self.endpoint.clone();
        // Events are rare (a handful per session) — a direct capture POST per
        // event, so quit loses at most the final session_ended, never a batch.
        std::thread::spawn(move || {
            let _ = ureq::Agent::config_builder()
                .timeout_global(Some(CAPTURE_TIMEOUT))
                .build()
                .new_agent()
                .post(&url)
                .header("content-type", "application/json")
                .send_json(&body);
        });
    }

    /// One human-sent message. NOT deduped — unlike track_feature this IS a
    /// usage meter, and the count is the whole point. Re-checks the enum at
    /// runtime because surfaces can originate in the renderer.
    pub fn message_sent(&self, app: &AppHandle, surface: &str) {
        if !MESSAGE_SURFACES.contains(&surface) {
            return;
        }
        let mut p = Map::new();
        p.insert("surface".into(), json!(surface));
        self.track(app, "message_sent", &p);
    }

    /// feature_used with per-session dedup (adoption signal, not a usage meter).
    pub fn track_feature(&self, app: &AppHandle, feature: &str) {
        if !FEATURES.contains(&feature) {
            return;
        }
        {
            let mut seen = self.features_seen.lock().unwrap();
            if !seen.insert(feature.to_string()) {
                return;
            }
        }
        let mut p = Map::new();
        p.insert("feature".into(), json!(feature));
        self.track(app, "feature_used", &p);
    }

    /// Renderer-originated track — everything crossing that seam is untrusted:
    /// the event allowlist filters property KEYS but not VALUES, so the closed
    /// enums are re-checked here (upstream isRendererMessageSurface + FEATURES).
    /// `steer`/`hive` surfaces are refused from the renderer — the backend
    /// counts them itself at its own handlers; letting the renderer name them
    /// would double-count.
    pub fn track_from_renderer(&self, app: &AppHandle, event: &str, props: &Map<String, Value>) {
        match event {
            "message_sent" => {
                let surface = props.get("surface").and_then(|v| v.as_str()).unwrap_or_default();
                if RENDERER_MESSAGE_SURFACES.contains(&surface) {
                    self.message_sent(app, surface);
                }
            }
            "feature_used" => {
                if let Some(f) = props.get("feature").and_then(|v| v.as_str()) {
                    self.track_feature(app, f);
                }
            }
            // Every other allowlisted event may arrive with its declared props.
            _ => self.track(app, event, props),
        }
    }

    /// Live opt-in/out from Settings. Upstream flipped an in-memory flag backed
    /// by `telemetryEnabled` config; here the kv write is the whole state —
    /// sends stop instantly and nothing needs recreating to turn back on.
    pub fn set_opt_in(&self, app: &AppHandle, on: bool) {
        state(app)
            .persist
            .lock()
            .unwrap()
            .set_kv(KV_OPT_OUT, &json!(!on));
    }

    /// Boot-time events — call once from setup (upstream init()): mints the
    /// install id, fires first_run / update_applied / app_launched. Stay-dark
    /// rules: no build key or DNT → no id is minted and no stamp is written.
    /// `harness_home` is the state dir (hive.root's parent); `app_version` is
    /// the running build.
    pub fn init(&self, app: &AppHandle, harness_home: &std::path::Path, app_version: &str) {
        if self.posthog_key.is_empty() || dnt_set() {
            return; // stay dark: no client, no id
        }
        let st = state(app);
        let (id_exists, previous_version) = {
            let persist = st.persist.lock().unwrap();
            (
                persist
                    .get_kv(KV_DISTINCT_ID)
                    .and_then(|v| v.as_str().map(|s| !s.is_empty()))
                    .unwrap_or(false),
                persist
                    .get_kv(KV_LAST_VERSION)
                    .and_then(|v| v.as_str().map(|s| s.to_string())),
            )
        };
        let first_run = !id_exists;
        // Mint (or reuse) the id — marks the install as seen.
        let _ = self.distinct_id(app);

        // Version-change detection — read BEFORE the stamp is refreshed, stamp
        // BEFORE sending: at-most-once beats at-least-once here.
        let transition = update_transition(previous_version.as_deref(), app_version, first_run);
        if previous_version.as_deref() != Some(app_version) {
            st.persist
                .lock()
                .unwrap()
                .set_kv(KV_LAST_VERSION, &json!(app_version));
        }

        if first_run {
            self.track(app, "first_run", &Map::new());
        }
        if let Some(t) = transition {
            let via = update_via(
                read_updater_log_tail(&harness_home.join("updater.log")).as_deref(),
                &t.to_version,
            );
            let mut p = Map::new();
            p.insert("from_version".into(), json!(t.from_version));
            p.insert("to_version".into(), json!(t.to_version));
            p.insert("via".into(), json!(via));
            self.track(app, "update_applied", &p);
        }
        self.track(app, "app_launched", &Map::new());
    }

    /// Fire session_ended and stop sending. Idempotent — upstream's endSession.
    /// Wire into a quit path if one is added; nothing here blocks shutdown.
    pub fn end_session(&self, app: &AppHandle) {
        if self.session_ended.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut p = Map::new();
        p.insert(
            "duration_bucket".into(),
            json!(duration_bucket(now_ms().saturating_sub(self.started_at_ms))),
        );
        // session_ended is set but track() checks it — send this one directly.
        if !self.posthog_key.is_empty() && !dnt_set() && !self.opted_out(app) {
            let mut properties = serde_json::Map::new();
            properties.insert("app_version".into(), json!(env!("CARGO_PKG_VERSION")));
            properties.insert("os".into(), json!(os_name()));
            properties.insert("arch".into(), json!(arch_name()));
            properties.insert("$process_person_profile".into(), json!(false));
            properties.insert("$ip".into(), Value::Null);
            for (k, v) in p {
                properties.insert(k, v);
            }
            let body = json!({
                "api_key": self.posthog_key,
                "event": "session_ended",
                "distinct_id": self.distinct_id(app),
                "properties": properties,
            });
            let url = self.endpoint.clone();
            std::thread::spawn(move || {
                let _ = ureq::Agent::config_builder()
                    .timeout_global(Some(CAPTURE_TIMEOUT))
                    .build()
                    .new_agent()
                    .post(&url)
                    .header("content-type", "application/json")
                    .send_json(&body);
            });
        }
    }
}

/// Semver-shaped and nothing else — the stamp is a user-writable kv value, so
/// it is re-validated on the way out (upstream VERSION_RE:
/// `^[0-9]{1,6}\.[0-9]{1,6}\.[0-9]{1,6}(?:-[0-9A-Za-z.]{1,32})?$`; hand-rolled
/// here because the regex crate isn't a dependency).
pub fn valid_version(v: &str) -> bool {
    let (core, prerelease_ok) = match v.split_once('-') {
        Some((c, tag)) => (c, !tag.is_empty() && tag.len() <= 32
            && tag.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '.')),
        None => (v, true),
    };
    if !prerelease_ok {
        return false;
    }
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 6 && p.bytes().all(|b| b.is_ascii_digit()))
}

pub struct VersionTransition {
    pub from_version: String,
    pub to_version: String,
}

/// The single place that decides whether a boot is a version change (upstream
/// updateTransition): first-run → never an update; unparseable landing → none;
/// same stamp → ordinary relaunch; else a transition (downgrades report
/// honestly, from > to).
pub fn update_transition(
    previous: Option<&str>,
    current: &str,
    first_run: bool,
) -> Option<VersionTransition> {
    if first_run || !valid_version(current) {
        return None;
    }
    if previous == Some(current) {
        return None;
    }
    Some(VersionTransition {
        from_version: match previous {
            Some(p) if valid_version(p) => p.to_string(),
            _ => "unknown".to_string(),
        },
        to_version: current.to_string(),
    })
}

// ── Was it OUR updater, or did the user reinstall by hand? ─────────────────
// Port of upstream's updateVia + readUpdaterLogTail: the updater's append-only
// log is the marker — a download line naming THIS version followed by a clean
// restart request, with no contradicting later launch, means `auto`.

const LOG_DOWNLOADED: &str = "update downloaded: ";
const LOG_QUIT_REQUESTED: &str = "quitAndInstall requested by the user";
const LOG_QUIT_FAILED: &str = "quitAndInstall failed:";
const LOG_READY_PREFIX: &str = "native updater ready (current v";
const LOG_QUIT_CANCELLED: &str = "quitAndInstall cancelled by the user at the quit warning";
const LOG_TAIL_BYTES: usize = 128 * 1024;

/// `auto` our updater installed it, `manual` something else moved the version,
/// `unknown` there is no log to read.
pub fn update_via(log_text: Option<&str>, to_version: &str) -> &'static str {
    let Some(text) = log_text else { return "unknown" };
    let lines: Vec<&str> = text.split('\n').collect();

    // "update downloaded: <ver>" with a lookahead that excludes every character
    // a version can CONTINUE with — `0.4.5` must not match `0.4.55` or
    // `0.4.5-beta.1`.
    let downloaded = |line: &str| -> bool {
        let Some(pos) = line.find(LOG_DOWNLOADED) else { return false };
        let rest = &line[pos + LOG_DOWNLOADED.len()..];
        if !rest.starts_with(to_version) {
            return false;
        }
        match rest[to_version.len()..].chars().next() {
            None => true,
            Some(c) => !c.is_ascii_digit() && c != '.' && c != '-',
        }
    };
    // The LAST download of THIS version — anything earlier names another one.
    let Some(i) = lines.iter().rposition(|l| downloaded(l)) else {
        return "manual";
    };
    // A restart request AFTER that download (file order is time order).
    let Some(j) = lines[i + 1..]
        .iter()
        .rposition(|l| l.contains(LOG_QUIT_REQUESTED))
        .map(|p| i + 1 + p)
    else {
        return "manual";
    };
    // updater.ts logs the failure immediately after the request — an attempt
    // that threw is the very next line; the app never quit.
    if lines.get(j + 1).is_some_and(|l| l.contains(LOG_QUIT_FAILED)) {
        return "manual";
    }
    // A clean return still proves nothing: disproof only. A later launch naming
    // a different version, or the user refusing the quit warning, takes it back.
    for line in &lines[j + 1..] {
        if line.contains(LOG_QUIT_CANCELLED) {
            return "manual";
        }
        if let Some(pos) = line.find(LOG_READY_PREFIX) {
            let rest = &line[pos + LOG_READY_PREFIX.len()..];
            let launched: String = rest
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != ')')
                .collect();
            if launched != to_version {
                return "manual";
            }
        }
    }
    "auto"
}

/// Last LOG_TAIL_BYTES of the updater log, or None when there is none to read.
/// Never throws — a missing log is an answer ('unknown'), not a failure.
pub fn read_updater_log_tail(path: &std::path::Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let size = f.metadata().ok()?.len() as usize;
    if size == 0 {
        return None;
    }
    let n = size.min(LOG_TAIL_BYTES);
    f.seek(SeekFrom::End(-(n as i64))).ok()?;
    let mut buf = vec![0u8; n];
    f.read_exact(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).to_string())
}

/// Coarse, non-identifying session-length label.
fn duration_bucket(ms: u64) -> &'static str {
    let m = ms / 60_000;
    if m < 5 {
        "<5m"
    } else if m < 30 {
        "5-30m"
    } else if m < 120 {
        "30m-2h"
    } else if m < 480 {
        "2-8h"
    } else {
        "8h+"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_is_exact() {
        assert!(event_allowlist("first_run").unwrap().is_empty());
        assert!(event_allowlist("message_sent").unwrap().contains(&"surface"));
        assert!(event_allowlist("rm_rf").is_none());
        assert!(!RENDERER_MESSAGE_SURFACES.contains(&"hive"));
    }

    #[test]
    fn versions_validate() {
        assert!(valid_version("0.4.6"));
        assert!(valid_version("0.4.6-beta.1"));
        assert!(!valid_version("0.4"));
        assert!(!valid_version("0.4.6.7"));
        assert!(!valid_version("anything"));
        assert!(update_transition(Some("0.4.5"), "0.4.6", false).is_some());
        assert!(update_transition(Some("0.4.6"), "0.4.6", false).is_none());
        assert!(update_transition(None, "0.4.6", true).is_none());
        assert_eq!(
            update_transition(None, "0.4.6", false).unwrap().from_version,
            "unknown"
        );
    }

    #[test]
    fn via_reads_the_log() {
        let log = "update downloaded: 0.4.5 — waiting for the user to restart\nquitAndInstall requested by the user\n";
        assert_eq!(update_via(Some(log), "0.4.5"), "auto");
        assert_eq!(update_via(None, "0.4.5"), "unknown");
        assert_eq!(update_via(Some("nothing relevant\n"), "0.4.5"), "manual");
        // a different version downloaded is not evidence for this one
        assert_eq!(update_via(Some(log), "0.4.55"), "manual");
    }
}
