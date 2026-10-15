//! Transcript usage reader — port of `src/main/transcript.ts` +
//! `src/main/pricing.ts`.
//!
//! The OFFLINE reconciler / fallback upstream's OTel collector covers live:
//! sum real token usage across the agent's Claude Code transcripts
//! (`~/.claude/projects/<cwd-key>/*.jsonl`), priced per-record by ITS OWN
//! model (cost bug #1), optionally filtered to one sessionId (bug #2 — two
//! agents sharing a cwd must each count only their own session).
//!
//! Per-file incremental caching is preserved: transcripts are append-only,
//! so a parsed byte range never changes — a steady-state beat is a readdir +
//! one stat per file. A torn trailing line is left unparsed until the writer
//! completes it.

use crate::lock_ext::LockExt;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::breaker::UsageSample;

// ─── pricing.rs (crate::pricing — the ONE place per-model pricing lives) ────

/// Strip Claude Code's variant suffix: `claude-opus-4-8[1m]` → `claude-opus-4-8`.
/// pub(crate): the telemetry collector normalizes model ids the same way.
pub(crate) fn normalize_model(model: &str) -> String {
    crate::pricing::normalize_model(model).to_string()
}

// ─── transcript.ts ──────────────────────────────────────────────────────────

/// Claude Code's project key: EVERY non-alphanumeric → dash
/// (C:\Users\me\app → C--Users-me-app). The legacy POSIX spelling is skipped:
/// it only matters for pre-2026 POSIX installs, and this is a Windows-first
/// port (deviation noted in PORTING.md).
fn project_key(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn project_dir(cwd: &str) -> Option<PathBuf> {
    let home =
        std::env::home_dir().or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))?;
    let dir = home.join(".claude").join("projects").join(project_key(cwd));
    dir.is_dir().then_some(dir)
}

/// `~/.claude/projects` — the transcript root (dirs::home_dir, matching the
/// user-profile lookup project_dir uses).
fn claude_projects_root() -> Option<PathBuf> {
    let home =
        std::env::home_dir().or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))?;
    Some(home.join(".claude").join("projects"))
}

/// Upstream `seedSessionTranscript` (transcript.ts): ensure
/// `<sessionId>.jsonl` exists under `~/.claude/projects/<projectKey(cwd)>` so
/// `claude --resume` run IN that cwd can find it — a session started in a
/// different cwd (restored team, worktree path) is copied across. A session
/// id is a filename segment, so it is validated against `[A-Za-z0-9_-]` like
/// upstream before it ever touches a path.
/// Best-effort: false on any miss/error → the caller falls back to a fresh
/// session (and flags resumeNotFound for a typed id).
pub fn seed_session_transcript(cwd: &str, session_id: &str) -> bool {
    match claude_projects_root() {
        Some(root) => seed_session_transcript_in(&root, cwd, session_id),
        None => false,
    }
}

/// The testable core of seed_session_transcript — `root` is injectable so a
/// unit test never touches the real ~/.claude tree.
fn seed_session_transcript_in(projects_root: &std::path::Path, cwd: &str, session_id: &str) -> bool {
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return false;
    }
    let target = projects_root
        .join(project_key(cwd))
        .join(format!("{session_id}.jsonl"));
    if target.is_file() {
        return true;
    }
    let Ok(rd) = fs::read_dir(projects_root) else {
        return false;
    };
    for entry in rd.flatten() {
        let candidate = entry.path().join(format!("{session_id}.jsonl"));
        if candidate.is_file() {
            if let Some(parent) = target.parent() {
                if fs::create_dir_all(parent).is_err() {
                    return false;
                }
            }
            return fs::copy(&candidate, &target).is_ok();
        }
    }
    false
}

// ─── Grok usage file (upstream telemetry.ts grokFallback, #536) ─────────────

/// JS `encodeURIComponent`: everything but `A-Za-z0-9 - _ . ! ~ * ' ( )` is
/// percent-encoded per UTF-8 byte (the Grok CLI keys its session dir by it).
fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// One Grok session's cumulative snapshot plus the model it reports.
pub struct GrokUsage {
    pub sample: UsageSample,
    pub model: String,
}

/// `~/.grok/sessions/<encodeURIComponent(cwd)>/<sessionId>/usage.json`, read
/// from the user's home.
pub fn grok_usage(cwd: &str, session_id: &str) -> Option<GrokUsage> {
    let home =
        std::env::home_dir().or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))?;
    grok_usage_in(&home.join(".grok").join("sessions"), cwd, session_id)
}

/// The root-injectable core of `grok_usage` (tests use a temp dir). `None` for
/// a missing/mid-write file or a session id that is not a plain path segment.
/// Cost is Grok's own `costUsdTicks` (10^10 ticks to the dollar) — taken as
/// given, no price table to maintain.
fn grok_usage_in(sessions_root: &Path, cwd: &str, session_id: &str) -> Option<GrokUsage> {
    if cwd.is_empty()
        || session_id.is_empty()
        || !session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    let file = sessions_root
        .join(encode_uri_component(cwd))
        .join(session_id)
        .join("usage.json");
    let parsed: serde_json::Value = serde_json::from_str(&fs::read_to_string(file).ok()?).ok()?;
    let t = parsed.get("session")?;
    let num = |k: &str| {
        t.get(k)
            .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
            .filter(|n| n.is_finite() && *n >= 0.0)
            .unwrap_or(0.0)
    };
    let ts = parsed
        .get("updatedAt")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.timestamp_millis() as u64)
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        });
    Some(GrokUsage {
        sample: UsageSample {
            input: num("inputTokens") as u64,
            output: num("outputTokens") as u64,
            cache_read: num("cachedReadTokens") as u64,
            cache_creation: num("cacheCreationTokens") as u64,
            usd: num("costUsdTicks") / 1e10,
            ts,
        },
        model: normalize_model(t.get("primaryModelId").and_then(|v| v.as_str()).unwrap_or("")),
    })
}

/// Upstream CumulativeSampleGate: the ledger is fed a running TOTAL, so a
/// cumulative file snapshot is admitted only when its numbers MOVED (an idle
/// Grok agent re-reads the same file every beat and stays silent). `ts` is not
/// part of the signature. A new session id always differs, so a respawned
/// agent's first sample is admitted without an explicit forget.
#[derive(Default)]
pub struct CumulativeSampleGate {
    last: Mutex<HashMap<String, String>>,
}

impl CumulativeSampleGate {
    pub fn admits(&self, agent_id: &str, session_id: &str, g: &GrokUsage) -> bool {
        let s = &g.sample;
        let sig = format!(
            "{session_id}|{}|{}|{}|{}|{}|{}",
            s.input, s.output, s.cache_read, s.cache_creation, g.model, s.usd
        );
        let mut last = self.last.lock_safe();
        if last.get(agent_id) == Some(&sig) {
            return false;
        }
        last.insert(agent_id.to_string(), sig);
        true
    }
}

#[derive(Clone, Default)]
struct Totals {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    usd: f64,
    model: Option<String>,
    last_ts: u64,
}

/// Per-file incremental parse state — one entry per PHYSICAL file, shared
/// across every sessionId query (upstream's fix for re-parse-per-agent).
struct FileEntry {
    size: u64,
    mtime_ms: u64,
    offset: u64,
    all: Totals,
    per_session: HashMap<String, Totals>,
}

const CACHE_MAX: usize = 1024;

#[derive(Default)]
pub struct UsageReader {
    cache: Mutex<HashMap<PathBuf, FileEntry>>,
}

impl UsageReader {
    /// Cumulative usage for an agent working in `cwd`, filtered to its own
    /// `session_id` (registry.session_id — avoids the co-located-agents
    /// double-count). With NO known session id this reports no data instead of
    /// summing every transcript in the (routinely shared) cwd — upstream D11:
    /// someone else's, or a past run's, history must never be attributed to
    /// this agent. Returns None when nothing readable.
    pub fn sample(&self, cwd: &str, session_id: Option<&str>) -> Option<UsageSample> {
        let sid = session_id.filter(|s| !s.is_empty())?;
        // Grok before the transcript read (upstream grokFallback): a Grok
        // agent has no Claude transcript, and its own file carries a real cost.
        // A Claude session id is not a dir under ~/.grok/sessions → None.
        if let Some(g) = grok_usage(cwd, sid) {
            return Some(g.sample);
        }
        self.sample_in(&project_dir(cwd)?, Some(sid))
    }

    /// The directory-injectable core of `sample` (tests point it at a temp dir).
    fn sample_in(&self, dir: &Path, session_id: Option<&str>) -> Option<UsageSample> {
        let rd = fs::read_dir(dir).ok()?;
        let mut out = UsageSample::default();
        let mut seen = false;
        let mut files: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .collect();
        files.sort();
        for file in files {
            let Some(entry) = self.read_file_usage(&file) else {
                continue;
            };
            let totals = match session_id {
                Some(sid) => entry.per_session.get(sid),
                None => Some(&entry.all),
            };
            let Some(t) = totals else { continue };
            if t.input + t.output + t.cache_read + t.cache_write == 0 {
                continue;
            }
            seen = true;
            out.input += t.input;
            out.output += t.output;
            out.cache_read += t.cache_read;
            out.cache_creation += t.cache_write;
            out.usd += t.usd;
            out.ts = out.ts.max(t.last_ts);
        }
        seen.then_some(out)
    }

    /// Read (incrementally) one transcript file's totals.
    fn read_file_usage(&self, path: &Path) -> Option<FileEntry> {
        let meta = fs::metadata(path).ok()?;
        let size = meta.len();
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let mut cache = self.cache.lock_safe();
        if let Some(c) = cache.get(path) {
            if c.size == size && c.mtime_ms == mtime {
                // Cloning the totals out under the lock keeps the borrow simple;
                // entries are small (a handful of counters per session bucket).
                return Some(FileEntry {
                    size: c.size,
                    mtime_ms: c.mtime_ms,
                    offset: c.offset,
                    all: c.all.clone(),
                    per_session: c.per_session.clone(),
                });
            }
        }

        let mut entry = match cache.get(path) {
            Some(c) if size >= c.offset => FileEntry {
                size,
                mtime_ms: mtime,
                offset: c.offset,
                all: c.all.clone(),
                per_session: c.per_session.clone(),
            },
            _ => FileEntry {
                size,
                mtime_ms: mtime,
                offset: 0,
                all: Totals::default(),
                per_session: HashMap::new(),
            },
        };

        if let Ok(mut f) = fs::File::open(path) {
            let len = size.saturating_sub(entry.offset) as usize;
            if len > 0 {
                let mut buf = vec![0u8; len];
                if f.seek(SeekFrom::Start(entry.offset)).is_ok() && f.read_exact(&mut buf).is_ok() {
                    // Consume only up to the last complete line; a torn tail
                    // is re-read once the writer finishes it.
                    let text = String::from_utf8_lossy(&buf);
                    if let Some(last_nl) = text.rfind('\n') {
                        let complete = &text[..=last_nl];
                        parse_usage_lines(complete, &mut entry);
                        entry.offset += complete.len() as u64;
                    }
                }
            }
        }

        if cache.len() > CACHE_MAX {
            // Drop the oldest half — usageCache eviction parity.
            let keys: Vec<PathBuf> = cache
                .keys()
                .take(cache.len() - CACHE_MAX / 2)
                .cloned()
                .collect();
            for k in keys {
                cache.remove(&k);
            }
        }
        let out = FileEntry {
            size: entry.size,
            mtime_ms: entry.mtime_ms,
            offset: entry.offset,
            all: entry.all.clone(),
            per_session: entry.per_session.clone(),
        };
        cache.insert(path.to_path_buf(), entry);
        Some(out)
    }
}

/// Port of parseUsageLines: assistant records only, priced per record's own
/// model, bucketed into `all` + that record's `sessionId`.
fn parse_usage_lines(text: &str, entry: &mut FileEntry) {
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<serde_json::Value>(t) else {
            continue;
        };
        if rec.get("type").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let msg = rec.get("message");
        let Some(u) = msg.and_then(|m| m.get("usage")) else {
            continue;
        };
        let num = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let (r_in, r_out) = (num("input_tokens"), num("output_tokens"));
        let (r_cw, r_cr) = (
            num("cache_creation_input_tokens"),
            num("cache_read_input_tokens"),
        );
        let model = msg
            .and_then(|m| m.get("model"))
            .and_then(|v| v.as_str())
            .map(normalize_model);
        let p = crate::pricing::price_for(model.as_deref().unwrap_or(""));
        let cost = (r_in as f64 / 1e6) * p.input_per_m
            + (r_out as f64 / 1e6) * p.output_per_m
            + (r_cr as f64 / 1e6) * p.cache_read_per_m
            + (r_cw as f64 / 1e6) * p.cache_write_per_m;
        // Record timestamp (ISO) → ms, for the velocity diff's clock.
        let ts = rec
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.timestamp_millis() as u64)
            .unwrap_or(0);

        let apply = |acc: &mut Totals| {
            if let Some(m) = &model {
                acc.model = Some(m.clone());
            }
            acc.input += r_in;
            acc.output += r_out;
            acc.cache_write += r_cw;
            acc.cache_read += r_cr;
            acc.usd += cost;
            acc.last_ts = acc.last_ts.max(ts);
        };
        apply(&mut entry.all);
        if let Some(sid) = rec.get("sessionId").and_then(|v| v.as_str()) {
            if !sid.is_empty() {
                apply(entry.per_session.entry(sid.to_string()).or_default());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_key_dashes_every_non_alnum() {
        assert_eq!(project_key("C:\\Users\\me\\app"), "C--Users-me-app");
        assert_eq!(project_key("/Users/me/MDv0.3.0"), "-Users-me-MDv0-3-0");
    }

    #[test]
    fn grok_usage_reads_ticks_and_gate_admits_only_movement() {
        assert_eq!(encode_uri_component(r"D:\a b\é"), "D%3A%5Ca%20b%5C%C3%A9");
        let root = std::env::temp_dir().join(format!("oa-grok-{}", uuid::Uuid::new_v4()));
        let cwd = r"D:\work\proj";
        let dir = root.join(encode_uri_component(cwd)).join("sess-1");
        fs::create_dir_all(&dir).unwrap();
        let write = |ticks: u64, out: u64| {
            fs::write(
                dir.join("usage.json"),
                format!(
                    r#"{{"updatedAt":"2026-01-01T00:00:00Z","session":{{"inputTokens":10,"outputTokens":{out},"cachedReadTokens":3,"cacheCreationTokens":1,"primaryModelId":"grok-4","costUsdTicks":{ticks}}}}}"#
                ),
            )
            .unwrap();
        };
        write(25_000_000_000, 7);
        let g = grok_usage_in(&root, cwd, "sess-1").unwrap();
        assert!((g.sample.usd - 2.5).abs() < 1e-9);
        assert_eq!((g.sample.input, g.sample.output, g.sample.cache_read), (10, 7, 3));
        assert_eq!(g.model, "grok-4");
        assert!(g.sample.ts > 0);
        // Missing session, traversal-ish ids, wrong cwd -> None.
        assert!(grok_usage_in(&root, cwd, "other").is_none());
        assert!(grok_usage_in(&root, cwd, "..").is_none());
        assert!(grok_usage_in(&root, r"D:\elsewhere", "sess-1").is_none());

        let gate = CumulativeSampleGate::default();
        assert!(gate.admits("a1", "sess-1", &g));
        assert!(!gate.admits("a1", "sess-1", &g), "unchanged -> silent");
        write(26_000_000_000, 8);
        let g2 = grok_usage_in(&root, cwd, "sess-1").unwrap();
        assert!(gate.admits("a1", "sess-1", &g2), "moved -> admitted");
        assert!(gate.admits("a2", "sess-1", &g2), "per agent");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sample_is_scoped_to_the_agents_own_session() {
        let dir = std::env::temp_dir().join(format!("oa-usage-scope-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let rec = |sid: &str, out: u64| {
            format!(
                "{{\"type\":\"assistant\",\"sessionId\":\"{sid}\",\"message\":{{\"model\":\"claude-sonnet-5\",\"usage\":{{\"input_tokens\":10,\"output_tokens\":{out}}}}}}}\n"
            )
        };
        fs::write(dir.join("mine.jsonl"), rec("mine", 5)).unwrap();
        fs::write(dir.join("foreign.jsonl"), rec("foreign", 1_000_000)).unwrap();
        let r = UsageReader::default();
        let s = r.sample_in(&dir, Some("mine")).unwrap();
        assert_eq!((s.input, s.output), (10, 5));
        assert!(r.sample_in(&dir, Some("nobody")).is_none());
        // No known session id -> no data, never the whole directory's history.
        assert!(r.sample("D:\\no\\such\\cwd", None).is_none());
        assert!(r.sample("D:\\no\\such\\cwd", Some("")).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_sums_per_session_and_prices_per_model() {
        let text = concat!(
            r#"{"type":"user","sessionId":"s1"}"#,
            "\n",
            r#"{"type":"assistant","sessionId":"s1","timestamp":"2026-01-01T00:00:00Z","message":{"model":"claude-opus-4-8[1m]","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":1000}}}"#,
            "\n",
            r#"{"type":"assistant","sessionId":"s2","message":{"model":"claude-haiku-4-5","usage":{"input_tokens":200,"output_tokens":100}}}"#,
            "\n",
            // torn / malformed lines and non-assistant records are skipped
            r#"{"type":"assistant""#,
            "\n",
        );
        let mut e = FileEntry {
            size: 0,
            mtime_ms: 0,
            offset: 0,
            all: Totals::default(),
            per_session: HashMap::new(),
        };
        parse_usage_lines(text, &mut e);
        assert_eq!(e.all.input, 300);
        assert_eq!(e.all.output, 150);
        assert_eq!(e.all.cache_read, 1000);
        assert_eq!(e.all.cache_write, 10);
        assert_eq!(e.per_session["s1"].input, 100);
        assert_eq!(e.per_session["s2"].input, 200);
        // opus pricing: 100in*15/M + 50out*75/M + 1000cr*1.5/M + 10cw*18.75/M
        let s1 = &e.per_session["s1"];
        let expected =
            100.0 / 1e6 * 15.0 + 50.0 / 1e6 * 75.0 + 1000.0 / 1e6 * 1.5 + 10.0 / 1e6 * 18.75;
        assert!((s1.usd - expected).abs() < 1e-9);
        assert_eq!(
            e.per_session["s1"].model.as_deref(),
            Some("claude-opus-4-8")
        );
    }
}
