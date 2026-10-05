//! MemoryReflector — the missing CONDENSE half of the janitor (reflect.ts).
//!
//! The janitor flags an oversized `agents/<id>/memory.md` but never shrinks
//! it. This module finishes the job: memory files that cross a byte/section
//! threshold are rebuilt into the canonical 3-region shape — pinned durable
//! facts, a single bounded condensed summary, and the newest K verbatim
//! sections — via a headless `claude -p` call.
//!
//! Safety pipeline (verbatim order): BACKUP (lossless cold copy) → SUMMARIZE
//! → REBUILD → VERIFY-DON'T-TRUST (every check must hold) → ATOMIC SWAP.
//! A failed verify leaves the original untouched.

use std::fs;
use std::path::Path;


use serde_json::{json, Value};

/// Total memory.md budget — mirrors the janitor's CONTEXT_BUDGET_BYTES.
pub const BUDGET_BYTES: u64 = 131_072;
/// The cheap tail-summarizer (the verify gate covers quality).
const CONDENSE_MODEL: &str = "claude-haiku-4-5";
/// Hard cap so a wedged headless run can't stall the loop.
/// (Now lives in hidden_claude.rs — kept here for the doc reference.)

pub const PINNED_HEADING: &str = "## 📌 Durable facts (pinned — never condensed)";
pub const CONDENSED_HEADING: &str = "## 🗜 Condensed history";
pub const RECENT_HEADING: &str = "## Recent";

/// Line-oriented output protocol — prompt and parser share these literals.
const CONDENSED_MARKER: &str = "<<<CONDENSED>>>";
const HOIST_MARKER: &str = "<<<HOIST>>>";
const END_MARKER: &str = "<<<END>>>";
const HOIST_BULLET_PREFIX: &str = "- ";

/// Instruction prefix — byte-identical across calls so Claude Code's prompt
/// cache hits; the dynamic content goes in the tail.
const CONDENSE_SYSTEM: &str = concat!(
    "You are compacting one AI agent's long-term memory file. You will receive:",
    "\n(A) the current CONDENSED summary, (B) older RECENT sections being evicted,",
    "\n(C) the PINNED durable-facts block (for context only — do not rewrite it).",
    "\nOutput exactly one block in this format:",
    "\n<<<CONDENSED>>>",
    "\n<free-form condensed summary>",
    "\n<<<HOIST>>>",
    "\n- <new durable fact, one per line>",
    "\n<<<END>>>",
    "\nRULES:",
    "\n- The condensed section = a single bounded summary of (A)+(B). Re-summarize (A) together",
    "\n  with (B) so the result does not grow unbounded. Target <= 1500 words. Preserve",
    "\n  every decision, root cause, protocol, file path, commit SHA, and numeric result.",
    "\n  Drop routine standup chatter, resolved blockers, and superseded plans.",
    "\n- Write the condensed section as literal free-form text; do not JSON-encode or escape it.",
    "\n- The hoist section = any NEW high-importance durable fact found in (B) that belongs",
    "\n  in the pinned block and is not already in (C). Prefix every fact with \"- \".",
    "\n- Leave the hoist section empty when there are no new durable facts.",
    "\n- <<<CONDENSED>>>, <<<HOIST>>>, <<<END>>> must each appear exactly once on a line by itself,",
    "\n  in that order. Never copy these marker strings into either section.",
    "\n- Output ONLY the framed block. No surrounding prose or code fence."
);

#[derive(Clone, Copy)]
pub struct ReflectSettings {
    pub enabled: bool,
    pub interval_ms: u64,
    /// Condense when bytes exceed this percent of BUDGET_BYTES.
    pub byte_trigger_pct: u64,
    /// ...OR when `## ` section count exceeds this (AND bytes > min_bytes).
    pub section_trigger: usize,
    /// Newest K verbatim `## ` sections always kept untouched.
    pub recent_keep: usize,
    /// Never condense a file smaller than this.
    pub min_bytes: u64,
}

impl Default for ReflectSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_ms: 30 * 60_000,
            byte_trigger_pct: 70,
            section_trigger: 12,
            recent_keep: 4,
            min_bytes: 48 * 1024,
        }
    }
}

/// A `## ` section: heading line + body beneath it.
#[derive(Clone, PartialEq)]
pub struct Section {
    pub heading: String,
    pub body: String,
}

/// memory.md split into the three regions; pinned/condensed are None for
/// legacy files — they're created on first condense.
pub struct Parsed {
    pub header: String,
    pub pinned: Option<String>,
    pub condensed: Option<String>,
    pub recent: Vec<Section>,
}

/// Count level-2 (`## `) headings — `#` H1 and `###` deeper excluded.
pub fn count_sections(text: &str) -> usize {
    text.lines().filter(|l| l.starts_with("## ")).count()
}

/// Split a memory.md into header + the three regions.
pub fn parse_memory(text: &str) -> Parsed {
    let lines: Vec<&str> = text.split('\n').collect();
    let first = lines.iter().position(|l| l.starts_with("## ")).unwrap_or(lines.len());
    let header = lines[..first].join("\n").trim_end().to_string();
    let mut sections: Vec<Section> = Vec::new();
    let mut cur: Option<Section> = None;
    for line in &lines[first..] {
        if line.starts_with("## ") {
            if let Some(s) = cur.take() {
                sections.push(s);
            }
            cur = Some(Section { heading: line.to_string(), body: String::new() });
        } else if let Some(s) = &mut cur {
            if !s.body.is_empty() {
                s.body.push('\n');
            }
            s.body.push_str(line);
        }
    }
    if let Some(s) = cur {
        sections.push(s);
    }
    let mut parsed = Parsed {
        header,
        pinned: None,
        condensed: None,
        recent: Vec::new(),
    };
    for s in sections {
        let h = s.heading.trim();
        if h.starts_with("## 📌") {
            parsed.pinned = Some(s.body.trim_end().to_string());
        } else if h.starts_with("## 🗜") {
            parsed.condensed = Some(s.body.trim_end().to_string());
        } else if h == RECENT_HEADING {
            // divider — its siblings ARE the recent list
        } else {
            parsed.recent.push(s);
        }
    }
    parsed
}

/// Non-empty trimmed lines of the pinned block — the set we must never lose.
pub fn pinned_lines(pinned: Option<&str>) -> Vec<String> {
    pinned
        .map(|p| p.split('\n').map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default()
}

/// Append hoisted durable facts, skipping any already present.
pub fn merge_pinned(old: &[String], hoist: &[String]) -> Vec<String> {
    let mut have: std::collections::HashSet<&str> = old.iter().map(|s| s.as_str()).collect();
    let mut out = old.to_vec();
    for raw in hoist {
        let line = raw.trim();
        if !line.is_empty() && have.insert(line) {
            out.push(line.to_string());
        }
    }
    out
}

/// Reassemble the canonical 3-region file.
pub fn rebuild(header: &str, pinned: &[String], condensed: &str, keep: &[Section]) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !header.trim().is_empty() {
        parts.push(header.trim().to_string());
    }
    parts.push(PINNED_HEADING.to_string());
    parts.push(if pinned.is_empty() { "_(none yet)_".into() } else { pinned.join("\n") });
    parts.push(CONDENSED_HEADING.to_string());
    parts.push(condensed.trim().to_string());
    parts.push(RECENT_HEADING.to_string());
    for s in keep {
        parts.push(format!("{}\n{}", s.heading, s.body).trim_end().to_string());
    }
    format!("{}\n", parts.join("\n\n"))
}

/// The verify-don't-trust gate — every check must hold or the rewrite is
/// rejected (the backup makes a rejection a pure no-op).
pub fn verify(rebuilt: &str, old_bytes: usize, old_pinned: &[String], merged: &[String], condensed: &str, keep: &[Section]) -> Result<(), String> {
    let re = parse_memory(rebuilt);
    if re.pinned.is_none() || re.condensed.is_none() {
        return Err("structure-missing-region".into());
    }
    let new_bytes = rebuilt.len();
    if new_bytes <= 200 {
        return Err("too-small".into());
    }
    if condensed.trim().is_empty() {
        return Err("empty-condensed".into());
    }
    if !((new_bytes as f64) < old_bytes as f64 * 0.95) {
        return Err("not-smaller".into());
    }
    let new_pinned: std::collections::HashSet<String> =
        pinned_lines(re.pinned.as_deref()).into_iter().collect();
    for line in old_pinned {
        if !new_pinned.contains(line) {
            return Err("pinned-line-dropped".into());
        }
    }
    for line in merged {
        if !new_pinned.contains(line) {
            return Err("pinned-merge-mismatch".into());
        }
    }
    if re.recent.len() != keep.len() {
        return Err("recent-count-mismatch".into());
    }
    for (i, s) in keep.iter().enumerate() {
        let a = format!("{}\n{}", s.heading, s.body).trim_end().to_string();
        let b = format!("{}\n{}", re.recent[i].heading, re.recent[i].body).trim_end().to_string();
        if a != b {
            return Err("recent-section-altered".into());
        }
    }
    Ok(())
}

/* ── summary parsing (envelope → fence → framed | legacy JSON) ────────────── */

fn unwrap_cli_envelope(raw: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        for k in ["result", "text"] {
            if let Some(s) = v.get(k).and_then(|x| x.as_str()) {
                return s.to_string();
            }
        }
    }
    raw.to_string()
}

/// Strip a complete outer code fence; an opening fence with no exact closing
/// line REJECTS (returns None) rather than guessing.
fn strip_outer_fence(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let first = lines.first()?.trim();
    if !first.starts_with("```") || first.len() > 20 {
        return Some(text.to_string());
    }
    if lines.len() < 2 || lines.last()?.trim() != "```" {
        return None;
    }
    Some(lines[1..lines.len() - 1].join("\n").trim().to_string())
}

fn find_unique_marker(lines: &[&str], marker: &str) -> Option<usize> {
    let mut found: Option<usize> = None;
    for (i, l) in lines.iter().enumerate() {
        if l.trim() == marker {
            if found.is_some() {
                return None;
            }
            found = Some(i);
        }
    }
    found
}

fn parse_framed(text: &str) -> Option<(String, Vec<String>)> {
    let lines: Vec<&str> = text.split('\n').collect();
    let c = find_unique_marker(&lines, CONDENSED_MARKER)?;
    let h = find_unique_marker(&lines, HOIST_MARKER)?;
    let e = find_unique_marker(&lines, END_MARKER)?;
    if c != 0 || e != lines.len() - 1 || !(c < h && h < e) {
        return None;
    }
    let condensed = lines[c + 1..h].join("\n").trim().to_string();
    if condensed.is_empty() {
        return None;
    }
    let mut hoist = Vec::new();
    for line in &lines[h + 1..e] {
        let b = line.trim();
        if b.is_empty() {
            continue;
        }
        let fact = b.strip_prefix(HOIST_BULLET_PREFIX)?.trim();
        if fact.is_empty() {
            return None;
        }
        hoist.push(fact.to_string());
    }
    Some((condensed, hoist))
}

fn parse_legacy_json(text: &str) -> Option<(String, Vec<String>)> {
    let v: Value = serde_json::from_str(text).ok()?;
    let condensed = v.get("condensed")?.as_str()?;
    if condensed.trim().is_empty() {
        return None;
    }
    let hoist = v
        .get("hoist")
        .and_then(|h| h.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    Some((condensed.to_string(), hoist))
}

/// Pull (condensed, hoist) out of CLI output. A partial frame fails closed —
/// never reinterpreted as legacy JSON.
pub fn parse_summary(stdout: &str) -> Option<(String, Vec<String>)> {
    let raw = stdout.trim();
    if raw.is_empty() {
        return None;
    }
    let unwrapped = unwrap_cli_envelope(raw).replace("\r\n", "\n").replace('\r', "\n");
    let unwrapped = unwrapped.trim();
    if unwrapped.is_empty() {
        return None;
    }
    let inner = strip_outer_fence(unwrapped)?;
    let lines: Vec<&str> = inner.split('\n').collect();
    if [CONDENSED_MARKER, HOIST_MARKER, END_MARKER]
        .iter()
        .any(|m| lines.iter().any(|l| l.trim() == *m))
    {
        return parse_framed(&inner);
    }
    parse_legacy_json(&inner)
}

/* ── the condense pipeline ────────────────────────────────────────────────── */

/// `20260606T110912Z` — matches the janitor's backup-dir stamp.
fn utc_stamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

/// Atomic write: temp sibling → rename over target.
fn atomic_write(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)
}

/// Run a hidden interactive claude session (PTY-based, not `claude -p`) and
/// return the assistant's final text response. Uses the user's interactive
/// plan quota, not the Agent SDK credit. Port of `hiddenClaude.ts`.
fn run_hidden_claude(command: &str, prompt: &str, cwd: &Path) -> Result<String, String> {
    let opts = crate::hidden_claude::HiddenClaudeOptions {
        model: CONDENSE_MODEL.into(),
        cwd: cwd.display().to_string(),
        command: Some(command.into()),
        disallowed_tools: vec!["Edit".into(), "Write".into(), "NotebookEdit".into(), "Bash".into()],
        ..Default::default()
    };
    let result = crate::hidden_claude::run_hidden_claude(prompt, &opts);
    match (result.ok, result.text, result.error) {
        (true, Some(text), _) => Ok(text),
        (true, None, _) => Err("no assistant response found in transcript".into()),
        (false, _, Some(e)) => Err(e),
        (false, _, None) => Err("hidden session failed".into()),
    }
}

/// The dual trigger: bytes > pct% of budget, OR section sprawl above the
/// byte floor (which doubles as the don't-burn-an-LLM-call guard).
fn should_condense(bytes: u64, mem: &Path, s: &ReflectSettings) -> bool {
    if bytes < s.min_bytes {
        return false;
    }
    if bytes > BUDGET_BYTES * s.byte_trigger_pct / 100 {
        return true;
    }
    fs::read_to_string(mem)
        .map(|t| count_sections(&t) > s.section_trigger)
        .unwrap_or(false)
}

/// Condense ONE agent's memory.md — the full pipeline. Returns a result map
/// mirroring upstream's ReflectResult.
pub fn condense_one(hive_root: &Path, id: &str, command: &str, s: &ReflectSettings, manual: bool) -> Value {
    let mem = hive_root.join("agents").join(id).join("memory.md");
    let Ok(text) = fs::read_to_string(&mem) else {
        return json!({"id": id, "condensed": false, "reason": "no-memory"});
    };
    let old_bytes = text.len() as u64;
    if !manual && !should_condense(old_bytes, &mem, s) {
        return json!({"id": id, "condensed": false, "reason": "below-threshold", "oldBytes": old_bytes});
    }
    let parsed = parse_memory(&text);
    let keep_n = s.recent_keep.max(1);
    let split = parsed.recent.len().saturating_sub(keep_n);
    let (evict, keep) = (&parsed.recent[..split], &parsed.recent[split..]);
    if evict.is_empty() {
        return json!({"id": id, "condensed": false, "reason": "nothing-to-evict", "oldBytes": old_bytes});
    }
    // 1) BACK UP first.
    let backup = hive_root.join("backups").join(utc_stamp()).join(id).join("memory.md");
    if let Some(dir) = backup.parent() {
        if fs::create_dir_all(dir).and_then(|_| fs::copy(&mem, &backup)).is_err() {
            return json!({"id": id, "condensed": false, "reason": "backup-failed", "oldBytes": old_bytes});
        }
    }
    // 2) SUMMARIZE the (condensed + evicted) tail via headless Haiku.
    let evict_text = evict
        .iter()
        .map(|s| format!("{}\n{}", s.heading, s.body))
        .collect::<Vec<_>>()
        .join("\n\n");
    let prompt = format!(
        "{}\n\n--- INPUT ---\n(A) CURRENT CONDENSED SUMMARY:\n{}\n\n(B) OLDER SECTIONS BEING EVICTED:\n{}\n\n(C) PINNED DURABLE FACTS (context only — do not rewrite):\n{}",
        CONDENSE_SYSTEM,
        parsed.condensed.as_deref().map(str::trim).filter(|t| !t.is_empty()).unwrap_or("(none yet)"),
        if evict_text.trim().is_empty() { "(none)" } else { evict_text.trim() },
        parsed.pinned.as_deref().map(str::trim).filter(|t| !t.is_empty()).unwrap_or("(none)"),
    );
    let out = match run_hidden_claude(command, &prompt, hive_root) {
        Ok(t) => t,
        Err(e) => {
            return json!({"id": id, "condensed": false, "reason": "summarize-failed", "detail": e, "oldBytes": old_bytes})
        }
    };
    let Some((condensed, hoist)) = parse_summary(&out) else {
        return json!({"id": id, "condensed": false, "reason": "summarize-failed", "detail": "no parseable summary", "oldBytes": old_bytes});
    };
    // 3) REBUILD + 4) VERIFY-DON'T-TRUST.
    let old_pinned = pinned_lines(parsed.pinned.as_deref());
    let merged = merge_pinned(&old_pinned, &hoist);
    let rebuilt = rebuild(&parsed.header, &merged, &condensed, keep);
    if let Err(reason) = verify(&rebuilt, text.len(), &old_pinned, &merged, &condensed, keep) {
        return json!({"id": id, "condensed": false, "reason": reason, "oldBytes": old_bytes, "newBytes": rebuilt.len()});
    }
    // 5) ATOMIC SWAP.
    if atomic_write(&mem, &rebuilt).is_err() {
        return json!({"id": id, "condensed": false, "reason": "swap-failed", "oldBytes": old_bytes, "newBytes": rebuilt.len()});
    }
    json!({
        "id": id, "condensed": true, "reason": "condensed",
        "oldBytes": old_bytes, "newBytes": rebuilt.len(),
        "evicted": evict.len(), "kept": keep.len(), "hoisted": hoist.len(),
        "backup": backup.display().to_string(),
    })
}

/// Scan every agent's memory.md once (serialized by the caller's interval).
pub fn reflect_scan(hive_root: &Path, claude_cmd: &str, s: &ReflectSettings) -> Vec<Value> {
    let agents_dir = hive_root.join("agents");
    let Ok(rd) = fs::read_dir(&agents_dir) else { return vec![] };
    let mut out = Vec::new();
    for e in rd.flatten() {
        if !e.path().join("memory.md").exists() {
            continue;
        }
        let id = e.file_name().to_string_lossy().to_string();
        out.push(condense_one(hive_root, &id, claude_cmd, s, false));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY: &str = "# Memory — w1\n\n## 2026-01-01 shift\nold notes here\n\n## 2026-01-02 shift\nmore notes\n\n## 2026-01-03 shift\nrecent\n";

    #[test]
    fn parse_memory_splits_regions() {
        let p = parse_memory(LEGACY);
        assert_eq!(p.recent.len(), 3);
        assert!(p.pinned.is_none() && p.condensed.is_none());
        assert_eq!(p.header, "# Memory — w1");
        // Structured file: pinned + condensed + recent divider.
        let structured = rebuild(&p.header, &["- fact A".to_string()], "the summary", &p.recent[..1]);
        let q = parse_memory(&structured);
        assert!(q.pinned.is_some() && q.condensed.is_some());
        assert_eq!(q.recent.len(), 1);
    }

    #[test]
    fn merge_pinned_dedupes_and_verify_gates() {
        let merged = merge_pinned(&["- a".into()], &["- a".into(), "- b".into()]);
        assert_eq!(merged, vec!["- a", "- b"]);
        // verify: a rewrite that drops a pinned line is rejected — long
        // enough condensed text to clear the too-small gate first.
        let header = "# Memory";
        let old_pinned = vec!["- keep-me".to_string()];
        let keep = parse_memory(LEGACY).recent;
        let keep = &keep[keep.len() - 1..];
        let long = "condensed text ".repeat(40);
        let bad = rebuild(header, &[], &long, keep);
        let merged_empty: Vec<String> = vec![];
        let v = verify(&bad, 9000, &old_pinned, &merged_empty, &long, keep);
        assert_eq!(v.unwrap_err(), "pinned-line-dropped");
        // And a legit smaller rewrite passes.
        let good = rebuild(header, &old_pinned, &"smaller summary of everything. ".repeat(8), keep);
        assert!(
            verify(&good, 4000, &old_pinned, &old_pinned, "smaller summary", keep).is_ok(),
            "{:?}", verify(&good, 4000, &old_pinned, &old_pinned, "smaller summary", keep)
        );
    }

    #[test]
    fn parse_summary_framed_legacy_and_rejects() {
        // Framed output.
        let framed = "<<<CONDENSED>>>\nsummary text\n<<<HOIST>>>\n- new fact\n<<<END>>>";
        let (c, h) = parse_summary(framed).unwrap();
        assert_eq!(c, "summary text");
        assert_eq!(h, vec!["new fact"]);
        // Legacy JSON (CLI envelope → inner frame).
        let env = r#"{"result":"{\"condensed\":\"s2\",\"hoist\":[\"f\"]}"}"#;
        let (c2, _) = parse_summary(env).unwrap();
        assert_eq!(c2, "s2");
        // A partial frame fails closed — never reinterpreted.
        assert!(parse_summary("<<<CONDENSED>>>\nonly").is_none());
        assert!(parse_summary("random prose").is_none());
        // Non-marker hoist line rejects.
        assert!(parse_summary("<<<CONDENSED>>>\ns\n<<<HOIST>>>\nnot a bullet\n<<<END>>>").is_none());
    }
}
