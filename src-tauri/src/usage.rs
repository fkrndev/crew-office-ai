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
    let home = std::env::home_dir()
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))?;
    let dir = home.join(".claude").join("projects").join(project_key(cwd));
    dir.is_dir().then_some(dir)
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
    /// Cumulative usage for an agent working in `cwd`, filtered to
    /// `session_id` when known (registry.session_id — avoids the
    /// co-located-agents double-count). Returns None when nothing readable.
    pub fn sample(&self, cwd: &str, session_id: Option<&str>) -> Option<UsageSample> {
        let dir = project_dir(cwd)?;
        let rd = fs::read_dir(&dir).ok()?;
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

        let mut cache = self.cache.lock().unwrap();
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
                if f.seek(SeekFrom::Start(entry.offset)).is_ok()
                    && f.read_exact(&mut buf).is_ok()
                {
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
            let keys: Vec<PathBuf> = cache.keys().take(cache.len() - CACHE_MAX / 2).cloned().collect();
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
    fn parse_sums_per_session_and_prices_per_model() {
        let text = concat!(
            r#"{"type":"user","sessionId":"s1"}"#, "\n",
            r#"{"type":"assistant","sessionId":"s1","timestamp":"2026-01-01T00:00:00Z","message":{"model":"claude-opus-4-8[1m]","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":1000}}}"#, "\n",
            r#"{"type":"assistant","sessionId":"s2","message":{"model":"claude-haiku-4-5","usage":{"input_tokens":200,"output_tokens":100}}}"#, "\n",
            // torn / malformed lines and non-assistant records are skipped
            r#"{"type":"assistant""#, "\n",
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
        let expected = 100.0 / 1e6 * 15.0 + 50.0 / 1e6 * 75.0 + 1000.0 / 1e6 * 1.5 + 10.0 / 1e6 * 18.75;
        assert!((s1.usd - expected).abs() < 1e-9);
        assert_eq!(e.per_session["s1"].model.as_deref(), Some("claude-opus-4-8"));
    }
}
