//! Clean conversation text for the orchestrator's chat view (simple mode).
//!
//! The raw terminal is the only place an engine's replies appear on screen, which
//! is intimidating for a non-technical person. This module reads the engine's OWN
//! durable transcript instead and hands the UI just the human turns:
//!
//!   * HERMES (the default engine) — `<agent dir>/.hermes/state.db`, the SQLite
//!     `SessionDB` the Hermes CLI writes (hermes_state_common.py `SCHEMA_SQL`:
//!     `messages(id AUTOINCREMENT, session_id, role, content, timestamp REAL,
//!     active, compacted, display_kind, display_metadata, _compressed_summary …)`).
//!     Rows are flushed after every model step (agent/session_persistence.py
//!     `_flush_messages_to_session_db`, called from turn_api_call / tool_executor /
//!     turn_final_response), so the transcript is live. We open it READ-ONLY (the
//!     DB is WAL, many readers + one writer is its design), per poll, and never hold
//!     a connection. The cursor is the highest `messages.id` seen: ids are
//!     AUTOINCREMENT, i.e. strictly monotonic, so `id > cursor` is an exact
//!     incremental read with no partial-write hazard (a row is visible only once its
//!     transaction committed).
//!   * CLAUDE — the transcript JSONL `<claude home>/.claude/projects/<key(cwd)>/
//!     <session id>.jsonl` (usage.rs `project_key`; `AGENT_OFFICE_CLAUDE_HOME`
//!     override via spawn_ext::claude_home). Append-only lines; the cursor is a BYTE
//!     OFFSET after the last complete (newline-terminated) line, so a half-written
//!     trailing line is simply not consumed yet.
//!   * anything else, or no structured source yet → `source: "none"` and the UI
//!     keeps the raw terminal as the primary view.
//!
//! Hooks were considered and rejected: Hermes' `agent_loop_stopped` fires only when
//! a turn is INTERRUPTED (plugins.py VALID_HOOKS comment), and `post_llm_call`
//! carries the final text but is not wired by our hive-hook and carries no user
//! turns. `sessions/*.json` under HERMES_HOME are request DUMPS (they can contain
//! credentials) and are never read.
//!
//! Output is only user/assistant text. Tool calls and tool results are omitted
//! (the UI's "working…" indicator covers them). Internal app-injected prompts (the
//! orientation prompt, inbox nudges, work orders, /compact) never appear as the
//! person's own bubbles. Text is stripped of ANSI/control characters, capped,
//! and obvious credential shapes are redacted. Nothing here reads the process
//! environment or any `.env` file.
//!
//! Every parsing function is pure and fixture-tested below (formats written by
//! hand; no real session is ever read).

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use serde::Serialize;
use serde_json::Value;

/// Per-message cap (chars). Longer replies are cut with an ellipsis.
pub const MAX_MESSAGE_CHARS: usize = 16_000;
/// Per-call cap on messages returned (newest kept).
pub const MAX_BATCH_MESSAGES: usize = 400;
/// Per-call cap on total chars returned (newest kept).
pub const MAX_BATCH_CHARS: usize = 300_000;
/// First load (no cursor): only the newest this-many messages.
pub const FIRST_LOAD_MESSAGES: usize = 120;
/// A transcript line longer than this (tool output with embedded images …) is skipped.
pub const MAX_LINE_BYTES: usize = 1 << 20;
/// Most bytes of a transcript consumed per call.
pub const MAX_READ_BYTES: u64 = 4 << 20;
/// First load of a huge transcript starts this far from the end.
pub const FIRST_LOAD_TAIL_BYTES: u64 = 2 << 20;
/// Raw Hermes rows fetched per call.
const HERMES_ROW_LIMIT: i64 = 400;
/// Hermes `content` is read through `substr(.., 1, N)` so a pathological row cannot blow memory.
const HERMES_CONTENT_CHARS: i64 = 1_000_000;

// ─── wire types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConvMessage {
    pub id: String,
    /// "user" | "assistant" | "system"
    pub role: String,
    pub text: String,
    /// Epoch milliseconds, 0 when the source carries none.
    pub ts: i64,
    /// Only on `system` rows: "team" = a background update from the team (the UI
    /// words it; `text` is empty so nothing internal ever leaks).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvResult {
    pub messages: Vec<ConvMessage>,
    /// Opaque; send it back as `since_cursor`.
    pub cursor: String,
    /// "hermes" | "claude" | "none"
    pub source: &'static str,
    /// Only with source "none": "unsupported_engine" | "starting" | "unreadable".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    /// The cursor no longer matched the source (new session, truncated file):
    /// the UI must REPLACE its list with `messages`.
    pub reset: bool,
    /// More already-written data is waiting; poll again straight away.
    pub more: bool,
}

impl ConvResult {
    fn none(reason: &'static str) -> Self {
        ConvResult {
            messages: Vec::new(),
            cursor: String::new(),
            source: "none",
            reason: Some(reason),
            reset: false,
            more: false,
        }
    }
}

// ─── text hygiene (pure) ────────────────────────────────────────────────────

/// Strip ANSI escape sequences (CSI, OSC, DCS/PM/APC strings, two-byte escapes),
/// C0/C1 control characters (keeping `\n` and `\t`), bidi overrides, the BOM and
/// lone carriage returns; collapse 3+ blank lines; trim; cap at `max_chars`.
pub fn sanitize_text(raw: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(raw.len().min(max_chars.saturating_add(8)));
    let mut it = raw.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\u{1b}' => match it.peek().copied() {
                Some('[') => {
                    it.next();
                    // CSI: parameters/intermediates then one final byte 0x40..=0x7E
                    for n in it.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') | Some('P') | Some('^') | Some('_') | Some('X') => {
                    it.next();
                    // OSC / DCS / PM / APC / SOS: until BEL or ST (ESC \)
                    while let Some(n) = it.next() {
                        if n == '\u{7}' {
                            break;
                        }
                        if n == '\u{1b}' {
                            if it.peek() == Some(&'\\') {
                                it.next();
                            }
                            break;
                        }
                    }
                }
                Some(_) => {
                    it.next(); // two-byte escape (ESC c, ESC 7, ESC ( B takes one more, harmless)
                }
                None => {}
            },
            // C1 CSI introducer
            '\u{9b}' => {
                for n in it.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&n) {
                        break;
                    }
                }
            }
            '\n' | '\t' => out.push(c),
            '\r' => {
                // CRLF → the LF that follows; a lone CR (progress redraw) is dropped
            }
            c if (c as u32) < 0x20 || c == '\u{7f}' || ((0x80..=0x9f).contains(&(c as u32))) => {}
            '\u{202a}'..='\u{202e}' | '\u{feff}' => {}
            c => out.push(c),
        }
    }
    // collapse 3+ newlines, trim trailing spaces on each line end of the whole text
    let mut collapsed = String::with_capacity(out.len());
    let mut nl = 0usize;
    for c in out.chars() {
        if c == '\n' {
            nl += 1;
            if nl <= 2 {
                collapsed.push(c);
            }
        } else {
            nl = 0;
            collapsed.push(c);
        }
    }
    let trimmed = collapsed.trim();
    if trimmed.chars().count() > max_chars {
        let mut cut: String = trimmed.chars().take(max_chars).collect();
        cut.push_str("\n\n…");
        cut
    } else {
        trimmed.to_string()
    }
}

fn secret_patterns() -> &'static Vec<(Regex, &'static str)> {
    static P: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    P.get_or_init(|| {
        let mk = |p: &str, r: &'static str| (Regex::new(p).expect("secret regex"), r);
        vec![
            mk(
                r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?(?:-----END [A-Z ]*PRIVATE KEY-----|$)",
                "[hidden key]",
            ),
            mk(r"\b(?:sk|pk|rk)-[A-Za-z0-9_\-]{16,}", "[hidden key]"),
            mk(r"\bgh[pousr]_[A-Za-z0-9]{20,}", "[hidden key]"),
            mk(r"\bgithub_pat_[A-Za-z0-9_]{20,}", "[hidden key]"),
            mk(r"\bxox[abprs]-[A-Za-z0-9\-]{10,}", "[hidden key]"),
            mk(r"\bAKIA[0-9A-Z]{16}\b", "[hidden key]"),
            mk(r"\bAIza[0-9A-Za-z_\-]{30,}", "[hidden key]"),
            mk(
                r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}",
                "[hidden key]",
            ),
            mk(r"(?i)\b(bearer\s+)[A-Za-z0-9._~+/=\-]{20,}", "${1}[hidden]"),
            // NAME_API_KEY=value / export TOKEN: value — an env-style assignment line
            mk(
                r"(?im)^(\s*(?:export\s+)?[A-Z][A-Z0-9_]*(?:API_KEY|APIKEY|SECRET|TOKEN|PASSWORD|PASSWD|PRIVATE_KEY|ACCESS_KEY)[A-Z0-9_]*\s*[=:]\s*)\S+",
                "${1}[hidden]",
            ),
        ]
    })
}

/// Replace obvious credential shapes. Defence in depth: Hermes redacts at the
/// source and this module never touches env, but a model can still echo a key.
pub fn redact_secrets(text: &str) -> String {
    let mut s = text.to_string();
    for (re, rep) in secret_patterns() {
        if re.is_match(&s) {
            s = re.replace_all(&s, *rep).into_owned();
        }
    }
    s
}

fn clean(raw: &str) -> String {
    redact_secrets(&sanitize_text(raw, MAX_MESSAGE_CHARS))
}

// ─── who is "the person" (pure) ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserKind {
    /// Typed by the human (or arrived from Slack on their behalf).
    Person,
    /// A background update the app injected on behalf of the team (inbox nudge, work order).
    Team,
    /// App plumbing that should not be shown at all (orientation prompt, /compact).
    Hidden,
}

/// Drop a leading `<goal>…</goal>` block (useHive `withStandingGoal` prepends it
/// for engines that cannot inject the goal through hooks).
pub fn strip_goal_block(text: &str) -> &str {
    let t = text.trim_start();
    if let Some(rest) = t.strip_prefix("<goal>") {
        if let Some(end) = rest.find("</goal>") {
            return rest[end + "</goal>".len()..].trim_start();
        }
    }
    text
}

/// Classify a user-role turn. Mirrors `isInternalPrompt` in
/// src/shared/conversation.ts — keep the two lists identical (a test pins the
/// head strings against src/shared/hiveNudge.ts and useHive.ts text).
pub fn classify_user_text(text: &str) -> UserKind {
    let t = strip_goal_block(text).trim_start();
    if t.starts_with("You have new hive inbox message(s)")
        || t.starts_with("WORK ORDER FROM HIVE")
        || t.starts_with("Terminal handoff failed for ")
    {
        return UserKind::Team;
    }
    if (t.starts_with("You're online as ") && t.contains("orchestrator of the hive"))
        // hive protocol prompt typed into engines that cannot take it any other way
        || (t.starts_with("You are \"") && t.contains("an autonomous agent in a collaborating hive"))
        || t.starts_with("[System:")
    {
        return UserKind::Hidden;
    }
    let first = t.split_whitespace().next().unwrap_or("");
    if first == "/compact" || first == "/clear" {
        return UserKind::Hidden;
    }
    UserKind::Person
}

fn user_message(id: String, raw: &str, ts: i64) -> Option<ConvMessage> {
    let stripped = strip_goal_block(raw);
    match classify_user_text(stripped) {
        UserKind::Hidden => None,
        UserKind::Team => Some(ConvMessage {
            id,
            role: "system".into(),
            text: String::new(),
            ts,
            kind: Some("team".into()),
        }),
        UserKind::Person => {
            let text = clean(stripped);
            (!text.is_empty()).then(|| ConvMessage { id, role: "user".into(), text, ts, kind: None })
        }
    }
}

fn assistant_message(id: String, raw: &str, ts: i64) -> Option<ConvMessage> {
    let text = clean(raw);
    (!text.is_empty()).then(|| ConvMessage { id, role: "assistant".into(), text, ts, kind: None })
}

/// Same message twice (compaction clones rows; Claude resumes copy history): keep
/// the first by (role, ts, text). Consecutive team updates collapse to one.
pub fn dedupe_messages(msgs: Vec<ConvMessage>) -> Vec<ConvMessage> {
    let mut seen: HashSet<(String, i64, String)> = HashSet::new();
    let mut out: Vec<ConvMessage> = Vec::with_capacity(msgs.len());
    for m in msgs {
        if !seen.insert((m.role.clone(), m.ts, m.text.clone())) {
            continue;
        }
        if m.kind.as_deref() == Some("team")
            && out.last().is_some_and(|p| p.kind.as_deref() == Some("team"))
        {
            continue;
        }
        out.push(m);
    }
    out
}

/// Keep the NEWEST messages that fit the per-call caps.
pub fn cap_batch(mut msgs: Vec<ConvMessage>, max_msgs: usize, max_chars: usize) -> Vec<ConvMessage> {
    if msgs.len() > max_msgs {
        msgs.drain(..msgs.len() - max_msgs);
    }
    let mut total = 0usize;
    let mut keep_from = msgs.len();
    for (i, m) in msgs.iter().enumerate().rev() {
        total += m.text.chars().count();
        if total > max_chars && keep_from != msgs.len() {
            break;
        }
        keep_from = i;
    }
    msgs.drain(..keep_from);
    msgs
}

// ─── cursors (pure) ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cursor {
    Hermes(i64),
    Claude { session: String, offset: u64 },
}

pub fn parse_cursor(s: &str) -> Option<Cursor> {
    if let Some(rest) = s.strip_prefix("h:") {
        return rest.parse::<i64>().ok().filter(|n| *n >= 0).map(Cursor::Hermes);
    }
    if let Some(rest) = s.strip_prefix("c:") {
        let (session, off) = rest.rsplit_once(':')?;
        if session.is_empty() {
            return None;
        }
        return Some(Cursor::Claude { session: session.to_string(), offset: off.parse().ok()? });
    }
    None
}

pub fn hermes_cursor(id: i64) -> String {
    format!("h:{id}")
}

pub fn claude_cursor(session: &str, offset: u64) -> String {
    format!("c:{session}:{offset}")
}

// ─── Hermes (pure parse + read-only SQLite read) ────────────────────────────

/// One `messages` row as read from state.db (only the columns we use).
#[derive(Debug, Clone, Default)]
pub struct HermesRow {
    pub id: i64,
    pub role: String,
    pub content: Option<String>,
    /// Epoch SECONDS (REAL).
    pub ts_secs: f64,
    pub active: bool,
    pub compacted: bool,
    pub display_kind: Option<String>,
    pub display_metadata: Option<String>,
    pub summary: bool,
}

const HERMES_JSON_PREFIX: &str = "\u{0}json:";

/// Hermes stores list/dict content (multimodal parts) as `"\0json:" + JSON`
/// (hermes_state.py `_CONTENT_JSON_PREFIX`). Returns the text parts joined, an
/// `[image]` marker per image part; a truncated/invalid payload gives "".
pub fn decode_hermes_content(raw: &str) -> String {
    let Some(body) = raw.strip_prefix(HERMES_JSON_PREFIX) else {
        return raw.to_string();
    };
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return String::new();
    };
    fn flat(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Array(a) => a.iter().for_each(|x| flat(x, out)),
            Value::Object(o) => {
                let ty = o.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if ty.contains("image") {
                    out.push("[image]".into());
                } else if let Some(t) = o.get("text").and_then(|t| t.as_str()) {
                    out.push(t.to_string());
                }
            }
            _ => {}
        }
    }
    let mut parts = Vec::new();
    flat(&v, &mut parts);
    parts.join("\n")
}

/// `display_metadata.model_only` rows are model-facing scaffolding that Hermes
/// itself never paints (context_compressor.py MODEL_ONLY_DISPLAY_METADATA_KEY).
/// Old rows can be double-encoded JSON.
fn is_model_only(meta: &Option<String>) -> bool {
    let Some(raw) = meta else { return false };
    let mut v: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return false,
    };
    if let Value::String(inner) = &v {
        v = serde_json::from_str(inner).unwrap_or(Value::Null);
    }
    match v.get("model_only") {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        _ => false,
    }
}

/// Rows (in id order) → display messages. Pure.
pub fn parse_hermes_rows(rows: &[HermesRow]) -> Vec<ConvMessage> {
    let mut out = Vec::new();
    for r in rows {
        if r.role != "user" && r.role != "assistant" {
            continue;
        }
        if !(r.active || r.compacted) || r.summary {
            continue;
        }
        match r.display_kind.as_deref() {
            None | Some("") | Some("steer") => {}
            _ => continue, // hidden / model_switch / async_delegation_complete / …
        }
        if is_model_only(&r.display_metadata) {
            continue;
        }
        let Some(raw) = r.content.as_deref() else { continue };
        let text = decode_hermes_content(raw);
        if text.trim().is_empty() {
            continue; // assistant rows that only carry tool_calls
        }
        let id = format!("h{}", r.id);
        let ts = (r.ts_secs * 1000.0) as i64;
        let m = if r.role == "user" {
            user_message(id, &text, ts)
        } else {
            assistant_message(id, &text, ts)
        };
        out.extend(m);
    }
    dedupe_messages(out)
}

#[derive(Debug, PartialEq, Eq)]
pub enum HermesErr {
    /// No state.db yet (engine still starting).
    Missing,
    /// Not a database / schema we do not understand.
    Unreadable,
    /// Locked or busy right now — try again next poll.
    Transient,
}

fn map_sqlite_err(e: &rusqlite::Error) -> HermesErr {
    use rusqlite::ErrorCode as C;
    match e {
        rusqlite::Error::SqliteFailure(f, _) => match f.code {
            C::DatabaseBusy | C::DatabaseLocked | C::SystemIoFailure | C::FileLockingProtocolFailed => {
                HermesErr::Transient
            }
            C::CannotOpen => HermesErr::Missing,
            _ => HermesErr::Unreadable,
        },
        _ => HermesErr::Unreadable,
    }
}

fn table_columns(conn: &rusqlite::Connection, table: &str) -> Result<HashSet<String>, rusqlite::Error> {
    // `table` is one of two literals below, never user input.
    let mut st = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = st.query_map([], |r| r.get::<_, String>(1))?;
    rows.collect::<Result<HashSet<_>, _>>()
}

fn opt_text(v: rusqlite::types::Value) -> Option<String> {
    use rusqlite::types::Value as V;
    match v {
        V::Text(s) => Some(s),
        V::Blob(b) => Some(String::from_utf8_lossy(&b).into_owned()),
        V::Integer(i) => Some(i.to_string()),
        V::Real(f) => Some(f.to_string()),
        V::Null => None,
    }
}

fn truthy(v: rusqlite::types::Value) -> bool {
    use rusqlite::types::Value as V;
    match v {
        V::Integer(i) => i != 0,
        V::Real(f) => f != 0.0,
        V::Text(s) => s == "1" || s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// Read rows after `since` (or the newest ones on a first load) from a Hermes
/// state.db, read-only. Returns (messages, new cursor id, more, reset).
pub fn read_hermes_db(
    db: &Path,
    since: Option<i64>,
) -> Result<(Vec<ConvMessage>, i64, bool, bool), HermesErr> {
    use rusqlite::{Connection, OpenFlags};
    if !db.is_file() {
        return Err(HermesErr::Missing);
    }
    let conn = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| map_sqlite_err(&e))?;
    let _ = conn.busy_timeout(Duration::from_millis(250));

    let mcols = table_columns(&conn, "messages").map_err(|e| map_sqlite_err(&e))?;
    for need in ["id", "session_id", "role", "content", "timestamp"] {
        if !mcols.contains(need) {
            return Err(HermesErr::Unreadable);
        }
    }
    let scols = table_columns(&conn, "sessions").unwrap_or_default();

    let col = |name: &str, fallback: &str| -> String {
        if mcols.contains(name) {
            format!("m.{name}")
        } else {
            fallback.to_string()
        }
    };
    let active = col("active", "1");
    let compacted = col("compacted", "0");
    let display_kind = col("display_kind", "NULL");
    let display_meta = col("display_metadata", "NULL");
    let summary = col("_compressed_summary", "0");

    let mut wheres = vec!["m.role IN ('user','assistant')".to_string(), "m.id > ?1".to_string()];
    if mcols.contains("active") && mcols.contains("compacted") {
        wheres.push("(m.active = 1 OR m.compacted = 1)".into());
    }
    let mut join = String::new();
    if scols.contains("id") {
        join.push_str(" LEFT JOIN sessions s ON s.id = m.session_id");
        // Sub-agent / tool sessions are not the person's conversation.
        if scols.contains("source") {
            wheres.push("COALESCE(s.source,'') <> 'tool'".into());
        }
        if scols.contains("model_config") {
            wheres.push("COALESCE(s.model_config,'') NOT LIKE '%_delegate_from%'".into());
        }
    }

    // A cursor ahead of the table (DB replaced / reset) → start over.
    let max_id: i64 = conn
        .query_row("SELECT COALESCE(MAX(id),0) FROM messages", [], |r| r.get(0))
        .map_err(|e| map_sqlite_err(&e))?;
    let (since_eff, reset) = match since {
        Some(c) if c > max_id => (0, true),
        Some(c) => (c, false),
        None => (0, false),
    };
    let first_load = since_eff == 0;
    let order = if first_load { "DESC" } else { "ASC" };
    let sql = format!(
        "SELECT m.id, m.role, substr(m.content,1,{HERMES_CONTENT_CHARS}), m.timestamp, {active}, {compacted}, {display_kind}, {display_meta}, {summary} \
         FROM messages m{join} WHERE {} ORDER BY m.id {order} LIMIT {HERMES_ROW_LIMIT}",
        wheres.join(" AND ")
    );
    let mut st = conn.prepare(&sql).map_err(|e| map_sqlite_err(&e))?;
    let mapped = st
        .query_map([since_eff], |r| {
            Ok(HermesRow {
                id: r.get(0)?,
                role: r.get::<_, String>(1)?,
                content: opt_text(r.get(2)?),
                ts_secs: r.get::<_, rusqlite::types::Value>(3).map(|v| match v {
                    rusqlite::types::Value::Real(f) => f,
                    rusqlite::types::Value::Integer(i) => i as f64,
                    _ => 0.0,
                })?,
                active: truthy(r.get(4)?),
                compacted: truthy(r.get(5)?),
                display_kind: opt_text(r.get(6)?),
                display_metadata: opt_text(r.get(7)?),
                summary: truthy(r.get(8)?),
            })
        })
        .map_err(|e| map_sqlite_err(&e))?;
    let mut rows: Vec<HermesRow> = Vec::new();
    for r in mapped {
        rows.push(r.map_err(|e| map_sqlite_err(&e))?);
    }
    let more = !first_load && rows.len() as i64 >= HERMES_ROW_LIMIT;
    if first_load {
        rows.reverse();
    }
    let cursor = rows.iter().map(|r| r.id).max().unwrap_or(since_eff);
    let mut msgs = parse_hermes_rows(&rows);
    if first_load && msgs.len() > FIRST_LOAD_MESSAGES {
        msgs.drain(..msgs.len() - FIRST_LOAD_MESSAGES);
    }
    Ok((cap_batch(msgs, MAX_BATCH_MESSAGES, MAX_BATCH_CHARS), cursor, more, reset))
}

// ─── Claude transcript (pure parse + incremental file read) ─────────────────

fn rfc3339_ms(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s).map(|t| t.timestamp_millis()).unwrap_or(0)
}

fn fnv_hex(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// Text blocks of a Claude `message.content` (string or block array); thinking,
/// tool_use, tool_result and images are skipped. `None` when only non-text.
fn claude_text(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(blocks) => {
            let texts: Vec<&str> = blocks
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect();
            (!texts.is_empty()).then(|| texts.join("\n\n"))
        }
        _ => None,
    }
}

/// Lines the Claude CLI writes as `user` turns that are not the person's words.
fn claude_user_noise(t: &str) -> bool {
    let t = t.trim_start();
    t.starts_with("<command-name>")
        || t.starts_with("<command-message>")
        || t.starts_with("<local-command-")
        || t.starts_with("<system-reminder>")
        || t.starts_with("Caveat: The messages below were generated by the user while running local commands")
        || t.starts_with("[Request interrupted by user")
}

/// Complete transcript lines → display messages. Pure. Invalid JSON lines are skipped.
pub fn parse_claude_lines(lines: &[String]) -> Vec<ConvMessage> {
    let mut out = Vec::new();
    for line in lines {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<Value>(l) else { continue };
        let ty = rec.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if ty != "user" && ty != "assistant" {
            continue;
        }
        if rec.get("isSidechain").and_then(|v| v.as_bool()).unwrap_or(false)
            || rec.get("isMeta").and_then(|v| v.as_bool()).unwrap_or(false)
        {
            continue;
        }
        let Some(msg) = rec.get("message") else { continue };
        if msg.get("model").and_then(|v| v.as_str()) == Some("<synthetic>") {
            continue;
        }
        let Some(raw) = msg.get("content").and_then(claude_text) else { continue };
        let id = format!(
            "c{}",
            rec.get("uuid").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| fnv_hex(l))
        );
        let ts = rec.get("timestamp").and_then(|v| v.as_str()).map(rfc3339_ms).unwrap_or(0);
        let m = if ty == "user" {
            if claude_user_noise(&raw) {
                continue;
            }
            user_message(id, &raw, ts)
        } else {
            assistant_message(id, &raw, ts)
        };
        out.extend(m);
    }
    dedupe_messages(out)
}

/// Read COMPLETE (newline-terminated) lines from `r`, at most `max_total` bytes
/// (stopping at a line boundary once reached). A line longer than `max_line`
/// is consumed but not returned (giant tool outputs). A trailing line with no
/// newline yet is NOT consumed — it is still being written. `drop_first_line`
/// discards the first complete line (used when starting mid-file).
/// Returns (lines, bytes consumed).
pub fn scan_lines<R: BufRead>(
    r: &mut R,
    max_line: usize,
    max_total: u64,
    drop_first_line: bool,
) -> std::io::Result<(Vec<String>, u64)> {
    let mut lines = Vec::new();
    let mut consumed: u64 = 0;
    let mut cur: Vec<u8> = Vec::new();
    let mut cur_bytes: u64 = 0; // bytes of the current (unterminated) line, incl. discarded ones
    let mut overflow = false;
    let mut dropped_first = !drop_first_line;
    loop {
        let (used, done_line) = {
            let buf = r.fill_buf()?;
            if buf.is_empty() {
                break; // EOF: any pending partial line stays unconsumed
            }
            match buf.iter().position(|b| *b == b'\n') {
                Some(i) => {
                    if !overflow {
                        if cur.len() + i > max_line {
                            overflow = true;
                            cur.clear();
                        } else {
                            cur.extend_from_slice(&buf[..i]);
                        }
                    }
                    cur_bytes += i as u64 + 1;
                    (i + 1, true)
                }
                None => {
                    if !overflow {
                        if cur.len() + buf.len() > max_line {
                            overflow = true;
                            cur.clear();
                        } else {
                            cur.extend_from_slice(buf);
                        }
                    }
                    cur_bytes += buf.len() as u64;
                    (buf.len(), false)
                }
            }
        };
        r.consume(used);
        if done_line {
            consumed += cur_bytes;
            if !overflow {
                if dropped_first {
                    lines.push(String::from_utf8_lossy(&cur).into_owned());
                } else {
                    dropped_first = true;
                }
            } else if !dropped_first {
                dropped_first = true;
            }
            cur.clear();
            cur_bytes = 0;
            overflow = false;
            if consumed >= max_total {
                break;
            }
        }
    }
    Ok((lines, consumed))
}

/// Locate `<session>.jsonl`: the expected project dir first, then any project
/// dir (a worker in a git worktree runs under a different cwd than its registry
/// row). The session id is a filename segment, so it is validated first.
pub fn find_claude_transcript(projects_root: &Path, cwd: &str, session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty()
        || !session_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    let direct = projects_root
        .join(crate::usage::project_key(cwd))
        .join(format!("{session_id}.jsonl"));
    if direct.is_file() {
        return Some(direct);
    }
    for e in std::fs::read_dir(projects_root).ok()?.flatten() {
        let p = e.path().join(format!("{session_id}.jsonl"));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Incremental read of one Claude transcript.
pub fn read_claude_file(
    path: &Path,
    session_id: &str,
    since: Option<&Cursor>,
) -> Result<ConvResult, std::io::Error> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let mut reset = false;
    let mut first_load = false;
    let mut drop_first = false;
    let mut off = match since {
        Some(Cursor::Claude { session, offset }) if session == session_id => {
            if *offset > len {
                reset = true;
                0
            } else {
                *offset
            }
        }
        Some(_) => {
            reset = true;
            0
        }
        None => 0,
    };
    if off == 0 {
        first_load = true;
        if len > FIRST_LOAD_TAIL_BYTES {
            off = len - FIRST_LOAD_TAIL_BYTES;
            // Mid-file start: is `off` already a line start?
            f.seek(SeekFrom::Start(off - 1))?;
            let mut prev = [0u8; 1];
            f.read_exact(&mut prev)?;
            drop_first = prev[0] != b'\n';
        }
    }
    f.seek(SeekFrom::Start(off))?;
    let mut rd = BufReader::with_capacity(64 * 1024, f);
    let (lines, consumed) = scan_lines(&mut rd, MAX_LINE_BYTES, MAX_READ_BYTES, drop_first)?;
    let new_off = off + consumed;
    let mut msgs = parse_claude_lines(&lines);
    if first_load && msgs.len() > FIRST_LOAD_MESSAGES {
        msgs.drain(..msgs.len() - FIRST_LOAD_MESSAGES);
    }
    Ok(ConvResult {
        messages: cap_batch(msgs, MAX_BATCH_MESSAGES, MAX_BATCH_CHARS),
        cursor: claude_cursor(session_id, new_off),
        source: "claude",
        reason: None,
        reset,
        more: new_off < len && consumed > 0 && consumed >= MAX_READ_BYTES,
    })
}

// ─── the command core (testable: every path is a parameter) ─────────────────

pub struct ReadParams<'a> {
    /// Registry provider id ("hermes", "claude", …).
    pub provider: &'a str,
    /// `<hive root>/agents/<id>` — HERMES_HOME is `<agent dir>/.hermes`.
    pub agent_dir: &'a Path,
    pub cwd: &'a str,
    /// The CLI's own session id from the registry (Claude hook payloads).
    pub session_id: Option<&'a str>,
    /// `<claude home>/.claude/projects`, when known.
    pub claude_projects_root: Option<&'a Path>,
}

pub fn read_conversation(p: &ReadParams, since_cursor: Option<&str>) -> ConvResult {
    let since = since_cursor.filter(|s| !s.is_empty()).and_then(parse_cursor);
    match p.provider {
        "hermes" => {
            let db = p.agent_dir.join(".hermes").join("state.db");
            let since_id = match since {
                Some(Cursor::Hermes(n)) => Some(n),
                _ => None,
            };
            // A cursor from another source (engine switched) is a reset.
            let foreign = since.is_some() && since_id.is_none();
            match read_hermes_db(&db, since_id) {
                Ok((messages, cursor, more, reset)) => ConvResult {
                    messages,
                    cursor: hermes_cursor(cursor),
                    source: "hermes",
                    reason: None,
                    reset: reset || foreign,
                    more,
                },
                Err(HermesErr::Missing) => ConvResult::none("starting"),
                Err(HermesErr::Unreadable) => ConvResult::none("unreadable"),
                // Busy right now: say nothing new, keep the cursor.
                Err(HermesErr::Transient) => ConvResult {
                    messages: Vec::new(),
                    cursor: since_cursor.unwrap_or("").to_string(),
                    source: "hermes",
                    reason: None,
                    reset: false,
                    more: false,
                },
            }
        }
        "claude" => {
            let (Some(root), Some(sid)) = (p.claude_projects_root, p.session_id.filter(|s| !s.is_empty())) else {
                return ConvResult::none("starting");
            };
            let Some(path) = find_claude_transcript(root, p.cwd, sid) else {
                return ConvResult::none("starting");
            };
            match read_claude_file(&path, sid, since.as_ref()) {
                Ok(r) => r,
                Err(_) => ConvResult::none("unreadable"),
            }
        }
        _ => ConvResult::none("unsupported_engine"),
    }
}

// ─── IPC ────────────────────────────────────────────────────────────────────

/// `conversation_read(agentId, sinceCursor?)` → `{messages, cursor, source, reason?, reset, more}`.
#[tauri::command(async)]
pub fn conversation_read(
    app: tauri::AppHandle,
    agent_id: String,
    since_cursor: Option<String>,
) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("conversation_read", || {
        let st = crate::commands::state(&app);
        let Some(entry) = st.hive.registry().into_iter().find(|a| a.id == agent_id) else {
            return Ok(serde_json::to_value(ConvResult::none("starting")).unwrap_or(Value::Null));
        };
        let agent_dir = st.hive.agent_dir(&entry.id);
        let projects_root = crate::spawn_ext::claude_home().map(|h| h.join(".claude").join("projects"));
        let res = read_conversation(
            &ReadParams {
                provider: &entry.provider,
                agent_dir: &agent_dir,
                cwd: &entry.cwd,
                session_id: entry.session_id.as_deref(),
                claude_projects_root: projects_root.as_deref(),
            },
            since_cursor.as_deref(),
        );
        serde_json::to_value(res).map_err(|e| e.to_string())
    })
}

// ─── tests (hand-written fixtures only; no real session is ever read) ───────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor as IoCursor;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ao-conv-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    // ── sanitize ──
    #[test]
    fn sanitize_strips_ansi_osc_and_controls() {
        let raw = "\u{1b}[1;31mred\u{1b}[0m \u{1b}]0;title\u{7}ok\u{1b}]8;;http://x\u{1b}\\link\u{1b}]8;;\u{1b}\\ a\u{0}b\u{8}c\u{7f}d\u{9b}31me\r\nline2\rX";
        assert_eq!(sanitize_text(raw, 1000), "red oklink abcde\nline2X");
    }

    #[test]
    fn sanitize_keeps_unicode_and_removes_bidi_overrides() {
        assert_eq!(sanitize_text("مرحبا 你好 \u{202e}evil\u{feff}", 100), "مرحبا 你好 evil");
    }

    #[test]
    fn sanitize_collapses_blank_runs_and_caps() {
        assert_eq!(sanitize_text("a\n\n\n\n\nb", 100), "a\n\nb");
        let long = "x".repeat(50);
        let s = sanitize_text(&long, 10);
        assert!(s.starts_with(&"x".repeat(10)) && s.ends_with('…'));
        assert!(sanitize_text(&"y".repeat(1_000_000), 100).chars().count() < 110);
    }

    #[test]
    fn sanitize_survives_dangling_escapes() {
        assert_eq!(sanitize_text("a\u{1b}", 10), "a");
        assert_eq!(sanitize_text("a\u{1b}[", 10), "a");
        assert_eq!(sanitize_text("a\u{1b}]no terminator", 100), "a");
    }

    #[test]
    fn redaction_hides_keys_but_not_prose() {
        let t = "key sk-ant-api03-abcdefghijklmnopqrstuv and ghp_abcdefghijklmnopqrstuvwxyz0123 and AKIAABCDEFGHIJKLMNOP\nOPENAI_API_KEY=abc123xyz\nexport GITHUB_TOKEN: tok_live_999\nAuthorization: Bearer abcdefghijklmnopqrstuvwxyz0123456789\nhello world token budget";
        let r = redact_secrets(t);
        assert!(!r.contains("abcdefghijklmnopqrstuv"));
        assert!(!r.contains("AKIAABCDEFGHIJKLMNOP"));
        assert!(!r.contains("abc123xyz") && !r.contains("tok_live_999"));
        assert!(!r.contains("0123456789"));
        assert!(r.contains("OPENAI_API_KEY=[hidden]"));
        assert!(r.contains("Bearer [hidden]"));
        assert!(r.contains("hello world token budget"));
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIabc\n-----END RSA PRIVATE KEY----- after";
        assert_eq!(redact_secrets(pem), "[hidden key] after");
    }

    // ── classification ──
    #[test]
    fn classify_recognises_app_injected_prompts() {
        assert_eq!(classify_user_text("hello Michael"), UserKind::Person);
        assert_eq!(classify_user_text("You have new hive inbox message(s) — at least: m1. Read"), UserKind::Team);
        assert_eq!(classify_user_text("WORK ORDER FROM HIVE\nMessage: x"), UserKind::Team);
        assert_eq!(classify_user_text("<goal>\nship it\n</goal>\n\nWORK ORDER FROM HIVE"), UserKind::Team);
        assert_eq!(classify_user_text("You're online as Michael, the orchestrator of the hive. Get oriented"), UserKind::Hidden);
        assert_eq!(
            classify_user_text("You are \"Michael\" (god), an autonomous agent in a collaborating hive of Claude agents.\nYour private workspace is X"),
            UserKind::Hidden
        );
        assert_eq!(classify_user_text("You are right about that, thanks"), UserKind::Person);
        assert_eq!(classify_user_text("/compact"), UserKind::Hidden);
        assert_eq!(classify_user_text("  /clear now"), UserKind::Hidden);
        assert_eq!(classify_user_text("[System: note]"), UserKind::Hidden);
        assert_eq!(classify_user_text("what is /compact?"), UserKind::Person);
        assert_eq!(strip_goal_block("<goal>g</goal>\n\nreal"), "real");
        assert_eq!(strip_goal_block("<goal>unterminated"), "<goal>unterminated");
    }

    /// The head strings are duplicated from the TS senders; fail loudly if they drift.
    #[test]
    fn injected_prompt_heads_match_the_ts_sources() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("src");
        let nudge = std::fs::read_to_string(root.join("shared").join("hiveNudge.ts")).unwrap();
        assert!(nudge.contains("You have new hive inbox message(s)"));
        let hive = std::fs::read_to_string(root.join("hooks").join("useHive.ts")).unwrap();
        for head in ["WORK ORDER FROM HIVE", "Terminal handoff failed for ", "orchestrator of the hive"] {
            assert!(hive.contains(head), "{head}");
        }
        assert!(hive.contains("You're online as ${godName}"));
        let hive_rs = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("hive.rs")).unwrap();
        assert!(
            hive_rs.contains("an autonomous agent in a collaborating hive"),
            "the hive protocol prompt head changed: update classify_user_text + isInternalPrompt"
        );
    }

    // ── dedupe / cap ──
    fn m(id: &str, role: &str, text: &str, ts: i64) -> ConvMessage {
        ConvMessage { id: id.into(), role: role.into(), text: text.into(), ts, kind: None }
    }

    #[test]
    fn dedupe_drops_clones_and_collapses_team_runs() {
        let mut team1 = m("a", "system", "", 1);
        team1.kind = Some("team".into());
        let mut team2 = m("b", "system", "", 2);
        team2.kind = Some("team".into());
        let v = dedupe_messages(vec![
            m("1", "user", "hi", 10),
            m("2", "user", "hi", 10), // clone
            m("3", "user", "hi", 11), // genuine repeat
            team1,
            team2,
            m("4", "assistant", "yo", 12),
        ]);
        let ids: Vec<&str> = v.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, ["1", "3", "a", "4"]);
    }

    #[test]
    fn cap_keeps_the_newest() {
        let v: Vec<ConvMessage> = (0..10).map(|i| m(&i.to_string(), "user", "abcd", i)).collect();
        let c = cap_batch(v.clone(), 3, 1000);
        assert_eq!(c.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["7", "8", "9"]);
        let c = cap_batch(v, 100, 9);
        assert_eq!(c.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["8", "9"]);
        // a single giant message is still returned
        assert_eq!(cap_batch(vec![m("1", "user", "abcdefgh", 1)], 5, 2).len(), 1);
    }

    // ── cursors ──
    #[test]
    fn cursor_roundtrip_and_garbage() {
        assert_eq!(parse_cursor(&hermes_cursor(42)), Some(Cursor::Hermes(42)));
        assert_eq!(
            parse_cursor(&claude_cursor("abc-1", 99)),
            Some(Cursor::Claude { session: "abc-1".into(), offset: 99 })
        );
        for bad in ["", "h:", "h:-1", "h:x", "c:abc", "c::5", "c:a:b", "zzz"] {
            assert_eq!(parse_cursor(bad), None, "{bad}");
        }
    }

    // ── Hermes: pure rows ──
    fn row(id: i64, role: &str, content: Option<&str>) -> HermesRow {
        HermesRow {
            id,
            role: role.into(),
            content: content.map(str::to_string),
            ts_secs: 1_700_000_000.5 + id as f64,
            active: true,
            ..Default::default()
        }
    }

    #[test]
    fn hermes_rows_keep_only_human_turns() {
        let mut hidden = row(5, "assistant", Some("scaffold"));
        hidden.display_kind = Some("hidden".into());
        let mut switch = row(6, "user", Some("switched model"));
        switch.display_kind = Some("model_switch".into());
        let mut steer = row(7, "user", Some("go left"));
        steer.display_kind = Some("steer".into());
        let mut inactive = row(8, "user", Some("undone"));
        inactive.active = false;
        let mut archived = row(9, "assistant", Some("old but kept"));
        archived.active = false;
        archived.compacted = true;
        let mut summary = row(10, "user", Some("summary of earlier turns"));
        summary.summary = true;
        let mut model_only = row(11, "user", Some("model only"));
        model_only.display_metadata = Some("{\"model_only\": true}".into());
        let mut double = row(12, "user", Some("double"));
        double.display_metadata = Some("\"{\\\"model_only\\\": 1}\"".into());
        let rows = vec![
            row(1, "user", Some("<goal>g</goal>\n\nhello \u{1b}[31mthere\u{1b}[0m")),
            row(2, "assistant", Some("")),          // tool-call-only row
            row(3, "tool", Some("{\"out\":1}")),    // tool result
            row(4, "assistant", Some("**Hi!** key sk-abcdefghijklmnopqrstu")),
            hidden, switch, steer, inactive, archived, summary, model_only, double,
            row(13, "user", None),
            row(14, "user", Some("You're online as Michael, the orchestrator of the hive.")),
            row(15, "user", Some("You have new hive inbox message(s). Read your inbox")),
        ];
        let v = parse_hermes_rows(&rows);
        let got: Vec<(&str, &str)> = v.iter().map(|x| (x.id.as_str(), x.role.as_str())).collect();
        assert_eq!(
            got,
            [("h1", "user"), ("h4", "assistant"), ("h7", "user"), ("h9", "assistant"), ("h15", "system")]
        );
        assert_eq!(v[0].text, "hello there");
        assert!(v[1].text.contains("**Hi!**") && v[1].text.contains("[hidden key]"));
        assert_eq!(v[4].kind.as_deref(), Some("team"));
        assert_eq!(v[4].text, "");
        assert_eq!(v[0].ts, 1_700_000_001_500);
    }

    #[test]
    fn hermes_json_content_decodes_parts() {
        let c = format!(
            "{HERMES_JSON_PREFIX}{}",
            r#"[{"type":"text","text":"look at this"},{"type":"image_url","image_url":{"url":"data:..."}},"tail"]"#
        );
        assert_eq!(decode_hermes_content(&c), "look at this\n[image]\ntail");
        assert_eq!(decode_hermes_content("plain"), "plain");
        // truncated JSON never leaks the raw sentinel text
        assert_eq!(decode_hermes_content(&format!("{HERMES_JSON_PREFIX}[{{\"ty")), "");
    }

    // ── Hermes: real SQLite (fixture DB we create ourselves, WAL like Hermes) ──
    fn make_db(dir: &Path) -> rusqlite::Connection {
        let c = rusqlite::Connection::open(dir.join("state.db")).unwrap();
        // Hermes' store is WAL (many readers + one writer); prove we read that shape.
        let mode: String = c.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0)).unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        c.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT NOT NULL, model_config TEXT, started_at REAL NOT NULL);
             CREATE TABLE messages (
               id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, role TEXT NOT NULL, content TEXT,
               tool_calls TEXT, timestamp REAL NOT NULL, active INTEGER NOT NULL DEFAULT 1,
               compacted INTEGER NOT NULL DEFAULT 0, display_kind TEXT, display_metadata TEXT,
               _compressed_summary INTEGER NOT NULL DEFAULT 0);
             INSERT INTO sessions VALUES ('s1','cli',NULL,1.0);
             INSERT INTO sessions VALUES ('sub','tool','{\"_delegate_from\":\"s1\"}',2.0);",
        )
        .unwrap();
        c
    }

    fn add(c: &rusqlite::Connection, sid: &str, role: &str, content: &str, ts: f64) {
        c.execute(
            "INSERT INTO messages (session_id, role, content, timestamp) VALUES (?1,?2,?3,?4)",
            rusqlite::params![sid, role, content, ts],
        )
        .unwrap();
    }

    #[test]
    fn hermes_db_incremental_read_while_a_writer_is_open() {
        let dir = tmp("hdb");
        let w = make_db(&dir); // stays open: WAL writer + our read-only reader
        let db = dir.join("state.db");
        assert_eq!(read_hermes_db(&dir.join("nope.db"), None).unwrap_err(), HermesErr::Missing);

        // empty DB → no messages, cursor 0
        let (v, cur, more, reset) = read_hermes_db(&db, None).unwrap();
        assert!((v.is_empty(), cur, more, reset) == (true, 0, false, false));

        add(&w, "s1", "user", "hi Michael", 10.0);
        add(&w, "s1", "assistant", "", 11.0); // tool-call-only
        add(&w, "s1", "tool", "{}", 12.0);
        add(&w, "sub", "assistant", "sub-agent chatter", 12.5); // tool/delegate session
        add(&w, "s1", "assistant", "Hello! How can I help?", 13.0);
        let (v, cur, _, _) = read_hermes_db(&db, None).unwrap();
        assert_eq!(v.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), ["hi Michael", "Hello! How can I help?"]);
        assert!(cur >= 5);

        // nothing new → empty, same cursor
        let (v2, cur2, _, _) = read_hermes_db(&db, Some(cur)).unwrap();
        assert!(v2.is_empty());
        assert_eq!(cur2, cur);

        // new rows appear incrementally
        add(&w, "s1", "user", "thanks", 20.0);
        let (v3, cur3, _, _) = read_hermes_db(&db, Some(cur)).unwrap();
        assert_eq!(v3.len(), 1);
        assert_eq!(v3[0].text, "thanks");
        assert!(cur3 > cur);

        // a cursor ahead of the table (db recreated) → reset + full reload
        let (v4, _, _, reset4) = read_hermes_db(&db, Some(9_999)).unwrap();
        assert!(reset4);
        assert_eq!(v4.len(), 3);
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hermes_db_first_load_is_the_newest_slice_and_batches_are_capped() {
        let dir = tmp("hbig");
        let w = make_db(&dir);
        w.execute_batch("BEGIN").unwrap();
        for i in 0..300 {
            add(&w, "s1", if i % 2 == 0 { "user" } else { "assistant" }, &format!("msg {i}"), i as f64);
        }
        w.execute_batch("COMMIT").unwrap();
        let (v, cur, _, _) = read_hermes_db(&dir.join("state.db"), None).unwrap();
        assert_eq!(v.len(), FIRST_LOAD_MESSAGES);
        assert_eq!(v.last().unwrap().text, "msg 299");
        assert_eq!(cur, 300);
        // a giant message is cut, not shipped whole
        add(&w, "s1", "assistant", &"z".repeat(200_000), 400.0);
        let (v, _, _, _) = read_hermes_db(&dir.join("state.db"), Some(cur)).unwrap();
        assert!(v[0].text.chars().count() <= MAX_MESSAGE_CHARS + 3);
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hermes_db_garbage_and_foreign_schema_are_unreadable_not_a_crash() {
        let dir = tmp("hbad");
        std::fs::write(dir.join("state.db"), b"this is not sqlite at all, just some text padding padding padding").unwrap();
        assert_eq!(read_hermes_db(&dir.join("state.db"), None).unwrap_err(), HermesErr::Unreadable);
        let dir2 = tmp("hschema");
        let c = rusqlite::Connection::open(dir2.join("state.db")).unwrap();
        c.execute_batch("CREATE TABLE messages (id INTEGER PRIMARY KEY, text TEXT);").unwrap();
        drop(c);
        assert_eq!(read_hermes_db(&dir2.join("state.db"), None).unwrap_err(), HermesErr::Unreadable);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn hermes_db_tolerates_an_older_schema_without_the_display_columns() {
        let dir = tmp("hold");
        let c = rusqlite::Connection::open(dir.join("state.db")).unwrap();
        c.execute_batch(
            "CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT, role TEXT, content TEXT, timestamp REAL);
             INSERT INTO messages (session_id, role, content, timestamp) VALUES ('a','user','old hello',1.0);",
        )
        .unwrap();
        let (v, _, _, _) = read_hermes_db(&dir.join("state.db"), None).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].text, "old hello");
        drop(c);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Claude: pure lines ──
    fn cl(ty: &str, content: &str, extra: &str) -> String {
        format!(
            r#"{{"type":"{ty}","uuid":"u-{}","timestamp":"2026-03-04T05:06:07.250Z","sessionId":"s","message":{{"role":"{ty}","content":{content}}}{extra}}}"#,
            fnv_hex(content)
        )
    }

    #[test]
    fn claude_lines_keep_text_and_skip_everything_else() {
        let lines = vec![
            cl("user", r#""hello there""#, ""),
            cl("assistant", r#"[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"Hi! \u001b[1mbold\u001b[0m"}]"#, ""),
            cl("assistant", r#"[{"type":"tool_use","id":"t","name":"Bash","input":{"command":"ls"}}]"#, ""),
            cl("user", r#"[{"type":"tool_result","tool_use_id":"t","content":"file.txt"}]"#, ""),
            cl("user", r#""<command-name>/model</command-name>""#, ""),
            cl("user", r#""<local-command-stdout>ok</local-command-stdout>""#, ""),
            cl("user", r#""[Request interrupted by user]""#, ""),
            cl("user", r#""meta caveat""#, r#","isMeta":true"#),
            cl("assistant", r#"[{"type":"text","text":"subagent says"}]"#, r#","isSidechain":true"#),
            cl("assistant", r#"[{"type":"text","text":"API Error: boom"}]"#, ""),
            r#"{"type":"summary","summary":"x"}"#.to_string(),
            r#"{"type":"system","content":"x"}"#.to_string(),
            "not json at all".to_string(),
            String::new(),
            cl("user", r#"[{"type":"text","text":"part one"},{"type":"text","text":"part two"}]"#, ""),
            cl("user", r#""You have new hive inbox message(s). Read""#, ""),
            cl("assistant", r#"[{"type":"text","text":"Done."}]"#, ""),
        ];
        // the synthetic API-error placeholder carries model "<synthetic>"
        let mut lines = lines;
        lines[9] = lines[9].replace(r#""role":"assistant""#, r#""role":"assistant","model":"<synthetic>""#);
        let v = parse_claude_lines(&lines);
        let got: Vec<(&str, String)> = v.iter().map(|x| (x.role.as_str(), x.text.clone())).collect();
        assert_eq!(
            got,
            [
                ("user", "hello there".to_string()),
                ("assistant", "Hi! bold".to_string()),
                ("user", "part one\n\npart two".to_string()),
                ("system", String::new()),
                ("assistant", "Done.".to_string()),
            ]
        );
        assert_eq!(v[0].ts, 1_772_600_767_250);
        assert!(v[0].id.starts_with("cu-"));
    }

    #[test]
    fn claude_user_content_with_no_uuid_gets_a_stable_id() {
        let l = r#"{"type":"user","message":{"role":"user","content":"x"}}"#.to_string();
        let a = parse_claude_lines(&[l.clone()]);
        let b = parse_claude_lines(&[l]);
        assert_eq!(a[0].id, b[0].id);
        assert_eq!(a[0].ts, 0);
    }

    // ── scan_lines ──
    #[test]
    fn scan_lines_does_not_consume_a_partial_trailing_line() {
        let mut r = IoCursor::new(b"one\ntwo\npart".to_vec());
        let (lines, used) = scan_lines(&mut r, 100, 1000, false).unwrap();
        assert_eq!(lines, ["one", "two"]);
        assert_eq!(used, 8);
        // once the newline lands the next read from `used` yields the line
        let mut r = IoCursor::new(b"part\n".to_vec());
        let (lines, used) = scan_lines(&mut r, 100, 1000, false).unwrap();
        assert_eq!((lines, used), (vec!["part".to_string()], 5));
    }

    #[test]
    fn scan_lines_skips_giant_lines_but_advances_past_them() {
        let mut data = b"a\n".to_vec();
        data.extend(std::iter::repeat_n(b'x', 5000));
        data.extend_from_slice(b"\nb\n");
        let mut r = BufReader::with_capacity(16, IoCursor::new(data.clone()));
        let (lines, used) = scan_lines(&mut r, 100, 1_000_000, false).unwrap();
        assert_eq!(lines, ["a", "b"]);
        assert_eq!(used, data.len() as u64);
        // an unterminated giant line is not consumed (still being written)
        let mut data2 = b"a\n".to_vec();
        data2.extend(std::iter::repeat_n(b'x', 5000));
        let mut r = BufReader::with_capacity(16, IoCursor::new(data2));
        let (lines, used) = scan_lines(&mut r, 100, 1_000_000, false).unwrap();
        assert_eq!((lines, used), (vec!["a".to_string()], 2));
    }

    #[test]
    fn scan_lines_respects_the_byte_budget_and_drop_first() {
        let mut r = IoCursor::new(b"aaaa\nbbbb\ncccc\n".to_vec());
        let (lines, used) = scan_lines(&mut r, 100, 6, false).unwrap();
        assert_eq!((lines, used), (vec!["aaaa".to_string(), "bbbb".to_string()], 10));
        let mut r = IoCursor::new(b"tail-of-cut\nreal\n".to_vec());
        let (lines, used) = scan_lines(&mut r, 100, 1000, true).unwrap();
        assert_eq!((lines, used), (vec!["real".to_string()], 17));
        // invalid UTF-8 is replaced, never an error
        let mut r = IoCursor::new(vec![b'o', 0xff, b'k', b'\n']);
        assert_eq!(scan_lines(&mut r, 100, 1000, false).unwrap().0, ["o\u{fffd}k"]);
    }

    // ── Claude: files ──
    #[test]
    fn claude_file_incremental_with_a_half_written_line() {
        let root = tmp("croot");
        let proj = root.join(crate::usage::project_key("C:\\work\\app"));
        std::fs::create_dir_all(&proj).unwrap();
        let path = proj.join("sess-1.jsonl");
        let l1 = cl("user", r#""first""#, "");
        let l2 = cl("assistant", r#"[{"type":"text","text":"second"}]"#, "");
        // line 2 is cut mid-write
        let half = &l2[..l2.len() / 2];
        std::fs::write(&path, format!("{l1}\n{half}")).unwrap();

        let found = find_claude_transcript(&root, "C:\\work\\app", "sess-1").unwrap();
        assert_eq!(found, path);
        // found under another cwd too (worktree)
        assert!(find_claude_transcript(&root, "D:\\elsewhere", "sess-1").is_some());
        assert!(find_claude_transcript(&root, "x", "../evil").is_none());
        assert!(find_claude_transcript(&root, "x", "").is_none());

        let r1 = read_claude_file(&path, "sess-1", None).unwrap();
        assert_eq!(r1.source, "claude");
        assert_eq!(r1.messages.len(), 1);
        assert_eq!(r1.messages[0].text, "first");
        let c1 = parse_cursor(&r1.cursor).unwrap();
        assert_eq!(c1, Cursor::Claude { session: "sess-1".into(), offset: l1.len() as u64 + 1 });

        // the writer finishes the line
        std::fs::write(&path, format!("{l1}\n{l2}\n")).unwrap();
        let r2 = read_claude_file(&path, "sess-1", Some(&c1)).unwrap();
        assert_eq!(r2.messages.len(), 1);
        assert_eq!(r2.messages[0].text, "second");
        assert!(!r2.reset);

        // nothing new
        let c2 = parse_cursor(&r2.cursor).unwrap();
        let r3 = read_claude_file(&path, "sess-1", Some(&c2)).unwrap();
        assert!(r3.messages.is_empty());
        assert_eq!(r3.cursor, r2.cursor);

        // truncated/rotated file → reset
        std::fs::write(&path, format!("{l1}\n")).unwrap();
        let r4 = read_claude_file(&path, "sess-1", Some(&c2)).unwrap();
        assert!(r4.reset);
        assert_eq!(r4.messages.len(), 1);

        // a cursor from a different session → reset
        let foreign = Cursor::Claude { session: "other".into(), offset: 3 };
        assert!(read_claude_file(&path, "sess-1", Some(&foreign)).unwrap().reset);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn claude_first_load_of_a_huge_transcript_starts_near_the_tail() {
        let root = tmp("chuge");
        let path = root.join("big.jsonl");
        let filler = cl("assistant", &format!(r#"[{{"type":"text","text":"{}"}}]"#, "f".repeat(900)), "");
        let mut body = String::new();
        let n = (FIRST_LOAD_TAIL_BYTES as usize / filler.len()) + 50;
        for _ in 0..n {
            // distinct ts so dedupe does not merge them
            body.push_str(&filler.replace("05:06:07", &format!("05:{:02}:{:02}", body.len() % 59, body.len() % 53)));
            body.push('\n');
        }
        body.push_str(&cl("user", r#""the very last""#, ""));
        body.push('\n');
        std::fs::write(&path, &body).unwrap();
        let r = read_claude_file(&path, "big", None).unwrap();
        assert!(r.messages.len() <= FIRST_LOAD_MESSAGES);
        assert_eq!(r.messages.last().unwrap().text, "the very last");
        assert_eq!(parse_cursor(&r.cursor), Some(Cursor::Claude { session: "big".into(), offset: body.len() as u64 }));
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── the core ──
    #[test]
    fn core_routes_per_engine_and_falls_back_cleanly() {
        let agent_dir = tmp("core");
        let mk = |provider: &'static str, root: Option<&'static Path>, sid: Option<&'static str>| ReadParams {
            provider,
            agent_dir: Box::leak(agent_dir.clone().into_boxed_path()),
            cwd: "C:\\w",
            session_id: sid,
            claude_projects_root: root,
        };
        // unsupported engine
        let r = read_conversation(&mk("codex", None, None), None);
        assert_eq!((r.source, r.reason), ("none", Some("unsupported_engine")));
        // hermes, db not there yet
        let r = read_conversation(&mk("hermes", None, None), None);
        assert_eq!((r.source, r.reason), ("none", Some("starting")));
        // hermes with a db
        let home = agent_dir.join(".hermes");
        std::fs::create_dir_all(&home).unwrap();
        let w = make_db(&home);
        add(&w, "s1", "user", "hey", 1.0);
        let r = read_conversation(&mk("hermes", None, None), None);
        assert_eq!((r.source, r.messages.len(), r.reason), ("hermes", 1, None));
        // a claude cursor handed to hermes (engine switched) → reset, full reload
        let r2 = read_conversation(&mk("hermes", None, None), Some("c:abc:77"));
        assert!(r2.reset);
        assert_eq!(r2.messages.len(), 1);
        // an unreadable db (close the writer cleanly FIRST: its checkpoint must not repair the garbage)
        drop(w);
        let _ = std::fs::remove_file(home.join("state.db-wal"));
        let _ = std::fs::remove_file(home.join("state.db-shm"));
        std::fs::write(home.join("state.db"), b"garbage garbage garbage garbage garbage garbage garbage garbage").unwrap();
        let r = read_conversation(&mk("hermes", None, None), None);
        assert_eq!((r.source, r.reason), ("none", Some("unreadable")));
        // claude without a session / without a transcript
        let r = read_conversation(&mk("claude", None, None), None);
        assert_eq!((r.source, r.reason), ("none", Some("starting")));
        let root: &'static Path = Box::leak(tmp("core-root").into_boxed_path());
        let r = read_conversation(&mk("claude", Some(root), Some("nope")), None);
        assert_eq!((r.source, r.reason), ("none", Some("starting")));
        let _ = std::fs::remove_dir_all(&agent_dir);
    }

    #[test]
    fn wire_shape_is_camel_case_with_optional_fields_omitted() {
        let r = ConvResult {
            messages: vec![m("h1", "user", "hi", 5)],
            cursor: "h:1".into(),
            source: "hermes",
            reason: None,
            reset: false,
            more: false,
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["source"], "hermes");
        assert!(v.get("reason").is_none());
        assert!(v["messages"][0].get("kind").is_none());
        assert_eq!(v["messages"][0]["role"], "user");
        let n = serde_json::to_value(ConvResult::none("starting")).unwrap();
        assert_eq!(n["reason"], "starting");
        assert_eq!(n["cursor"], "");
    }
}
