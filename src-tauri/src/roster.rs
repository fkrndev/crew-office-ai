//! The roster on disk — port of `roster.ts`.
//!
//! WHY IT EXISTS UPSTREAM: the UI floor (cards, notes, queues) lived in
//! localStorage, which is partitioned by ORIGIN — a dev run and a packaged
//! build could never see each other's state. A file keyed on the harness
//! home is addressed by path, not origin, so both shares it. Tauri has one
//! origin, but the file still carries UI state the registry doesn't (notes,
//! parked queues, archived/restorable entries, selectedId).
//!
//! Durability rules, verbatim priorities:
//!   1. Never lose a roster — every write first copies the previous file into
//!      `roster-backups/`; only the newest `KEEP_BACKUPS` are retained so the
//!      folder can't grow without bound (every write adds one).
//!   2. Never write a truncated file — unique temp + rename.
//!   3. Never let an empty write erase a full roster — the empty-guard below.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Backups retained in `roster-backups/` (newest wins).
const KEEP_BACKUPS: usize = 20;

/// Renderer-owned snapshot; the inner agent shape stays opaque here (the
/// store owns it — repeating it would couple every agent-field change).
pub struct RosterStore {
    home: PathBuf,
    /// Disarmed only after a NON-empty write lands in THIS process — the
    /// empty-guard's memory. Kept on the instance so a reload can't re-arm
    /// it (the 2026-08-16 incident: two refusal-worthy writes minutes apart).
    wrote: std::sync::Mutex<bool>,
    /// Same-millisecond disambiguator for backup filenames.
    seq: std::sync::Mutex<u64>,
}

impl RosterStore {
    pub fn new(home: PathBuf) -> Self {
        Self {
            home,
            wrote: std::sync::Mutex::new(false),
            seq: std::sync::Mutex::new(0),
        }
    }

    fn roster_path(&self) -> PathBuf {
        self.home.join("roster.json")
    }
    fn backup_dir(&self) -> PathBuf {
        self.home.join("roster-backups")
    }

    fn is_snapshot(v: &Value) -> bool {
        v.get("agents").is_some_and(|a| a.is_array())
            && v.get("archived").is_some_and(|a| a.is_array())
            && v.get("restorable").is_some_and(|a| a.is_array())
    }

    fn entry_count(v: &Value) -> usize {
        ["agents", "archived", "restorable"]
            .iter()
            .map(|k| {
                v.get(k)
                    .and_then(|a| a.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0)
            })
            .sum()
    }

    /// The stored roster, or None ("no opinion" → the renderer falls back to
    /// localStorage, degrading to the old behaviour rather than an empty floor).
    pub fn read(&self) -> Option<Value> {
        let text = fs::read_to_string(self.roster_path()).ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        Self::is_snapshot(&v).then_some(v)
    }

    /// Copy the current roster into the append-only backup folder.
    fn backup(&self, reason: &str) {
        let p = self.roster_path();
        if !p.exists() {
            return;
        }
        let _ = fs::create_dir_all(self.backup_dir());
        let mut seq = self.seq.lock().unwrap();
        *seq += 1;
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let _ = fs::copy(
            &p,
            self.backup_dir()
                .join(format!("roster-{ts}-{reason}-{}.json", *seq)),
        );
        drop(seq);
        self.prune_backups(KEEP_BACKUPS);
    }

    /// Drop all but the newest `keep` backups. Order is the epoch-ms stamp in
    /// the name (then the per-run sequence); unparseable names sort oldest.
    fn prune_backups(&self, keep: usize) {
        let Ok(rd) = fs::read_dir(self.backup_dir()) else {
            return;
        };
        let key = |name: &str| -> (u64, u64) {
            let mut parts = name.trim_end_matches(".json").split('-');
            let ts = parts.nth(1).and_then(|t| t.parse().ok()).unwrap_or(0);
            let seq = name
                .trim_end_matches(".json")
                .rsplit('-')
                .next()
                .and_then(|t| t.parse().ok())
                .unwrap_or(0);
            (ts, seq)
        };
        let mut files: Vec<(u64, u64, PathBuf)> = rd
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("roster-"))
            .map(|e| {
                let (ts, seq) = key(&e.file_name().to_string_lossy());
                (ts, seq, e.path())
            })
            .collect();
        if files.len() <= keep {
            return;
        }
        files.sort();
        for (_, _, path) in &files[..files.len() - keep] {
            let _ = fs::remove_file(path);
        }
    }

    /// Write the roster, backing up the previous contents first.
    /// THE EMPTY-GUARD: an empty write is refused until this run has landed a
    /// non-empty one — the dangerous sequence is a first-boot renderer with
    /// empty state flattening a file that holds a real roster. The refusal
    /// never disarms the guard (only a non-empty write does).
    pub fn write(&self, snap: &Value) -> Result<(), String> {
        if !Self::is_snapshot(snap) {
            return Err("invalid snapshot".into());
        }
        let _ = fs::create_dir_all(&self.home);
        let existing = self.read();
        let mut wrote = self.wrote.lock().unwrap();
        if !*wrote
            && existing
                .as_ref()
                .map(|e| Self::entry_count(e) > 0)
                .unwrap_or(false)
            && Self::entry_count(snap) == 0
        {
            // Back up anyway — what's on disk is what we're protecting.
            self.backup("declined");
            return Err("skipped: empty-first-write".into());
        }
        self.backup(if *wrote { "write" } else { "run-start" });
        let body = serde_json::json!({
            "version": 1,
            "savedAt": chrono::Utc::now().to_rfc3339(),
            "agents": snap["agents"],
            "archived": snap["archived"],
            "restorable": snap["restorable"],
            "queues": snap.get("queues").filter(|q| q.is_object()).cloned().unwrap_or(serde_json::json!({})),
            "selectedId": snap.get("selectedId").filter(|s| s.is_string()).cloned().unwrap_or(serde_json::Value::Null),
        });
        crate::hive::atomic_write_file(
            &self.roster_path(),
            serde_json::to_string_pretty(&body)
                .map_err(|e| e.to_string())?
                .as_bytes(),
        )
        .map_err(|e| e.to_string())?;
        *wrote = true;
        Ok(())
    }

    /// Retire the roster during a full reset — backed up, then removed.
    /// A roster is never destroyed, only superseded.
    pub fn archive(&self) {
        let p = self.roster_path();
        if p.exists() {
            self.backup("reset");
            let _ = fs::remove_file(&p);
        }
    }
}

/// The roster file's home — sibling of the hive dir (harness home).
pub fn roster_path(home: &Path) -> PathBuf {
    home.join("roster.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(tag: &str) -> (RosterStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!("oa-roster-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        (RosterStore::new(dir.clone()), dir)
    }

    fn snap(n: usize) -> Value {
        let agents: Vec<Value> = (0..n)
            .map(|i| serde_json::json!({"id": format!("a{i}")}))
            .collect();
        serde_json::json!({"agents": agents, "archived": [], "restorable": [], "queues": {}, "selectedId": null})
    }

    #[test]
    fn empty_first_write_refused_then_disarmed() {
        let (st, dir) = store("empty");
        st.write(&snap(2)).unwrap();
        // A fresh store instance (fresh process): the on-disk roster has 2
        // entries and this run has written nothing — an empty write declines.
        let st2 = RosterStore::new(dir.clone());
        assert!(st2
            .write(&snap(0))
            .unwrap_err()
            .contains("empty-first-write"));
        assert!(st2.read().unwrap()["agents"].as_array().unwrap().len() == 2);
        // A non-empty write lands and disarms the guard for later empties.
        st2.write(&snap(1)).unwrap();
        st2.write(&snap(0)).unwrap();
        assert_eq!(st2.read().unwrap()["agents"].as_array().unwrap().len(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn backups_are_pruned_to_the_newest_few() {
        let (st, dir) = store("prune");
        for i in 1..=(KEEP_BACKUPS + 15) {
            st.write(&snap(i % 3 + 1)).unwrap();
        }
        let n = std::fs::read_dir(dir.join("roster-backups")).unwrap().count();
        assert_eq!(n, KEEP_BACKUPS);
        // The roster itself is intact.
        assert!(st.read().is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn every_write_leaves_a_backup() {
        let (st, dir) = store("bak");
        st.write(&snap(1)).unwrap();
        st.write(&snap(2)).unwrap();
        // First-ever write has nothing to back up; the second backs up write #1.
        let backups = std::fs::read_dir(dir.join("roster-backups"))
            .unwrap()
            .count();
        assert!(backups >= 1, "prior file backed up");
        st.archive();
        assert!(!roster_path(&dir).exists());
        let backups = std::fs::read_dir(dir.join("roster-backups"))
            .unwrap()
            .count();
        assert!(backups >= 2, "reset backup");
        std::fs::remove_dir_all(&dir).ok();
    }
}
