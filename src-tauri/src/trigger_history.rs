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
