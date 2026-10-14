//! Lifetime cost — port of `costLifetime.ts`.
//!
//! The telemetry `usd` counter is cumulative-SINCE-PROCESS-START, not
//! lifetime: an app restart rebuilds it empty under the SAME session_id (the
//! agent resumes; only our accumulator forgot). Upstream measured the
//! last-value read hiding 59% of real spend on a live ledger.
//!
//! Recovery: `cost-ledger.jsonl` keeps every line. Within one
//! (agent,session) the counter only climbs, so a DECREASE is the reset
//! signature and nothing else. Lifetime per agent:
//!
//!   `sum(peak of each closed segment) + peak of the open segment`
//!
//! Incremental fold: each pass reads only bytes appended since the last one;
//! a truncated/rotated/replaced file resets the fold. Read-only — never writes.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Per (agent,session) fold state: closed segments plus the open one.
#[derive(Default, Clone, Copy)]
struct Segment {
    committed: f64,
    peak: f64,
}

/// Float-noise guard — real resets drop by cents, anything below is dust.
const EPS: f64 = 1e-9;

/// Incremental reader for an append-only JSONL ledger: each `advance` returns
/// only the COMPLETE lines appended since the last call. A shrunk file, or one
/// whose first bytes changed (deleted and recreated, rotated, rewritten),
/// restarts from byte 0 and reports `reset` so the caller drops its fold.
#[derive(Default)]
pub(crate) struct LedgerCursor {
    /// Bytes of the ledger already consumed.
    offset: u64,
    /// First (up to 64) bytes of the file as last seen — an append never
    /// changes them, so a mismatch means the file was replaced.
    head: Vec<u8>,
    /// Trailing partial line, held as bytes so a multi-byte char split across
    /// a read boundary is never decoded in half.
    tail: Vec<u8>,
}

pub(crate) struct Advance {
    pub reset: bool,
    /// Newly completed lines, separated by a line feed.
    pub lines: String,
}

impl LedgerCursor {
    pub(crate) fn advance(&mut self, path: &Path) -> std::io::Result<Advance> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = fs::File::open(path)?;
        let size = f.metadata()?.len();
        let mut stale = size < self.offset;
        if !stale && self.offset > 0 && !self.head.is_empty() {
            let mut h = vec![0u8; self.head.len()];
            stale = f.read_exact(&mut h).is_err() || h != self.head;
        }
        if stale {
            *self = Self::default();
        }
        f.seek(SeekFrom::Start(self.offset))?;
        let mut buf = Vec::new();
        let n = f.read_to_end(&mut buf)? as u64;
        if self.head.len() < 64 {
            let want = 64 - self.head.len();
            self.head.extend_from_slice(&buf[..want.min(buf.len())]);
        }
        self.offset += n;
        let mut all = std::mem::take(&mut self.tail);
        all.extend_from_slice(&buf);
        let lines = match all.iter().rposition(|b| *b == b'\n') {
            Some(cut) => {
                self.tail = all[cut + 1..].to_vec();
                String::from_utf8_lossy(&all[..cut]).into_owned()
            }
            None => {
                self.tail = all;
                String::new()
            }
        };
        Ok(Advance { reset: stale, lines })
    }
}

#[derive(Default)]
pub struct CostLedgerTotals {
    cur: LedgerCursor,
    /// "agentId\tsessionId" → fold state.
    seg: HashMap<String, Segment>,
    /// agentId → lifetime usd, recomputed after each pass.
    totals: HashMap<String, f64>,
    /// True once a full pass reached EOF — separates "no spend" from
    /// "not read yet" so a cold zero is never published as fact.
    warm: bool,
}

impl CostLedgerTotals {
    /// Lifetime usd for one agent — None until the first complete pass.
    pub fn usd_for(&self, agent_id: &str) -> Option<f64> {
        if !self.warm {
            return None;
        }
        Some(*self.totals.get(agent_id).unwrap_or(&0.0))
    }

    pub fn all(&self) -> HashMap<String, f64> {
        self.totals.clone()
    }

    pub fn ready(&self) -> bool {
        self.warm
    }

    /// True lifetime spend across every agent in the ledger.
    pub fn floor_total(&self) -> f64 {
        self.totals.values().sum()
    }

    /// Fold whatever was appended since the last pass. Safe from a timer,
    /// never throws — an unreadable ledger leaves the last good totals.
    pub fn refresh(&mut self, ledger_path: &Path) {
        let _ = self.fold(ledger_path);
    }

    /// Fold until caught up to EOF — for callers that want the number now.
    /// `warm` latches once the first full pass lands, so the first refresh
    /// must happen unconditionally.
    pub fn refresh_fully(&mut self, ledger_path: &Path) {
        for _ in 0..64 {
            self.refresh(ledger_path);
            if self.warm {
                return;
            }
        }
    }

    fn fold(&mut self, path: &Path) -> std::io::Result<()> {
        let adv = self.cur.advance(path)?;
        // Truncated, rotated or replaced underneath us: every held segment is
        // suspect.
        if adv.reset {
            self.seg.clear();
            self.totals.clear();
        }
        for line in adv.lines.split('\n') {
            if !line.is_empty() {
                self.fold_line(line);
            }
        }
        if adv.reset || !adv.lines.is_empty() {
            self.recompute();
        }
        self.warm = true;
        Ok(())
    }

    fn fold_line(&mut self, line: &str) {
        let Ok(row) = serde_json::from_str::<serde_json::Value>(line) else {
            return; // half-written tail line
        };
        let Some(agent) = row.get("agent_id").and_then(|v| v.as_str()) else {
            return;
        };
        let session = row.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
        let usd = row.get("usd").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if !usd.is_finite() {
            return;
        }
        let key = format!("{agent}\t{session}");
        let s = self.seg.entry(key).or_default();
        if usd < s.peak - EPS {
            // Counter went backwards: the previous segment ended at its peak.
            s.committed += s.peak;
            s.peak = usd;
        } else if usd > s.peak {
            s.peak = usd;
        }
    }

    fn recompute(&mut self) {
        let mut next: HashMap<String, f64> = HashMap::new();
        for (key, s) in &self.seg {
            let agent = key.split('\t').next().unwrap_or("");
            *next.entry(agent.to_string()).or_insert(0.0) += s.committed + s.peak;
        }
        self.totals = next;
    }
}

/// One-shot fold of a ledger already in memory (tests + simple callers).
pub fn lifetime_usd_from_ledger(text: &str) -> HashMap<String, f64> {
    let mut t = CostLedgerTotals::default();
    let s = if text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    };
    for line in s.split('\n').filter(|l| !l.is_empty()) {
        t.fold_line(line);
    }
    t.recompute();
    t.all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(agent: &str, session: &str, usd: f64) -> String {
        format!(
            r#"{{"agent_id":"{agent}","session_id":"{session}","ts":0,"input":0,"output":0,"cache_read":0,"cache_creation":0,"model":"m","usd":{usd}}}"#
        )
    }

    #[test]
    fn lifetime_sums_peaks_across_resets() {
        // Two sessions; one resets its counter mid-stream (the restart
        // signature). Lifetime = closed peak + open peak per session.
        let text = [
            line("w1", "s1", 1.0),
            line("w1", "s1", 3.0),
            line("w1", "s1", 0.2), // ← counter went backwards = app restart
            line("w1", "s1", 0.9),
            line("w2", "s9", 5.0),
        ]
        .join("\n")
            + "\n";
        let t = lifetime_usd_from_ledger(&text);
        assert!((t["w1"] - 3.9).abs() < 1e-9); // 3.0 peak + 0.9 open peak
        assert!((t["w2"] - 5.0).abs() < 1e-9);
    }

    #[test]
    fn incremental_fold_tracks_appends_and_rotations() {
        let dir = std::env::temp_dir().join(format!("oa-clt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("cost-ledger.jsonl");
        std::fs::write(&p, line("w1", "s1", 2.0) + "\n").unwrap();
        let mut t = CostLedgerTotals::default();
        t.refresh_fully(&p);
        assert!((t.usd_for("w1").unwrap() - 2.0).abs() < 1e-9);
        // An append folds only the new bytes; a counter decrease closes the
        // segment.
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        writeln!(f, "{}", line("w1", "s1", 0.5)).unwrap();
        t.refresh_fully(&p);
        assert!((t.usd_for("w1").unwrap() - 2.5).abs() < 1e-9);
        // Rotation under our offset → fold restarts clean.
        std::fs::write(&p, line("w1", "s2", 7.0) + "\n").unwrap();
        t.refresh_fully(&p);
        assert!((t.usd_for("w1").unwrap() - 7.0).abs() < 1e-9);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_lines_drop_not_throw() {
        let t = lifetime_usd_from_ledger(
            "not json\n{\"agent_id\":\"w\",\"session_id\":\"s\",\"usd\":1.5}\n",
        );
        assert!((t["w"] - 1.5).abs() < 1e-9);
        assert!(lifetime_usd_from_ledger("").is_empty());
    }
}
