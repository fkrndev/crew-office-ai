//! PersistStore — durable harness state in SQLite (port of `db.ts`).
//!
//! Phase A scope, verbatim:
//!   - kv:               scalar app state (window bounds etc.)
//!   - command_history:  every prompt the user submits to an agent
//!
//! WAL mode so reads never block the single writer; the file sits under the
//! harness home. Schema evolves via ordered user_version migrations —
//! NEVER edit a shipped migration, only append.

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::{json, Value};

/// Ordered, append-only migrations. Index N takes the DB from user_version N
/// to N+1 — append a new entry to evolve, never edit an existing one.
const MIGRATIONS: &[&str] = &[
    // → user_version 1 (Phase A): scalar kv + net-new command history.
    "CREATE TABLE IF NOT EXISTS kv (
        key        TEXT PRIMARY KEY,
        value      TEXT NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS command_history (
        id       INTEGER PRIMARY KEY AUTOINCREMENT,
        agent_id TEXT NOT NULL,
        cwd      TEXT,
        text     TEXT NOT NULL,
        ts       INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_ch_agent_ts ON command_history(agent_id, ts DESC);",
];

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub struct PersistStore {
    conn: Option<Connection>,
    path: PathBuf,
}

impl PersistStore {
    pub fn new(path: PathBuf) -> Self {
        Self { conn: None, path }
    }

    /// Open (creating if needed) and migrate. Idempotent.
    pub fn open(&mut self) -> Result<(), String> {
        if self.conn.is_some() {
            return Ok(());
        }
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let conn = Connection::open(&self.path).map_err(|e| e.to_string())?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "busy_timeout", 5000i64)
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "foreign_keys", true)
            .map_err(|e| e.to_string())?;
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let mut conn = conn;
        for i in (version as usize)..MIGRATIONS.len() {
            // Each migration + its bump run in one transaction — a crash
            // mid-migration never leaves a half-applied schema.
            let tx = conn.transaction().map_err(|e| e.to_string())?;
            tx.execute_batch(MIGRATIONS[i]).map_err(|e| e.to_string())?;
            tx.pragma_update(None, "user_version", i as i64 + 1)
                .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
        }
        self.conn = Some(conn);
        Ok(())
    }

    pub fn is_open(&self) -> bool {
        self.conn.is_some()
    }

    // ─── kv ───────────────────────────────────────────────────────────────

    pub fn get_kv(&self, key: &str) -> Option<Value> {
        let c = self.conn.as_ref()?;
        let value: String = c
            .query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0))
            .ok()?;
        serde_json::from_str(&value).ok()
    }

    pub fn set_kv(&self, key: &str, value: &Value) {
        let Some(c) = self.conn.as_ref() else { return };
        let _ = c.execute(
            "INSERT INTO kv (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            rusqlite::params![key, value.to_string(), now_ms()],
        );
    }

    // ─── command history ──────────────────────────────────────────────────

    /// Record one submitted prompt — empty text/agent id are ignored.
    pub fn add_history(&self, agent_id: &str, cwd: Option<&str>, text: &str) {
        let Some(c) = self.conn.as_ref() else { return };
        let text = text.trim();
        if text.is_empty() || agent_id.is_empty() {
            return;
        }
        let _ = c.execute(
            "INSERT INTO command_history (agent_id, cwd, text, ts) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![agent_id, cwd, text, now_ms()],
        );
    }

    /// Most-recent-first, optionally scoped to one agent.
    pub fn list_history(&self, agent_id: Option<&str>, limit: i64) -> Vec<Value> {
        let Some(c) = self.conn.as_ref() else {
            return vec![];
        };
        let lim = clamp_limit(limit, 100);
        let sql = if agent_id.is_some() {
            "SELECT id, agent_id, cwd, text, ts FROM command_history WHERE agent_id = ?1 ORDER BY ts DESC, id DESC LIMIT ?2"
        } else {
            "SELECT id, agent_id, cwd, text, ts FROM command_history ORDER BY ts DESC, id DESC LIMIT ?2"
        };
        let Ok(mut stmt) = c.prepare(sql) else {
            return vec![];
        };
        let rows = if let Some(a) = agent_id {
            stmt.query_map(rusqlite::params![a, lim], row_map)
        } else {
            stmt.query_map(rusqlite::params![rusqlite::types::Null, lim], row_map)
        };
        rows.map(|it| it.flatten().collect()).unwrap_or_default()
    }

    /// Substring search over prompt text — LIKE wildcards escaped.
    pub fn search_history(&self, query: &str, limit: i64) -> Vec<Value> {
        let Some(c) = self.conn.as_ref() else {
            return vec![];
        };
        let q = query.trim();
        if q.is_empty() {
            return vec![];
        }
        let lim = clamp_limit(limit, 50);
        let escaped: String = q
            .chars()
            .flat_map(|ch| {
                if matches!(ch, '\\' | '%' | '_') {
                    vec!['\\', ch]
                } else {
                    vec![ch]
                }
            })
            .collect();
        let needle = format!("%{escaped}%");
        c.prepare(
            "SELECT id, agent_id, cwd, text, ts FROM command_history WHERE text LIKE ?1 ESCAPE '\\' ORDER BY ts DESC, id DESC LIMIT ?2",
        )
        .and_then(|mut stmt| {
            stmt.query_map(rusqlite::params![needle, lim], row_map)
                .map(|it| it.flatten().collect())
        })
        .unwrap_or_default()
    }
}

fn row_map(r: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": r.get::<_, i64>(0)?,
        "agentId": r.get::<_, String>(1)?,
        "cwd": r.get::<_, Option<String>>(2)?,
        "text": r.get::<_, String>(3)?,
        "ts": r.get::<_, i64>(4)?,
    }))
}

/// The default DB location — beside config.json under the harness home.
pub fn db_path(home: &Path) -> PathBuf {
    home.join("harness.db")
}

fn clamp_limit(n: i64, fallback: i64) -> i64 {
    if n <= 0 {
        fallback
    } else {
        n.min(1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_kv_and_history() {
        let dir = std::env::temp_dir().join(format!("oa-db-{}", uuid::Uuid::new_v4()));
        let mut st = PersistStore::new(db_path(&dir));
        st.open().unwrap();
        st.open().unwrap(); // idempotent
        st.set_kv("window.bounds", &json!({"w": 1200}));
        assert_eq!(st.get_kv("window.bounds"), Some(json!({"w": 1200})));
        st.set_kv("window.bounds", &json!({"w": 800}));
        assert_eq!(st.get_kv("window.bounds"), Some(json!({"w": 800})));

        st.add_history("w1", Some("C:\\proj"), "do the thing");
        st.add_history("w2", None, "other task");
        st.add_history("", None, "dropped — no agent");
        st.add_history("w1", None, "   ");
        let all = st.list_history(None, 10);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0]["agentId"], "w2"); // most recent first
        let one = st.list_history(Some("w1"), 10);
        assert_eq!(one.len(), 1);
        // LIKE escaping: a literal % in the query is not a wildcard.
        st.add_history("w1", None, "report 100% done");
        let hits = st.search_history("100% ", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["text"], "report 100% done");
        std::fs::remove_dir_all(&dir).ok();
    }
}
