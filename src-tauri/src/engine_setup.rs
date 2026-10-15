//! First-run engine setup progress.
//!
//! The default engine (Hermes) is installed on first spawn by the missing-CLI
//! ladder (misc.rs `install_script_with_rung`, run in an "install PTY" in the
//! agent's own terminal slot) and then needs a first start on a fresh per-agent
//! HERMES_HOME that prints "preparing dependencies for this data root..." for a
//! while. To a non-technical user that is a terminal with scrolling text and no
//! explanation. This module turns the ladder's real steps into one structured
//! event so the UI can say what is happening:
//!
//! ```text
//! engine:setup  { agentId, engine, phase, detail, ts, startedAt, reason?, line? }
//! ```
//!
//! # Phases (in order; `failed` / `stopped` can end any of them)
//!
//! | phase       | detail (machine token)                       | where it comes from |
//! |-------------|----------------------------------------------|---------------------|
//! | `checking`  | `binary`                                     | `spawn_inner_impl` begins a Hermes spawn |
//! | `installing`| the rung (`native`, `npm`, `node-then-npm`)  | the ladder armed the install PTY; installer sub-steps (`python`, `node`, `ripgrep`, `ffmpeg`) from [`MARKERS`] |
//! | `starting`  | `launch` / `relaunch`                        | the engine PTY was spawned (or the install PTY exited 0 and the spawn re-runs) |
//! | `preparing` | `preparing-deps`                             | [`MARKERS`]: Hermes' "preparing dependencies" line in the PTY stream |
//! | `ready`     | `prompt` / `welcome` / `hook` / `settled`    | prompt-up escape sequence or banner in the stream, the first hive hook event, or (safety net) a quiet settle |
//! | `failed`    | the last output line                         | `reason` below |
//! | `stopped`   | -                                            | the agent was killed / torn down while setup was in flight |
//!
//! `failed.reason` is one of: `network`, `no_key`, `installer_error`,
//! `installer_unavailable`, `engine_missing`, `engine_exit`, `timeout`,
//! `spawn_error` (see [`classify_failure`]).
//!
//! `line` rides along (throttled to one per [`LINE_THROTTLE`]) so the card's
//! "Show details" disclosure can show the recent raw output without the
//! frontend subscribing to the PTY itself.
//!
//! # Rules
//!
//! * Observation only. [`observe_output`] is called from the PTY reader thread
//!   AFTER the chunk was emitted to the terminal and recorded in the tail; it
//!   never blocks on I/O, never writes to the PTY, never alters the stream.
//!   With no active watch it is one atomic load.
//! * The marker table is small on purpose and every entry is documented and
//!   tested. Output the table does not know changes nothing (the phase just
//!   stays as the ladder left it).
//! * Phase events are real: each comes from a ladder step, a PTY exit, a hook
//!   event or a recognised line - never from a timer pretending progress. The
//!   timers only END a stuck setup (`timeout`) or settle a quiet start.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Once};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::lock_ext::LockExt;

/// The event channel the frontend listens on (cth.ts `onEngineSetup`).
pub const EVENT: &str = "engine:setup";

/// Verification hook: with this env var set (to anything but `0`/empty) EVERY
/// provider's spawn is watched and scanned with the Hermes marker table. It
/// exists so the whole pipeline - PTY observer, markers, exit classification,
/// events, Try again - can be driven end to end by a harmless `custom` stub
/// engine, without ever running the real Hermes installer or touching a real
/// Hermes home / keyring. Unset in production.
pub const WATCH_ALL_ENV: &str = "AGENT_OFFICE_SETUP_WATCH_ALL";

fn watch_all() -> bool {
    std::env::var(WATCH_ALL_ENV).is_ok_and(|v| !v.is_empty() && v != "0")
}

/// Is a spawn of `provider` announced as a first-run setup? Hermes always (its
/// first start can take minutes); other engines only report once their install
/// ladder runs - unless the verification hook above is on.
pub fn watches(provider: &str) -> bool {
    provider == "hermes" || watch_all()
}

/// Which marker table scans this provider's output.
fn marker_engine(provider: &str) -> String {
    if watch_all() { "hermes".to_string() } else { provider.to_string() }
}

/// One `line` event per this much time at most.
pub const LINE_THROTTLE: Duration = Duration::from_millis(700);
/// A whole install (Python + Node + ripgrep + ffmpeg download) can take
/// minutes; past this the setup is reported as stuck.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(25 * 60);
/// First start incl. "preparing dependencies" on a fresh data root.
pub const START_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Safety net: a start with output, no `preparing` marker, at least this old
/// and quiet for [`SETTLE_QUIET`] counts as ready.
pub const SETTLE_AFTER: Duration = Duration::from_secs(30);
pub const SETTLE_QUIET: Duration = Duration::from_secs(15);
/// How many recent lines a watch remembers (snapshot / reload).
const RECENT_MAX: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Checking,
    Installing,
    Starting,
    Preparing,
    Ready,
    Failed,
    Stopped,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Checking => "checking",
            Phase::Installing => "installing",
            Phase::Starting => "starting",
            Phase::Preparing => "preparing",
            Phase::Ready => "ready",
            Phase::Failed => "failed",
            Phase::Stopped => "stopped",
        }
    }
}

// ─── marker table ───────────────────────────────────────────────────────────

/// What a recognised line does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Move to this phase (detail = the marker id).
    Phase(Phase),
    /// Keep the phase, refine `detail` (e.g. which installer sub-step).
    Detail,
}

/// One recognised output line. Matching is case-insensitive on the ANSI-
/// stripped text: every `all` needle must be present and, when `any` is
/// non-empty, at least one of those too. A marker fires once per phase.
pub struct Marker {
    pub id: &'static str,
    pub engine: &'static str,
    /// Only consulted while the watch is in one of these phases.
    pub only_in: &'static [Phase],
    pub all: &'static [&'static str],
    pub any: &'static [&'static str],
    pub effect: Effect,
}

const INSTALL_VERBS: &[&str] = &["install", "download", "extract", "fetch"];

/// THE table (text markers). Keep it short; add a test with every entry.
///
/// * `preparing-deps` - Hermes prints "preparing dependencies for this data
///   root..." on the first start of a fresh HERMES_HOME; it can run for
///   minutes, so it is its own phase.
/// * `python` / `node` / `ripgrep` / `ffmpeg` - the installer's sub-steps
///   (it bundles all four); detail only, so the card can say "Installing
///   Python".
/// * `welcome` - Hermes' REPL banner ("Welcome to Hermes Agent!"): the
///   prompt is up.
pub const MARKERS: &[Marker] = &[
    Marker {
        id: "preparing-deps",
        engine: "hermes",
        only_in: &[Phase::Starting],
        all: &["preparing dependencies"],
        any: &[],
        effect: Effect::Phase(Phase::Preparing),
    },
    Marker {
        id: "python",
        engine: "hermes",
        only_in: &[Phase::Installing],
        all: &["python"],
        any: INSTALL_VERBS,
        effect: Effect::Detail,
    },
    Marker {
        id: "node",
        engine: "hermes",
        only_in: &[Phase::Installing],
        all: &["node"],
        any: INSTALL_VERBS,
        effect: Effect::Detail,
    },
    Marker {
        id: "ripgrep",
        engine: "hermes",
        only_in: &[Phase::Installing],
        all: &["ripgrep"],
        any: INSTALL_VERBS,
        effect: Effect::Detail,
    },
    Marker {
        id: "ffmpeg",
        engine: "hermes",
        only_in: &[Phase::Installing],
        all: &["ffmpeg"],
        any: INSTALL_VERBS,
        effect: Effect::Detail,
    },
    Marker {
        id: "welcome",
        engine: "hermes",
        only_in: &[Phase::Starting, Phase::Preparing],
        all: &["welcome to hermes agent"],
        any: &[],
        effect: Effect::Phase(Phase::Ready),
    },
];

/// Raw (pre-strip) escape sequences that mean "an interactive prompt is up",
/// for any engine: DEC bracketed-paste-enable (prompt_toolkit - the Hermes
/// REPL - turns it on when it starts reading input) and the alternate screen
/// (full-screen TUIs). Only consulted while `starting` / `preparing`.
pub const READY_SEQS: &[(&str, &str)] = &[("\x1b[?2004h", "prompt"), ("\x1b[?1049h", "prompt")];

// ─── pure stream detector ───────────────────────────────────────────────────

/// What feeding the detector produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signal {
    /// A marker moved the phase.
    Phase(Phase, &'static str),
    /// A marker refined the detail only.
    Detail(&'static str),
    /// One cleaned, human-readable output line.
    Line(String),
}

/// Incremental PTY-text scanner. Pure (no I/O, no clock) so the table is
/// unit-testable with canned output.
pub struct Detector {
    engine: String,
    phase: Phase,
    /// Current (incomplete) line, ANSI-stripped.
    buf: String,
    fired: Vec<&'static str>,
}

const BUF_MAX: usize = 2048;

impl Detector {
    pub fn new(engine: &str, phase: Phase) -> Self {
        Self { engine: engine.to_string(), phase, buf: String::new(), fired: Vec::new() }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The ladder (not the stream) moved the phase.
    pub fn set_phase(&mut self, p: Phase) {
        if self.phase != p {
            self.phase = p;
            self.fired.clear();
        }
    }

    pub fn feed(&mut self, chunk: &str) -> Vec<Signal> {
        let mut out = Vec::new();
        // 1. Raw prompt-up sequences (before stripping eats them).
        if matches!(self.phase, Phase::Starting | Phase::Preparing) {
            for (seq, id) in READY_SEQS {
                if chunk.contains(seq) {
                    self.phase = Phase::Ready;
                    out.push(Signal::Phase(Phase::Ready, id));
                    return out;
                }
            }
        }
        // 2. Text: whole lines first (each scanned for markers, then reported),
        // then the partial line - a marker such as "preparing dependencies..."
        // is recognised before its newline ever arrives.
        self.buf.push_str(&strip_ansi(chunk));
        // `\r` ends a line too: installers redraw progress with a bare CR.
        while let Some(i) = self.buf.find(['\n', '\r']) {
            let line: String = self.buf[..i].to_string();
            self.buf.drain(..=i);
            if self.scan_markers(&line, &mut out) {
                return out;
            }
            if let Some(clean) = clean_line(&line) {
                out.push(Signal::Line(clean));
            }
        }
        let partial = self.buf.clone();
        if self.scan_markers(&partial, &mut out) {
            return out;
        }
        if self.buf.len() > BUF_MAX {
            let line = std::mem::take(&mut self.buf);
            if let Some(clean) = clean_line(&line) {
                out.push(Signal::Line(clean));
            }
        }
        out
    }

    /// Push every marker `text` triggers; true when the stream reached `ready`
    /// (nothing after it is worth scanning).
    fn scan_markers(&mut self, text: &str, out: &mut Vec<Signal>) -> bool {
        if text.is_empty() {
            return false;
        }
        let hay = text.to_lowercase();
        for m in MARKERS {
            if m.engine != self.engine
                || !m.only_in.contains(&self.phase)
                || self.fired.contains(&m.id)
                || !m.all.iter().all(|n| hay.contains(n))
                || (!m.any.is_empty() && !m.any.iter().any(|n| hay.contains(n)))
            {
                continue;
            }
            self.fired.push(m.id);
            match m.effect {
                Effect::Phase(p) => {
                    self.phase = p;
                    self.fired.clear();
                    out.push(Signal::Phase(p, m.id));
                    // The phase changed: the rest of the table was written for
                    // the old one.
                    return p == Phase::Ready;
                }
                Effect::Detail => out.push(Signal::Detail(m.id)),
            }
        }
        false
    }
}

/// Drop ANSI/VT escape sequences and stray control characters, keep text,
/// `\n`, `\r`, `\t`.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\x1b' => match it.peek().copied() {
                // CSI: ESC [ params final(0x40-0x7e)
                Some('[') => {
                    it.next();
                    for n in it.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&n) {
                            break;
                        }
                    }
                }
                // OSC: ESC ] ... BEL | ESC \
                Some(']') => {
                    it.next();
                    while let Some(n) = it.next() {
                        if n == '\x07' {
                            break;
                        }
                        if n == '\x1b' {
                            if it.peek() == Some(&'\\') {
                                it.next();
                            }
                            break;
                        }
                    }
                }
                // Any other two-byte escape (ESC c, ESC = ...).
                Some(_) => {
                    it.next();
                }
                None => {}
            },
            '\n' | '\r' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// A displayable line: whitespace-collapsed, capped, and carrying at least a
/// few letters/digits (spinner frames and box-drawing rows are dropped).
fn clean_line(raw: &str) -> Option<String> {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().filter(|c| c.is_alphanumeric()).count() < 3 {
        return None;
    }
    Some(collapsed.chars().take(200).collect())
}

/// Machine-readable reason for a setup that ended badly. `tail` is the last
/// output (raw is fine - it is stripped here). Order matters: a network error
/// beats everything (the installer's own message is useless to the user),
/// then a missing key, a missing binary, then the phase's generic reason.
pub fn classify_failure(phase: Phase, tail: &str) -> &'static str {
    let t = strip_ansi(tail).to_lowercase();
    const NETWORK: &[&str] = &[
        "could not resolve host",
        "couldn't resolve host",
        "name or service not known",
        "no such host is known",
        "the remote name could not be resolved",
        "unable to connect to the remote server",
        "network is unreachable",
        "connection refused",
        "connection reset",
        "connection timed out",
        "operation timed out",
        "timed out",
        "econnreset",
        "enotfound",
        "etimedout",
        "getaddrinfo",
        "curl: (6)",
        "curl: (7)",
        "curl: (28)",
        "curl: (35)",
        "curl: (56)",
        "ssl connect error",
        "could not establish trust relationship",
    ];
    const NO_KEY: &[&str] = &[
        "no api key",
        "no api keys",
        "missing api key",
        "api key not set",
        "api key is not set",
        "invalid api key",
        "invalid_api_key",
        "incorrect api key",
        "unauthorized",
        "authentication failed",
        "error 401",
        "http 401",
        "status 401",
        "status code 401",
        "401 unauthorized",
    ];
    const MISSING: &[&str] = &[
        "is not recognized as an internal or external command",
        "command not found",
        "no such file or directory: 'hermes'",
    ];
    if NETWORK.iter().any(|n| t.contains(n)) {
        return "network";
    }
    if !matches!(phase, Phase::Installing) && NO_KEY.iter().any(|n| t.contains(n)) {
        return "no_key";
    }
    if MISSING.iter().any(|n| t.contains(n)) {
        return if matches!(phase, Phase::Installing) { "installer_error" } else { "engine_missing" };
    }
    if matches!(phase, Phase::Installing) {
        "installer_error"
    } else {
        "engine_exit"
    }
}

fn last_line(tail: &str) -> String {
    strip_ansi(tail)
        .split(['\n', '\r'])
        .filter_map(clean_line)
        .last()
        .unwrap_or_default()
}

// ─── watches ────────────────────────────────────────────────────────────────

type Emitter = Arc<dyn Fn(Value) + Send + Sync>;

struct Watch {
    agent_id: String,
    engine: String,
    det: Detector,
    emit: Emitter,
    detail: String,
    started: Instant,
    started_ms: i64,
    phase_since: Instant,
    phase_ms: i64,
    saw_output: bool,
    last_output: Option<Instant>,
    saw_preparing: bool,
    /// Newest cleaned line and whether it already went out in an event.
    last_line: Option<String>,
    line_sent: bool,
    last_line_emit: Option<Instant>,
    recent: VecDeque<String>,
}

static WATCHES: LazyLock<Mutex<HashMap<String, Watch>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Last failure per agent - so a window that reloads (or opens late) can still
/// show the failure card until the user acts on it.
static FAILED: LazyLock<Mutex<HashMap<String, Value>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Watch {
    fn payload(&self, phase: Phase, reason: Option<&str>, with_line: bool) -> Value {
        let mut v = json!({
            "agentId": self.agent_id,
            "engine": self.engine,
            "phase": phase.as_str(),
            "detail": self.detail,
            "ts": now_ms(),
            "startedAt": self.started_ms,
        });
        if let Some(r) = reason {
            v["reason"] = json!(r);
        }
        if with_line {
            if let Some(l) = &self.last_line {
                v["line"] = json!(l);
            }
        }
        v
    }

    /// Move the phase; returns the event to emit.
    fn enter(&mut self, phase: Phase, detail: &str, reason: Option<&str>, now: Instant) -> Value {
        self.det.set_phase(phase);
        self.detail = detail.to_string();
        self.phase_since = now;
        self.phase_ms = now_ms();
        if phase == Phase::Preparing {
            self.saw_preparing = true;
        }
        self.line_sent = true;
        self.last_line_emit = Some(now);
        self.payload(phase, reason, true)
    }

    fn note_line(&mut self, line: String, now: Instant) -> Option<Value> {
        self.recent.push_back(line.clone());
        while self.recent.len() > RECENT_MAX {
            self.recent.pop_front();
        }
        self.last_line = Some(line);
        self.line_sent = false;
        self.flush_line(now)
    }

    /// Emit the pending line when the throttle allows.
    fn flush_line(&mut self, now: Instant) -> Option<Value> {
        if self.line_sent || self.last_line.is_none() {
            return None;
        }
        if self.last_line_emit.is_some_and(|t| now.duration_since(t) < LINE_THROTTLE) {
            return None;
        }
        self.line_sent = true;
        self.last_line_emit = Some(now);
        Some(self.payload(self.det.phase(), None, true))
    }

    /// Timers: end a stuck setup, settle a quiet start. Pure in `now`.
    fn tick(&mut self, now: Instant) -> Option<(Phase, &'static str, Option<&'static str>)> {
        let phase = self.det.phase();
        let in_phase = now.saturating_duration_since(self.phase_since);
        match phase {
            Phase::Installing if now.saturating_duration_since(self.started) >= INSTALL_TIMEOUT => {
                Some((Phase::Failed, "timeout", Some("timeout")))
            }
            Phase::Starting | Phase::Preparing
                if now.saturating_duration_since(self.started) >= START_TIMEOUT =>
            {
                Some((Phase::Failed, "timeout", Some("timeout")))
            }
            Phase::Starting
                if !self.saw_preparing
                    && self.saw_output
                    && in_phase >= SETTLE_AFTER
                    && self
                        .last_output
                        .is_some_and(|t| now.saturating_duration_since(t) >= SETTLE_QUIET) =>
            {
                Some((Phase::Ready, "settled", None))
            }
            _ => None,
        }
    }
}

/// Run `f` on the watch for `key`; deliver whatever events it returns AFTER
/// the lock is released. Terminal phases drop the watch.
fn with_watch(key: &str, f: impl FnOnce(&mut Watch) -> Vec<Value>) {
    let (emit, events) = {
        let mut map = WATCHES.lock_safe();
        let Some(w) = map.get_mut(key) else { return };
        let events = f(w);
        let emit = w.emit.clone();
        let done = events
            .iter()
            .any(|e| matches!(e["phase"].as_str(), Some("ready" | "failed" | "stopped")));
        if done {
            map.remove(key);
        }
        ACTIVE.store(map.len(), Ordering::Relaxed);
        (emit, events)
    };
    for e in events {
        remember_failure(&e);
        emit(e);
    }
}

fn remember_failure(e: &Value) {
    let Some(agent) = e["agentId"].as_str() else { return };
    let mut failed = FAILED.lock_safe();
    if e["phase"] == "failed" {
        failed.insert(agent.to_string(), e.clone());
    } else {
        failed.remove(agent);
    }
}

// ─── ladder / lifecycle hooks (called from commands.rs, pty.rs, hooks.rs) ────

/// Start (or restart) watching `pty_id`. `emit` delivers events; the real
/// entry point [`begin`] builds it from an AppHandle, tests pass a recorder.
pub(crate) fn begin_with(
    emit: Emitter,
    pty_id: &str,
    agent_id: &str,
    engine: &str,
    phase: Phase,
    detail: &str,
) {
    ensure_ticker();
    let now = Instant::now();
    let w = Watch {
        agent_id: agent_id.to_string(),
        engine: engine.to_string(),
        det: Detector::new(&marker_engine(engine), phase),
        emit,
        detail: detail.to_string(),
        started: now,
        started_ms: now_ms(),
        phase_since: now,
        phase_ms: now_ms(),
        saw_output: false,
        last_output: None,
        saw_preparing: false,
        last_line: None,
        line_sent: true,
        last_line_emit: None,
        recent: VecDeque::new(),
    };
    let first = w.payload(phase, None, false);
    let emit = w.emit.clone();
    {
        let mut map = WATCHES.lock_safe();
        map.insert(pty_id.to_string(), w);
        ACTIVE.store(map.len(), Ordering::Relaxed);
    }
    remember_failure(&first);
    emit(first);
}

pub(crate) fn begin<R: tauri::Runtime>(
    app: &AppHandle<R>,
    pty_id: &str,
    agent_id: &str,
    owner: &str,
    engine: &str,
    phase: Phase,
    detail: &str,
) {
    begin_with(emitter(app, owner), pty_id, agent_id, engine, phase, detail);
}

fn emitter<R: tauri::Runtime>(app: &AppHandle<R>, owner: &str) -> Emitter {
    let app = app.clone();
    let owner = owner.to_string();
    Arc::new(move |v| crate::floors::emit_to_owner(&app, &owner, EVENT, v))
}

/// The ladder armed an install PTY for `provider` (rung = the closed
/// `agent_install_started.rung` enum). `relaunch` false (the manual rung)
/// means nothing will ever start: that is a failure, said honestly.
pub(crate) fn installing<R: tauri::Runtime>(
    app: &AppHandle<R>,
    pty_id: &str,
    agent_id: &str,
    owner: &str,
    provider: &str,
    rung: &str,
    relaunch: bool,
) {
    if !relaunch || rung == "manual" {
        begin(app, pty_id, agent_id, owner, provider, Phase::Installing, rung);
        with_watch(pty_id, |w| {
            let now = Instant::now();
            vec![w.enter(Phase::Failed, provider, Some("installer_unavailable"), now)]
        });
        return;
    }
    let exists = WATCHES.lock_safe().contains_key(pty_id);
    if exists {
        with_watch(pty_id, |w| vec![w.enter(Phase::Installing, rung, None, Instant::now())]);
    } else {
        begin(app, pty_id, agent_id, owner, provider, Phase::Installing, rung);
    }
}

/// The engine PTY itself is up. `needs_key`: the spawn resolved no credential
/// from any source - setup cannot finish, say so instead of waiting for a 401.
pub(crate) fn launched(pty_id: &str, relaunch: bool, needs_key: bool) {
    with_watch(pty_id, |w| {
        let now = Instant::now();
        if needs_key {
            return vec![w.enter(Phase::Failed, "no API key", Some("no_key"), now)];
        }
        if w.det.phase() == Phase::Starting {
            return Vec::new(); // the install exit already announced it
        }
        vec![w.enter(Phase::Starting, if relaunch { "relaunch" } else { "launch" }, None, now)]
    });
}

/// PTY reader hook: called AFTER the chunk was emitted and recorded. Cheap
/// when nothing is being watched.
pub(crate) fn observe_output(pty_id: &str, chunk: &str) {
    if ACTIVE.load(Ordering::Relaxed) == 0 {
        return;
    }
    with_watch(pty_id, |w| {
        let now = Instant::now();
        w.saw_output = true;
        w.last_output = Some(now);
        let mut events = Vec::new();
        for sig in w.det.feed(chunk) {
            match sig {
                Signal::Phase(p, id) => {
                    // The line (if any) travels with the phase event.
                    events.push(w.enter(p, id, None, now));
                }
                Signal::Detail(id) => {
                    // A sub-step change is worth an event of its own (not
                    // throttled): "Installing Python" -> "Installing ffmpeg".
                    w.detail = id.to_string();
                    w.line_sent = true;
                    w.last_line_emit = Some(now);
                    events.push(w.payload(w.det.phase(), None, true));
                }
                Signal::Line(l) => {
                    if let Some(e) = w.note_line(l, now) {
                        events.push(e);
                    }
                }
            }
            if events.iter().any(|e| e["phase"] == "ready") {
                break;
            }
        }
        events
    });
}

/// The PTY exited (natural exit, before the relaunch/teardown logic).
/// `relaunch_pending`: this was an install PTY armed to re-run the spawn.
pub(crate) fn on_exit(pty_id: &str, code: Option<u32>, tail: &str, relaunch_pending: bool) {
    with_watch(pty_id, |w| {
        let now = Instant::now();
        let phase = w.det.phase();
        if phase == Phase::Installing && code == Some(0) && relaunch_pending {
            // Installer finished: the engine spawn re-runs next.
            return vec![w.enter(Phase::Starting, "relaunch", None, now)];
        }
        let reason = classify_failure(phase, tail);
        let detail = {
            let l = last_line(tail);
            if l.is_empty() { w.last_line.clone().unwrap_or_default() } else { l }
        };
        vec![w.enter(Phase::Failed, &detail, Some(reason), now)]
    });
}

/// The agent's first hive hook event proves the engine is up and talking.
pub(crate) fn note_hook(agent_id: &str) {
    if ACTIVE.load(Ordering::Relaxed) == 0 {
        return;
    }
    let keys: Vec<String> = WATCHES
        .lock_safe()
        .iter()
        .filter(|(_, w)| {
            w.agent_id == agent_id
                && matches!(w.det.phase(), Phase::Checking | Phase::Starting | Phase::Preparing)
        })
        .map(|(k, _)| k.clone())
        .collect();
    for k in keys {
        with_watch(&k, |w| vec![w.enter(Phase::Ready, "hook", None, Instant::now())]);
    }
}

/// The spawn itself failed (spawn_inner returned Err).
pub(crate) fn spawn_failed(pty_id: Option<&str>, error: &str) {
    let Some(pty_id) = pty_id else { return };
    with_watch(pty_id, |w| {
        let detail: String = error.chars().take(200).collect();
        vec![w.enter(Phase::Failed, &detail, Some("spawn_error"), Instant::now())]
    });
}

/// The agent was killed / torn down: whatever setup was in flight is moot.
pub(crate) fn forget(pty_id: &str) {
    with_watch(pty_id, |w| vec![w.enter(Phase::Stopped, "stopped", None, Instant::now())]);
}

/// Drop a remembered failure (the user pressed Try again / fixed the key).
pub(crate) fn clear_failure(agent_id: &str) {
    FAILED.lock_safe().remove(agent_id);
}

/// Current state of every in-flight setup + remembered failures, for a window
/// that (re)loaded after the events went by.
pub fn snapshot() -> Vec<Value> {
    let mut out: Vec<Value> = WATCHES
        .lock_safe()
        .values()
        .map(|w| {
            let mut v = w.payload(w.det.phase(), None, false);
            v["ts"] = json!(w.phase_ms);
            v["lines"] = json!(w.recent.iter().collect::<Vec<_>>());
            v
        })
        .collect();
    let live: Vec<String> = out.iter().filter_map(|v| v["agentId"].as_str().map(String::from)).collect();
    out.extend(
        FAILED
            .lock_safe()
            .iter()
            .filter(|(a, _)| !live.contains(a))
            .map(|(_, v)| v.clone()),
    );
    out
}

fn tick_all(now: Instant) {
    let keys: Vec<String> = WATCHES.lock_safe().keys().cloned().collect();
    for k in keys {
        tick_key(&k, now);
    }
}

fn tick_key(key: &str, now: Instant) {
    with_watch(key, |w| {
        let mut events = Vec::new();
        if let Some(e) = w.flush_line(now) {
            events.push(e);
        }
        if let Some((p, detail, reason)) = w.tick(now) {
            events.push(w.enter(p, detail, reason, now));
        }
        events
    });
}

fn ensure_ticker() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = std::thread::Builder::new().name("engine-setup-tick".into()).spawn(|| loop {
            std::thread::sleep(Duration::from_secs(1));
            crate::panic_guard::guarded("engine-setup-tick", || tick_all(Instant::now()));
        });
    });
}

// ─── commands ───────────────────────────────────────────────────────────────

/// In-flight setups + remembered failures (see [`snapshot`]).
#[tauri::command]
pub fn engine_setup_snapshot() -> Vec<Value> {
    crate::panic_guard::cmd_guard("engine_setup_snapshot", || snapshot())
}

/// "Try again": stop whatever is half-alive in the agent's terminal slot and
/// re-run the whole spawn ladder for the same agent (installer included).
#[tauri::command(async)]
pub fn engine_setup_retry(
    app: AppHandle,
    window: tauri::WebviewWindow,
    agent_id: String,
) -> Result<bool, String> {
    crate::panic_guard::cmd_guard("engine_setup_retry", || {
        let st = crate::commands::state(&app);
        let pty_id = format!("pty-{agent_id}");
        let id = st.resolve_pty_id(&pty_id).unwrap_or_else(|| pty_id.clone());
        if st.ptys.lock_safe().contains_key(&id) {
            let _ = crate::commands::kill_agent_core(&app, &id);
        }
        crate::commands::PENDING_INSTALLS.lock_safe().remove(&pty_id);
        clear_failure(&agent_id);
        // The pooled terminal clears its old frame and its latched exit.
        crate::floors::emit_to_owner(
            &app,
            window.label(),
            &format!("pty:relaunch:{pty_id}"),
            json!({}),
        );
        crate::commands::respawn_agent_core(&app, window.label(), &agent_id).map(|_| true)
    })
}

/// What the user's own Hermes setup looks like - read-only. Resolves the home
/// exactly like the seeding code (`hive::user_hermes_home`), reads ONLY
/// `config.yaml` (provider / model) and `.env` (is any `*_API_KEY=` set?), and
/// never returns a key value.
#[tauri::command]
pub fn hermes_user_config_status() -> Value {
    crate::panic_guard::cmd_guard("hermes_user_config_status", || {
        hermes_user_config_status_at(&crate::hive::user_hermes_home())
    })
}

pub(crate) fn hermes_user_config_status_at(home: &std::path::Path) -> Value {
    let cfg = std::fs::read_to_string(home.join("config.yaml")).ok();
    let exists = cfg.is_some();
    let (provider, model) = cfg
        .as_deref()
        .map(crate::hive::yaml_model_summary)
        .unwrap_or((None, None));
    let has_keys = std::fs::read_to_string(home.join(".env"))
        .map(|s| crate::hive::env_has_api_key(&s))
        .unwrap_or(false);
    json!({
        "exists": exists,
        "provider": provider,
        "model": model,
        "hasKeys": has_keys,
        "path": home.display().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(d: &mut Detector, chunks: &[&str]) -> Vec<Signal> {
        chunks.iter().flat_map(|c| d.feed(c)).collect()
    }

    #[test]
    fn strip_ansi_removes_csi_osc_and_controls() {
        assert_eq!(strip_ansi("\x1b[1;32mhello\x1b[0m \x1b]0;title\x07world\x1b[?25l"), "hello world");
        assert_eq!(strip_ansi("a\x07b\x00c\r\nd"), "abc\r\nd");
        assert_eq!(strip_ansi("\x1b]8;;http://x\x1b\\link\x1b]8;;\x1b\\"), "link");
    }

    #[test]
    fn preparing_marker_moves_starting_to_preparing_even_mid_line() {
        let mut d = Detector::new("hermes", Phase::Starting);
        // The marker arrives split across chunks and wrapped in colour codes.
        let sigs = feed_all(&mut d, &["\x1b[2mPreparing depend", "encies for this data root\x1b[0m..."]);
        assert!(sigs.contains(&Signal::Phase(Phase::Preparing, "preparing-deps")), "{sigs:?}");
        assert_eq!(d.phase(), Phase::Preparing);
        // Fires once.
        let again = d.feed("\npreparing dependencies for this data root...\n");
        assert!(!again.iter().any(|s| matches!(s, Signal::Phase(..))), "{again:?}");
    }

    #[test]
    fn markers_are_engine_and_phase_scoped() {
        // Another engine never matches Hermes' table.
        let mut other = Detector::new("claude", Phase::Starting);
        assert!(other.feed("preparing dependencies for this data root...\n")
            .iter()
            .all(|s| !matches!(s, Signal::Phase(..))));
        // The same text while installing is just a line.
        let mut inst = Detector::new("hermes", Phase::Installing);
        let s = inst.feed("preparing dependencies for this data root...\n");
        assert!(s.iter().all(|x| !matches!(x, Signal::Phase(..))), "{s:?}");
    }

    #[test]
    fn installer_substeps_refine_detail_only() {
        let mut d = Detector::new("hermes", Phase::Installing);
        let s = d.feed("Downloading Python 3.14 runtime\n");
        assert!(s.contains(&Signal::Detail("python")), "{s:?}");
        assert_eq!(d.phase(), Phase::Installing);
        let s = d.feed("installing ripgrep...\nExtracting ffmpeg\n");
        assert!(s.contains(&Signal::Detail("ripgrep")));
        assert!(s.contains(&Signal::Detail("ffmpeg")));
        // A line that merely mentions a tool without an install verb is not a marker.
        let mut d = Detector::new("hermes", Phase::Installing);
        assert!(!d.feed("python is fine\n").iter().any(|x| matches!(x, Signal::Detail(_))));
    }

    #[test]
    fn prompt_up_sequences_and_banner_mean_ready() {
        for chunk in ["\x1b[?9001h\x1b[?2004h> ", "\x1b[?1049h"] {
            let mut d = Detector::new("hermes", Phase::Starting);
            assert_eq!(d.feed(chunk), vec![Signal::Phase(Phase::Ready, "prompt")], "{chunk:?}");
        }
        let mut d = Detector::new("hermes", Phase::Preparing);
        let s = d.feed("Welcome to Hermes Agent! Type your message or /help.\n");
        assert!(s.contains(&Signal::Phase(Phase::Ready, "welcome")), "{s:?}");
        // While still installing a prompt sequence means nothing.
        let mut i = Detector::new("hermes", Phase::Installing);
        assert!(i.feed("\x1b[?2004h").iter().all(|s| !matches!(s, Signal::Phase(..))));
    }

    #[test]
    fn lines_are_cleaned_split_on_cr_and_spinners_dropped() {
        let mut d = Detector::new("hermes", Phase::Installing);
        let s = d.feed("\x1b[32m  Fetching   index \x1b[0m\r50%\r\x1b[2K⠋\nplain line here\n");
        let lines: Vec<&str> = s
            .iter()
            .filter_map(|x| if let Signal::Line(l) = x { Some(l.as_str()) } else { None })
            .collect();
        assert_eq!(lines, vec!["Fetching index", "plain line here"]);
    }

    #[test]
    fn classify_failure_table() {
        use Phase::*;
        assert_eq!(classify_failure(Installing, "curl: (6) Could not resolve host: hermes-agent.nousresearch.com"), "network");
        assert_eq!(classify_failure(Installing, "Unable to connect to the remote server"), "network");
        assert_eq!(classify_failure(Installing, "Error: ENOTFOUND registry"), "network");
        assert_eq!(classify_failure(Installing, "pip failed: Disk full"), "installer_error");
        assert_eq!(classify_failure(Starting, "openai.AuthenticationError: Error code: 401 - invalid api key"), "no_key");
        assert_eq!(classify_failure(Preparing, "No API keys or providers found"), "no_key");
        assert_eq!(classify_failure(Starting, "'hermes' is not recognized as an internal or external command"), "engine_missing");
        assert_eq!(classify_failure(Starting, "Traceback ... ValueError"), "engine_exit");
        // The installer is never blamed on a key.
        assert_eq!(classify_failure(Installing, "401 unauthorized from mirror"), "installer_error");
        // Network wins over a key message (offline retries show the same text).
        assert_eq!(classify_failure(Starting, "getaddrinfo failed; unauthorized"), "network");
    }

    // ---- watch lifecycle (recorder emitter, global map keyed per test) ------

    type Rec = Arc<Mutex<Vec<Value>>>;
    fn recorder() -> (Emitter, Rec) {
        let rec: Rec = Arc::new(Mutex::new(Vec::new()));
        let r2 = rec.clone();
        (Arc::new(move |v| r2.lock_safe().push(v)), rec)
    }
    fn phases(rec: &Rec) -> Vec<String> {
        rec.lock_safe().iter().map(|e| e["phase"].as_str().unwrap().to_string()).collect()
    }
    fn uniq(tag: &str) -> String {
        format!("{tag}-{}", uuid::Uuid::new_v4().simple())
    }

    #[test]
    fn full_first_run_sequence_is_emitted_in_order() {
        let (emit, rec) = recorder();
        let pty = uniq("pty");
        begin_with(emit, &pty, "agent-a", "hermes", Phase::Checking, "binary");
        with_watch(&pty, |w| vec![w.enter(Phase::Installing, "native", None, Instant::now())]);
        observe_output(&pty, "Downloading Python runtime\n");
        // Installer exits 0 with a relaunch armed -> starting.
        on_exit(&pty, Some(0), "", true);
        observe_output(&pty, "preparing dependencies for this data root...\n");
        observe_output(&pty, "\x1b[?2004h");
        assert_eq!(phases(&rec), vec!["checking", "installing", "installing", "starting", "preparing", "ready"]);
        let evs = rec.lock_safe();
        assert_eq!(evs[0]["agentId"], "agent-a");
        assert_eq!(evs[0]["engine"], "hermes");
        assert!(evs[0]["ts"].as_i64().unwrap() > 0);
        assert_eq!(evs[1]["detail"], "native");
        assert_eq!(evs[2]["detail"], "python");
        assert_eq!(evs[4]["detail"], "preparing-deps");
        assert_eq!(evs[5]["detail"], "prompt");
        drop(evs);
        // Terminal phase dropped the watch: more output is ignored.
        observe_output(&pty, "preparing dependencies\n");
        assert_eq!(rec.lock_safe().len(), 6);
    }

    #[test]
    fn install_failure_is_classified_and_remembered_until_cleared() {
        let (emit, rec) = recorder();
        let (pty, agent) = (uniq("pty"), uniq("agent"));
        begin_with(emit, &pty, &agent, "hermes", Phase::Installing, "native");
        on_exit(&pty, Some(1), "\x1b[31mcurl: (6) Could not resolve host: x\x1b[0m\r\n", true);
        let last = rec.lock_safe().last().cloned().unwrap();
        assert_eq!(last["phase"], "failed");
        assert_eq!(last["reason"], "network");
        assert!(last["detail"].as_str().unwrap().contains("Could not resolve host"));
        // A reloaded window still sees the failure...
        assert!(snapshot().iter().any(|v| v["agentId"] == agent.as_str() && v["phase"] == "failed"));
        // ...until the user acts on it.
        clear_failure(&agent);
        assert!(!snapshot().iter().any(|v| v["agentId"] == agent.as_str()));
    }

    #[test]
    fn exit_before_ready_and_spawn_error_and_stop() {
        let (emit, rec) = recorder();
        let pty = uniq("pty");
        begin_with(emit, &pty, "a", "hermes", Phase::Starting, "launch");
        on_exit(&pty, Some(1), "No API keys or providers found\r\n", false);
        assert_eq!(rec.lock_safe().last().unwrap()["reason"], "no_key");

        let (emit, rec) = recorder();
        let pty = uniq("pty");
        begin_with(emit, &pty, "b", "hermes", Phase::Checking, "binary");
        spawn_failed(Some(&pty), "cwd does not exist: x");
        let last = rec.lock_safe().last().cloned().unwrap();
        assert_eq!((last["phase"].as_str(), last["reason"].as_str()), (Some("failed"), Some("spawn_error")));

        let (emit, rec) = recorder();
        let pty = uniq("pty");
        begin_with(emit, &pty, "c", "hermes", Phase::Preparing, "preparing-deps");
        forget(&pty);
        assert_eq!(phases(&rec), vec!["preparing", "stopped"]);
        forget(&pty); // idempotent
        assert_eq!(rec.lock_safe().len(), 2);
    }

    #[test]
    fn hook_event_means_ready_but_not_while_installing() {
        let (emit, rec) = recorder();
        let (pty, agent) = (uniq("pty"), uniq("agent"));
        begin_with(emit, &pty, &agent, "hermes", Phase::Installing, "native");
        note_hook(&agent);
        assert_eq!(phases(&rec), vec!["installing"]);
        with_watch(&pty, |w| vec![w.enter(Phase::Preparing, "preparing-deps", None, Instant::now())]);
        note_hook(&agent);
        assert_eq!(phases(&rec), vec!["installing", "preparing", "ready"]);
        assert_eq!(rec.lock_safe().last().unwrap()["detail"], "hook");
    }

    #[test]
    fn launched_without_key_fails_and_relaunch_is_not_repeated() {
        let (emit, rec) = recorder();
        let pty = uniq("pty");
        begin_with(emit, &pty, "a", "hermes", Phase::Checking, "binary");
        launched(&pty, false, true);
        assert_eq!(rec.lock_safe().last().unwrap()["reason"], "no_key");

        let (emit, rec) = recorder();
        let pty = uniq("pty");
        begin_with(emit, &pty, "a", "hermes", Phase::Installing, "native");
        on_exit(&pty, Some(0), "", true); // -> starting (relaunch)
        launched(&pty, true, false); // already starting: no duplicate event
        assert_eq!(phases(&rec), vec!["installing", "starting"]);
    }

    #[test]
    fn timers_end_a_stuck_setup_and_settle_a_quiet_start() {
        let (emit, _rec) = recorder();
        let pty = uniq("pty");
        begin_with(emit, &pty, "a", "hermes", Phase::Starting, "launch");
        let t0 = Instant::now();
        let mut map = WATCHES.lock_safe();
        let w = map.get_mut(&pty).unwrap();
        // No output at all: neither settles nor times out early.
        assert_eq!(w.tick(t0 + Duration::from_secs(60)), None);
        // Output seen, quiet long enough, no "preparing": settled -> ready.
        w.saw_output = true;
        w.last_output = Some(t0);
        assert_eq!(w.tick(t0 + SETTLE_AFTER + SETTLE_QUIET), Some((Phase::Ready, "settled", None)));
        // After a "preparing" marker the quiet settle must NOT fire (silent npm install).
        w.saw_preparing = true;
        assert_eq!(w.tick(t0 + SETTLE_AFTER + SETTLE_QUIET), None);
        // But the hard timeout ends it.
        assert_eq!(
            w.tick(t0 + START_TIMEOUT + Duration::from_secs(5)),
            Some((Phase::Failed, "timeout", Some("timeout")))
        );
        // Installing has its own, longer, limit.
        w.det.set_phase(Phase::Installing);
        assert_eq!(w.tick(t0 + Duration::from_secs(20 * 60)), None);
        assert_eq!(
            w.tick(t0 + INSTALL_TIMEOUT + Duration::from_secs(5)),
            Some((Phase::Failed, "timeout", Some("timeout")))
        );
        map.remove(&pty);
        ACTIVE.store(map.len(), Ordering::Relaxed);
    }

    #[test]
    fn line_events_are_throttled_and_flushed_by_the_ticker() {
        let (emit, rec) = recorder();
        let pty = uniq("pty");
        begin_with(emit, &pty, "a", "hermes", Phase::Installing, "native");
        observe_output(&pty, "first line here\n");
        observe_output(&pty, "second line here\n"); // inside the throttle window
        let n = rec.lock_safe().len();
        assert!(n >= 1 && rec.lock_safe().iter().any(|e| e["line"] == "first line here" || e["line"] == "second line here"));
        // The ticker later delivers the held-back newest line.
        tick_key(&pty, Instant::now() + Duration::from_secs(5));
        assert_eq!(rec.lock_safe().last().unwrap()["line"], "second line here");
        forget(&pty);
    }

    #[test]
    fn only_hermes_is_watched_unless_the_verification_hook_is_on() {
        // This is the only test that touches the variable.
        std::env::remove_var(WATCH_ALL_ENV);
        assert!(watches("hermes"));
        assert!(!watches("claude") && !watches("custom"));
        assert_eq!(marker_engine("custom"), "custom");
        std::env::set_var(WATCH_ALL_ENV, "1");
        assert!(watches("custom"));
        assert_eq!(marker_engine("custom"), "hermes");
        std::env::set_var(WATCH_ALL_ENV, "0");
        assert!(!watches("custom"));
        std::env::remove_var(WATCH_ALL_ENV);
    }

    #[test]
    fn user_config_status_reads_provider_and_key_presence_but_never_values() {
        let dir = std::env::temp_dir().join(format!("oa-hus-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // Nothing there.
        let v = hermes_user_config_status_at(&dir);
        assert_eq!(v["exists"], false);
        assert_eq!(v["hasKeys"], false);
        assert!(v["provider"].is_null());
        std::fs::write(
            dir.join("config.yaml"),
            "model:\n  provider: anthropic\n  default: claude-opus-4.6\nagent:\n  provider: other\n",
        )
        .unwrap();
        std::fs::write(dir.join(".env"), "ANTHROPIC_API_KEY=sk-ant-SECRET-VALUE\nFOO=bar\n").unwrap();
        let v = hermes_user_config_status_at(&dir);
        assert_eq!(v["exists"], true);
        assert_eq!(v["provider"], "anthropic");
        assert_eq!(v["model"], "claude-opus-4.6");
        assert_eq!(v["hasKeys"], true);
        assert!(!v.to_string().contains("SECRET"), "key value leaked: {v}");
        // An empty key line is not a key.
        std::fs::write(dir.join(".env"), "OPENROUTER_API_KEY=\n# ANTHROPIC_API_KEY=abc\n").unwrap();
        assert_eq!(hermes_user_config_status_at(&dir)["hasKeys"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
