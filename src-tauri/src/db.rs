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

    /// Drop the connection (upstream persist.close). app_reset_all calls this
    /// before deleting harness.db — on Windows the file stays locked while a
    /// connection holds it, so the delete fails without a close first.
    pub fn close(&mut self) {
        self.conn = None;
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

// ─── window geometry (kv key `window.bounds`) ───────────────────────────────

/// kv key for the persisted main-window geometry.
pub const WINDOW_BOUNDS_KEY: &str = "window.bounds";

/// Persisted/restored main-window rect in PHYSICAL pixels. `x`/`y` are absent
/// when the saved position is unusable (an unplugged monitor) — the caller
/// then restores the size only and centers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowBounds {
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub width: u32,
    pub height: u32,
}

/// Validate + clamp saved bounds (upstream clampBounds). `areas` are the
/// connected monitors' work areas as `(x, y, w, h)` in physical pixels. The
/// size is raised to `min` and capped at the chosen monitor; the position is
/// kept only when the window meaningfully overlaps a monitor (a window parked
/// on an unplugged display, or at the minimized sentinel (-32000,-32000), must
/// not reopen off-screen) and is then shifted fully inside that monitor.
/// None for unusable input.
pub fn clamp_window_bounds(
    raw: &Value,
    areas: &[(i32, i32, u32, u32)],
    min: (u32, u32),
) -> Option<WindowBounds> {
    let num = |k: &str| raw.get(k).and_then(|v| v.as_f64()).filter(|n| n.is_finite());
    let (w, h) = (num("width")?, num("height")?);
    if w < 1.0 || h < 1.0 {
        return None;
    }
    let mut width = (w.round() as u32).max(min.0);
    let mut height = (h.round() as u32).max(min.1);
    let (Some(x), Some(y)) = (num("x"), num("y")) else {
        return Some(WindowBounds { x: None, y: None, width, height });
    };
    let (x, y) = (x.round() as i64, y.round() as i64);
    // Largest-overlap monitor; require a reachable chunk of the window on it.
    let overlap = |a: &(i32, i32, u32, u32)| -> i64 {
        let ox = (x + width as i64).min(a.0 as i64 + a.2 as i64) - x.max(a.0 as i64);
        let oy = (y + height as i64).min(a.1 as i64 + a.3 as i64) - y.max(a.1 as i64);
        if ox > 0 && oy > 0 {
            ox * oy
        } else {
            0
        }
    };
    let best = areas
        .iter()
        .filter(|a| {
            // At least ~100x100 of the window must be on it.
            let ox = (x + width as i64).min(a.0 as i64 + a.2 as i64) - x.max(a.0 as i64);
            let oy = (y + height as i64).min(a.1 as i64 + a.3 as i64) - y.max(a.1 as i64);
            ox >= 100 && oy >= 100
        })
        .max_by_key(|a| overlap(a));
    let Some(&(ax, ay, aw, ah)) = best else {
        return Some(WindowBounds { x: None, y: None, width, height });
    };
    width = width.min(aw.max(min.0));
    height = height.min(ah.max(min.1));
    let nx = x.clamp(ax as i64, (ax as i64 + aw as i64 - width as i64).max(ax as i64));
    let ny = y.clamp(ay as i64, (ay as i64 + ah as i64 - height as i64).max(ay as i64));
    Some(WindowBounds {
        x: Some(nx as i32),
        y: Some(ny as i32),
        width,
        height,
    })
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
    fn window_bounds_clamp_to_a_visible_monitor() {
        let one = [(0, 0, 1920u32, 1040u32)];
        let two = [(0, 0, 1920u32, 1040u32), (1920, 0, 1920u32, 1040u32)];
        let min = (960, 600);
        let b = |v: Value, areas: &[(i32, i32, u32, u32)]| clamp_window_bounds(&v, areas, min);
        // In-bounds rect is restored verbatim.
        assert_eq!(
            b(json!({"x": 100, "y": 50, "width": 1400, "height": 900}), &one),
            Some(WindowBounds { x: Some(100), y: Some(50), width: 1400, height: 900 })
        );
        // Below-minimum size is raised.
        assert_eq!(
            b(json!({"x": 0, "y": 0, "width": 300, "height": 200}), &one),
            Some(WindowBounds { x: Some(0), y: Some(0), width: 960, height: 600 })
        );
        // Parked on a now-unplugged second monitor -> position dropped.
        assert_eq!(
            b(json!({"x": 2500, "y": 100, "width": 1200, "height": 800}), &one),
            Some(WindowBounds { x: None, y: None, width: 1200, height: 800 })
        );
        // ...but kept when that monitor is still there.
        assert_eq!(
            b(json!({"x": 2500, "y": 100, "width": 1200, "height": 800}), &two)
                .and_then(|r| r.x),
            Some(2500)
        );
        // The minimized sentinel never restores off-screen.
        assert_eq!(
            b(json!({"x": -32000, "y": -32000, "width": 1200, "height": 800}), &one)
                .and_then(|r| r.x),
            None
        );
        // A window hanging off the edge is shifted fully inside; oversize is capped.
        assert_eq!(
            b(json!({"x": 1000, "y": 500, "width": 1400, "height": 900}), &one),
            Some(WindowBounds { x: Some(520), y: Some(140), width: 1400, height: 900 })
        );
        assert_eq!(
            b(json!({"x": 0, "y": 0, "width": 5000, "height": 4000}), &one),
            Some(WindowBounds { x: Some(0), y: Some(0), width: 1920, height: 1040 })
        );
        // Size only (no position) and garbage.
        assert_eq!(
            b(json!({"width": 1300, "height": 850}), &one),
            Some(WindowBounds { x: None, y: None, width: 1300, height: 850 })
        );
        assert_eq!(b(json!({"w": 1200}), &one), None);
        assert_eq!(b(json!("nope"), &one), None);
        // No monitor info -> size only.
        assert_eq!(
            b(json!({"x": 5, "y": 5, "width": 1300, "height": 850}), &[]).and_then(|r| r.x),
            None
        );
    }

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
