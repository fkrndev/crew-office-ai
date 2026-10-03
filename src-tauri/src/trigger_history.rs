//! The trigger ledger — port of `triggerHistory.ts` + the
//! `TriggerHistoryEntry` contract from `shared/triggers.ts`.
//!
//! Every inbound message a webhook (or, later, a peer's clone node) pushed at
//! us, and every reply sent back. Its OWN file — not config.json — because
//! config.json is rewritten wholesale on every settings write and a growing
//! array would tax every unrelated save, and because the ledger is
//! disposable: losing it must never cost the user their settings.
//!
//! SECURITY (verbatim from upstream): NEVER write a secret, API key or token
//! into an entry. The ledger is plaintext JSON surfaced verbatim in the UI, so
//! entries are built FIELD BY FIELD — never by spreading a caller's object
//! (a WebhookEndpoint carries `secret`; it must never land here).
//!
//! Nothing here throws at its caller: a ledger write must never fail the
//! event it records.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Ledger cap — oldest entries drop past this (TRIGGER_HISTORY_LIMIT).
pub const TRIGGER_HISTORY_LIMIT: usize = 500;

/// classifyInboundKind — best-effort intent guess when the payload doesn't
/// declare `kind`. Deliberately conservative: anything we aren't confident is
/// chatter is a DIRECTIVE, because mis-labelling a directive as communication
/// is what lets unapproved work through a communication-only gate.
pub fn classify_inbound_kind(text: &str) -> &'static str {
    let t = text.trim().to_lowercase();
    if t.is_empty() {
        return "communication";
    }
    let opener = [
        "what", "how", "when", "where", "who", "why", "is", "are", "do", "does", "did", "can",
        "could", "status", "any",
    ]
    .iter()
    .any(|w| t.starts_with(w) && (t.len() == w.len() || t[w.len()..].starts_with(' ')));
    let imperative = [
        "fix", "build", "ship", "deploy", "run", "write", "create", "add", "remove", "delete",
        "refactor", "implement", "update", "merge", "revert",
    ]
    .iter()
    .any(|w| {
        t.split(|c: char| !c.is_alphanumeric())
            .any(|tok| tok == *w)
    });
    if opener && t.ends_with('?') && !imperative {
        "communication"
    } else {
        "directive"
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerHistoryEntry {
    pub id: String,
    /// 'webhook' today; 'org' when peer nodes land.
    pub source: String,
    /// Which webhook (or peer) — the endpoint id for webhooks.
    #[serde(default)]
    pub source_id: String,
    /// Display name at event time — survives a rename/delete.
    #[serde(default)]
    pub source_name: String,
    /// 'inbound' | 'outbound' — both sides recorded as one conversation.
    pub direction: String,
    /// The other party: who sent it to us, or who we sent it to.
    #[serde(default)]
    pub peer: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<String>,
    /// Full message body — never truncated at rest.
    pub body: String,
    /// 'directive' | 'communication'.
    pub kind: String,
    /// 'auto-allowed' | 'pending' | 'approved' | 'rejected' — the verdict.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub decision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub correlation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub task_id: Option<String>,
    pub at: u64,
}

#[derive(Clone)]
pub struct TriggerHistory {
    path: PathBuf,
}

impl TriggerHistory {
    pub fn new(dir: PathBuf) -> Self {
        Self { path: dir.join("trigger-history.json") }
    }

    /// Newest first ON DISK as well as in memory — the UI's hot path is a
    /// slice, not a sort (the cap is enforced on append).
    fn read_all(&self) -> Vec<TriggerHistoryEntry> {
        let Ok(text) = fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        let Ok(parsed) = serde_json::from_str::<Vec<Value>>(&text) else {
            return Vec::new();
        };
        parsed
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .filter(|e: &TriggerHistoryEntry| {
                !e.id.is_empty()
                    && (e.source == "webhook" || e.source == "org")
                    && (e.direction == "inbound" || e.direction == "outbound")
            })
            .collect()
    }

    fn write_all(&self, entries: &[TriggerHistoryEntry]) {
        let _ = fs::create_dir_all(self.path.parent().unwrap());
        if let Ok(s) = serde_json::to_string_pretty(entries) {
            let _ = fs::write(&self.path, s);
        }
    }

    /// Record one event; returns the row as persisted (the caller needs the
    /// generated id to flip a `pending` decision later).
    pub fn append(&self, mut entry: TriggerHistoryEntry) -> TriggerHistoryEntry {
        if entry.id.is_empty() {
            entry.id = uuid::Uuid::new_v4().simple().to_string()[..16].to_string();
        }
        if entry.at == 0 {
            entry.at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
        }
        if entry.kind != "communication" {
            entry.kind = "directive".into(); // normaliseKind's contract
        }
        let mut all = self.read_all();
        all.insert(0, entry.clone());
        all.truncate(TRIGGER_HISTORY_LIMIT);
        self.write_all(&all);
        entry
    }

    /// The whole ledger, newest first.
    pub fn list(&self) -> Vec<TriggerHistoryEntry> {
        self.read_all()
    }

    /// Amend one entry's verdict — the `pending`→`approved`/`rejected`
    /// transition (+ the taskId whatever the approval spawned). Absent ids
    /// are normal: the entry may have aged past the cap.
    pub fn update(
        &self,
        id: &str,
        decision: Option<String>,
        task_id: Option<String>,
        correlation_id: Option<String>,
        title: Option<String>,
    ) -> Option<TriggerHistoryEntry> {
        let mut all = self.read_all();
        let i = all.iter().position(|e| e.id == id)?;
        let e = &mut all[i];
        if let Some(d) = decision {
            e.decision = Some(d);
        }
        if let Some(t) = task_id {
            e.task_id = Some(t);
        }
        if let Some(c) = correlation_id {
            e.correlation_id = Some(c);
        }
        if let Some(t) = title {
            e.title = Some(t);
        }
        let out = e.clone();
        self.write_all(&all);
        Some(out)
    }

    /// Wipe the ledger, or just one source's half (clearing webhook noise
    /// must not throw away the org conversation, and vice versa).
    pub fn clear(&self, source: Option<&str>) {
        match source {
            None => {
                let _ = fs::remove_file(&self.path);
            }
            Some(s) => {
                let kept: Vec<_> = self.read_all().into_iter().filter(|e| e.source != s).collect();
                self.write_all(&kept);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger() -> (TriggerHistory, PathBuf) {
        let dir = std::env::temp_dir().join(format!("oa-trig-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        (TriggerHistory::new(dir.clone()), dir)
    }

    fn entry(id: &str, decision: Option<&str>) -> TriggerHistoryEntry {
        TriggerHistoryEntry {
            id: id.into(),
            source: "webhook".into(),
            source_id: "ep1".into(),
            source_name: "hook".into(),
            direction: "inbound".into(),
            peer: "sender".into(),
            title: None,
            body: "do the thing".into(),
            kind: "directive".into(),
            decision: decision.map(|d| d.into()),
            correlation_id: None,
            task_id: None,
            at: 0,
        }
    }

    #[test]
    fn append_is_newest_first_and_normalises_kind() {
        let (h, dir) = ledger();
        h.append(entry("a", None));
        h.append(entry("b", None));
        let all = h.list();
        assert_eq!(all[0].id, "b");
        assert_eq!(all[1].id, "a");
        let mut c = entry("c", None);
        c.kind = "bogus".into(); // normaliseKind: anything ≠ communication → directive
        let e = h.append(c);
        assert_eq!(e.kind, "directive");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn update_flips_verdict_and_records_task() {
        let (h, dir) = ledger();
        let e = h.append(entry("x", Some("pending")));
        let out = h
            .update(&e.id, Some("approved".into()), Some("task-9".into()), None, None)
            .unwrap();
        assert_eq!(out.decision.as_deref(), Some("approved"));
        assert_eq!(out.task_id.as_deref(), Some("task-9"));
        // Aged-out ids are a normal miss, not an error.
        assert!(h.update("nope", Some("rejected".into()), None, None, None).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clear_by_source_keeps_other_half() {
        let (h, dir) = ledger();
        h.append(entry("w", None));
        let mut org = entry("o", None);
        org.source = "org".into();
        h.append(org);
        h.clear(Some("webhook"));
        let all = h.list();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "o");
        h.clear(None);
        assert!(h.list().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cap_drops_oldest() {
        let (h, dir) = ledger();
        for i in 0..(TRIGGER_HISTORY_LIMIT + 5) {
            let mut e = entry(&format!("e{i}"), None);
            e.at = i as u64;
            h.append(e);
        }
        let all = h.list();
        assert_eq!(all.len(), TRIGGER_HISTORY_LIMIT);
        assert_eq!(all[0].id, format!("e{}", TRIGGER_HISTORY_LIMIT + 4)); // newest first
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn classify_matches_upstream_golden_cases() {
        // Ported verbatim from classifyInboundKind: question-opener + '?'
        // + no imperative verb = communication; everything else directive.
        assert_eq!(classify_inbound_kind(""), "communication");
        assert_eq!(classify_inbound_kind("what is the weather?"), "communication");
        assert_eq!(classify_inbound_kind("can you see the progress?"), "communication");
        assert_eq!(classify_inbound_kind("what is the build status?"), "directive"); // 'build' is imperative
        assert_eq!(classify_inbound_kind("what should we deploy?"), "directive"); // imperative inside
        assert_eq!(classify_inbound_kind("deploy the release now"), "directive");
        assert_eq!(classify_inbound_kind("status"), "directive"); // no '?'
    }
}
