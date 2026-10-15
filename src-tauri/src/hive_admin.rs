//! Hive admin helpers — pure logic behind the `hive:*`/`workers:*`/`pty:*`/
//! `session:*` IPC surface in cmd_hive.rs (the commands index.ts registers
//! that don't fit the core Hive impl).
//!
//! Storage notes:
//!  - tasks.json is `{"tasks": [...]}` of FLAT cards (taskLedger.ts semantics:
//!    edit raw entries, never re-serialize a partial model). Add/delete write
//!    the whole ledger; per-card patches stay in hive.rs `update_task`.
//!  - `archived`/`onHold` live on the registry.json row — exactly where
//!    upstream stores them (routeMessage reads the row directly). A legacy
//!    `registry-extra.json` sidecar from an earlier port is still READ as a
//!    fallback but new writes go to the registry only.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

use crate::hive::Hive;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Unique-temp-file + rename (hive::atomic_write_file) — the ledger/registry
/// are never observed half-written by the router or the UI, and two writers
/// never share a temp name.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    Ok(crate::hive::atomic_write_file(path, bytes)?)
}

// ─── task ledger (taskLedger.ts add/delete) ─────────────────────────────────

/// Write the ledger file. The caller holds `hive.tasks_lock()` across its
/// read-modify-write and calls `tasks_written` AFTER releasing it (the git
/// commit must not run under the store lock).
fn write_tasks(hive: &Hive, doc: &Value) -> Result<()> {
    atomic_write(
        &hive.root.join("tasks.json"),
        serde_json::to_string_pretty(doc)?.as_bytes(),
    )
}

fn tasks_written(hive: &Hive, doc: &Value) {
    let n = doc["tasks"].as_array().map(|t| t.len()).unwrap_or(0);
    let _ = hive.log(&json!({ "kind": "tasks", "count": n }));
    hive.commit(&format!("hive: tasks ({n})"));
}

/// Short random task id — lowercase alnum (~8 chars), upstream's card-id
/// shape when the caller didn't pick one.
fn new_task_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

/// Append one card against the latest on-disk ledger. Assigns `id` (random,
/// 8 chars) and `createdAt` (ms epoch) when the caller left them out.
/// Idempotent by task id — a duplicate id is an error, same as upstream's
/// `addTask` returning `{ok:false}`.
pub fn add_task(hive: &Hive, fields: &Value) -> Result<Value> {
    let mut card = fields
        .as_object()
        .cloned()
        .context("task must be an object")?;
    let id = card
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(new_task_id);
    card.insert("id".into(), Value::String(id.clone()));
    if card.get("createdAt").is_none() {
        // HiveTask.createdAt is an ISO string upstream (hive.ts:127).
        card.insert("createdAt".into(), json!(chrono::Utc::now().to_rfc3339()));
    }

    let lock = hive.tasks_lock();
    let guard = crate::hive::lock_store(&lock);
    let mut doc = hive.read_tasks()?;
    let tasks = doc
        .get_mut("tasks")
        .and_then(|t| t.as_array_mut())
        .context("tasks.json is not an object")?;
    if tasks
        .iter()
        .any(|t| t.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
    {
        anyhow::bail!("task {id} already exists");
    }
    tasks.push(Value::Object(card));
    write_tasks(hive, &doc)?;
    drop(guard);
    tasks_written(hive, &doc);
    let out = doc["tasks"]
        .as_array()
        .and_then(|t| t.last().cloned())
        .unwrap_or(Value::Null);
    Ok(out)
}

/// Delete only the named card — the remaining list is the new membership
/// (mergeTaskLedger protects FIELDS, never card membership). 404-style error
/// when the id is absent.
pub fn delete_task(hive: &Hive, id: &str) -> Result<Value> {
    let lock = hive.tasks_lock();
    let guard = crate::hive::lock_store(&lock);
    let mut doc = hive.read_tasks()?;
    let tasks = doc
        .get_mut("tasks")
        .and_then(|t| t.as_array_mut())
        .context("tasks.json is not an object")?;
    let before = tasks.len();
    tasks.retain(|t| t.get("id").and_then(|v| v.as_str()) != Some(id));
    if tasks.len() == before {
        anyhow::bail!("no task {id}");
    }
    let remaining = Value::Array(tasks.clone());
    write_tasks(hive, &doc)?;
    drop(guard);
    tasks_written(hive, &doc);
    Ok(remaining)
}

// ─── registry patching (rename / role) ──────────────────────────────────────

/// Patch raw registry.json entries, preserving every key the typed
/// RegistryEntry doesn't model (single-writer file, read-modify-write — the
/// same rule taskLedger's patchTaskInLedger exists to enforce).
fn patch_registry_entry(hive: &Hive, id: &str, patch: &Map<String, Value>) -> Result<bool> {
    let path = hive.root.join("registry.json");
    // Same store lock as hive.rs' typed registry writers; a corrupt file is
    // an error, never an empty doc that overwrites the roster.
    let lock = hive.registry_lock();
    let _g = crate::hive::lock_store(&lock);
    let mut doc = crate::hive::read_json_strict(&path, json!({ "agents": {} }))?;
    let Some(agents) = doc.get_mut("agents") else {
        anyhow::bail!("registry.json is not an object");
    };
    // Upstream stores `agents` as a map keyed by id; tolerate the port's
    // original array shape for registries written before the migration.
    let obj = match agents {
        Value::Object(map) => map
            .iter_mut()
            .find(|(k, v)| k.as_str() == id || v.get("id").and_then(|v| v.as_str()) == Some(id))
            .map(|(_, v)| v),
        Value::Array(arr) => arr
            .iter_mut()
            .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id)),
        _ => anyhow::bail!("registry.json is not an object"),
    };
    let Some(obj) = obj.and_then(|e| e.as_object_mut()) else {
        return Ok(false);
    };
    for (k, v) in patch {
        obj.insert(k.clone(), v.clone());
    }
    atomic_write(&path, serde_json::to_string_pretty(&doc)?.as_bytes())?;
    Ok(true)
}

/// hive.renameAgent — change the display name; the durable id, agent dir,
/// session and mailboxes stay keyed by `id`. Upstream also patches fleet.json;
/// this port keeps no fleet cache (roster.rs is renderer-state), so registry
/// + log is the whole record.
pub fn rename_agent(hive: &Hive, id: &str, new_name: &str) -> Result<Value> {
    let next = new_name.trim();
    if next.is_empty() {
        anyhow::bail!("Name is required");
    }
    let reg = hive.registry();
    let Some(agent) = reg.iter().find(|a| a.id == id) else {
        anyhow::bail!("Agent not found");
    };
    if agent.name == next {
        return Ok(json!({ "ok": true, "name": next }));
    }
    let previous = agent.name.clone();
    let mut patch = Map::new();
    patch.insert("name".into(), Value::String(next.to_string()));
    patch_registry_entry(hive, id, &patch)?;
    let _ = hive.log(&json!({
        "kind": "rename", "agent": id, "previousName": previous, "name": next
    }));
    hive.commit(&format!("hive: rename {id}"));
    Ok(json!({ "ok": true, "name": next }))
}

/// hive.patchAgentRole — update the durable job string and refresh
/// identity.md so the floor and the hive read the same role (upstream
/// rewrites identity.md on a role change; the seeded format is
/// register_agent's).
pub fn patch_agent_role(hive: &Hive, id: &str, role: &str) -> Result<Value> {
    let next = role.trim();
    if next.is_empty() {
        anyhow::bail!("empty role");
    }
    let reg = hive.registry();
    let Some(agent) = reg.iter().find(|a| a.id == id) else {
        anyhow::bail!("unknown agent");
    };
    if agent.role == next {
        return Ok(json!({ "ok": true }));
    }
    let mut patch = Map::new();
    patch.insert("role".into(), Value::String(next.to_string()));
    patch.insert("lastSeen".into(), json!(now_ms()));
    patch_registry_entry(hive, id, &patch)?;
    // Same text register_agent writes on spawn (capabilities, god lines…) —
    // a private re-format here would drop them.
    let mut updated = agent.clone();
    updated.role = next.to_string();
    let _ = fs::write(
        hive.agent_dir(id).join("identity.md"),
        Hive::identity_text(&updated),
    );
    let _ = hive.log(&json!({ "kind": "role", "agent": id, "role": next }));
    hive.commit(&format!("hive: role {id}"));
    Ok(json!({ "ok": true }))
}

// ─── archived / hold (registry.json rows, upstream shape) ───────────────────
//
// Upstream stores `archived`/`onHold` directly on the registry entry
// (hive.ts:160-183) and routeMessage/fleet read them from there — so this
// port patches registry.json and keeps a read fallback on the legacy
// `registry-extra.json` sidecar written by earlier port versions.

fn extras_path(hive: &Hive) -> PathBuf {
    hive.root.join("registry-extra.json")
}

fn read_extras(hive: &Hive) -> Value {
    let text = fs::read_to_string(extras_path(hive)).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_else(|_| json!({ "agents": {} }))
}

/// The whole flags map — workers_list joins every entry against live PTYs.
/// Registry rows are authoritative; the legacy sidecar fills agents that
/// only have old-format flags.
pub fn all_extras(hive: &Hive) -> Map<String, Value> {
    let mut out: Map<String, Value> = read_extras(hive)
        .get("agents")
        .and_then(|a| a.as_object())
        .cloned()
        .unwrap_or_default();
    for a in hive.registry() {
        if a.archived || a.on_hold {
            out.insert(
                a.id.clone(),
                json!({ "archived": a.archived, "onHold": a.on_hold }),
            );
        }
    }
    out
}

/// Flip the archived flag (closing a tab archives — retained, NOT deleted; a
/// respawn clears it). Written to the registry row — where routeMessage reads
/// it — with the lastSeen bump upstream applies on every registry write.
pub fn set_archived(hive: &Hive, id: &str, archived: bool) -> Result<Value> {
    let Some(agent) = hive.registry().into_iter().find(|a| a.id == id) else {
        return Ok(json!({ "ok": true }));
    };
    if agent.archived == archived {
        return Ok(json!({ "ok": true }));
    }
    let mut patch = Map::new();
    patch.insert("archived".into(), json!(archived));
    patch.insert("lastSeen".into(), json!(now_ms()));
    patch_registry_entry(hive, id, &patch)?;
    let _ = hive.log(&json!({ "kind": "archive", "agent": id, "archived": archived }));
    hive.commit(&format!(
        "hive: {} {id}",
        if archived { "archive" } else { "unarchive" }
    ));
    Ok(json!({ "ok": true }))
}

impl Hive {
    /// Startup migration (upstream archiveOrphanedAgents, #57/#58): archive
    /// every `archived:false` entry with no live PTY — stale carry-overs from a
    /// session that quit/crashed without archiving. Run at boot BEFORE the
    /// renderer respawns anything; a respawn goes through ensure_agent, which
    /// clears the flag again. God and the assistant are never archived.
    /// Returns the ids archived.
    pub fn archive_orphaned_agents(
        &self,
        live_agent_ids: &std::collections::HashSet<String>,
    ) -> Vec<String> {
        let mut done = Vec::new();
        for a in self.registry() {
            if a.archived || a.is_god || a.is_assistant || live_agent_ids.contains(&a.id) {
                continue;
            }
            if set_archived(self, &a.id, true).is_ok() {
                done.push(a.id);
            }
        }
        done
    }
}

/// Put an agent on hold / take it off — upstream's setAgentHold: the human
/// claimed a 1:1, so auto-delivery and god's dispatch skip them.
pub fn set_hold(hive: &Hive, id: &str, hold: bool) -> Result<Value> {
    let Some(agent) = hive.registry().into_iter().find(|a| a.id == id) else {
        anyhow::bail!("Agent not found");
    };
    if agent.on_hold == hold {
        return Ok(json!({ "ok": true, "onHold": hold }));
    }
    let mut patch = Map::new();
    patch.insert("onHold".into(), json!(hold));
    patch.insert("lastSeen".into(), json!(now_ms()));
    patch_registry_entry(hive, id, &patch)?;
    patch_fleet_hold(hive, id, hold);
    let _ = hive.log(&json!({ "kind": "agent-hold", "agent": id, "onHold": hold }));
    Ok(json!({ "ok": true, "onHold": hold }))
}

/// Patch fleet.json's row for `id` in the same operation as the registry
/// write (upstream setAgentHold): god's roster is injected from that file on
/// its next prompt, and waiting up to 8s for the periodic snapshot means one
/// more dispatch can still land on someone the human has just claimed. The
/// fleet is a cache — a missing/unparseable file or absent row is ignored.
fn patch_fleet_hold(hive: &Hive, id: &str, hold: bool) {
    let path = hive.root.join("fleet.json");
    let Ok(text) = fs::read_to_string(&path) else {
        return;
    };
    let Ok(mut fleet) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    let Some(row) = fleet
        .get_mut("agents")
        .and_then(|a| a.as_array_mut())
        .and_then(|a| a.iter_mut().find(|r| r.get("id").and_then(|v| v.as_str()) == Some(id)))
        .and_then(|r| r.as_object_mut())
    else {
        return;
    };
    row.insert("onHold".into(), json!(hold));
    if let Ok(out) = serde_json::to_string_pretty(&fleet) {
        let _ = atomic_write(&path, out.as_bytes());
    }
}

// ─── read surface ───────────────────────────────────────────────────────────

/// hive:agentDirectory — the agent's hive folder listing (name/kind/size/
/// mtime), confined by fs_ops::safe_resolve through list_dir.
pub fn agent_directory(hive: &Hive, id: &str) -> Vec<Value> {
    crate::fs_ops::list_dir(&hive.agent_dir(id), "").unwrap_or_default()
}

/// hive:agentUsage — the cost-ledger fold plus the registry session_id the
/// caller needs to key a transcript sample (`st.usage.sample(cwd, sid)` is
/// layered on by the command, which owns the UsageReader).
pub fn agent_usage(hive: &Hive, id: &str) -> Value {
    let entry = hive.registry().into_iter().find(|a| a.id == id);
    let ledger = hive.usage_sample(id);
    json!({
        "agent_id": id,
        "cwd": entry.as_ref().map(|a| a.cwd.clone()),
        "session_id": entry.and_then(|a| a.session_id),
        "ledger": ledger,
    })
}

/// hive:agentContext — the context-gauge slot. Live membership is caller
/// supplied (the command knows the PTY set + st.context_usage); the helper
/// shapes the row and reports whether the id is even registered.
pub fn agent_context(hive: &Hive, id: &str, pty_live: bool) -> Value {
    let registered = hive.registry().iter().any(|a| a.id == id);
    json!({
        "agent_id": id,
        "live": pty_live,
        "registered": registered,
    })
}

/// Last `max_lines` non-blank lines of `path`, oldest→newest, read from the
/// end in chunks so a multi-MB log is never loaded whole. A missing file is
/// an empty tail.
fn tail_lines(path: &std::path::Path, max_lines: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    const CHUNK: u64 = 64 * 1024;
    let Ok(mut f) = fs::File::open(path) else {
        return Vec::new();
    };
    let Ok(len) = f.metadata().map(|m| m.len()) else {
        return Vec::new();
    };
    let mut buf: Vec<u8> = Vec::new();
    let mut pos = len;
    // Need max_lines complete lines: stop once the buffer holds more than
    // max_lines newlines (the oldest, possibly partial, line is discarded).
    while pos > 0 && buf.iter().filter(|&&b| b == b'\n').count() <= max_lines {
        let take = CHUNK.min(pos);
        pos -= take;
        let mut chunk = vec![0u8; take as usize];
        if f.seek(SeekFrom::Start(pos)).is_err() || f.read_exact(&mut chunk).is_err() {
            break;
        }
        chunk.extend_from_slice(&buf);
        buf = chunk;
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.lines().collect();
    if pos > 0 && !lines.is_empty() {
        lines.remove(0); // starts mid-line
    }
    let lines: Vec<String> = lines
        .into_iter()
        .filter(|l| !l.trim().is_empty())
        .map(String::from)
        .collect();
    let skip = lines.len().saturating_sub(max_lines);
    lines.into_iter().skip(skip).collect()
}

/// hive:log — upstream logTail: the last `limit` events in FILE order,
/// oldest→newest; unparseable lines are preserved as `{"raw": <line>}` so a
/// corrupt tail never silently disappears. `limit` is capped at 500. When the
/// live log was just rotated and holds fewer lines than asked for, the tail
/// continues into `log.jsonl.1`.
pub fn hive_log(hive: &Hive, limit: usize) -> Vec<Value> {
    let cap = limit.min(500);
    let live = hive.root.join("log.jsonl");
    let mut lines = tail_lines(&live, cap);
    if lines.len() < cap {
        let mut prev = live.into_os_string();
        prev.push(".1");
        let mut older = tail_lines(std::path::Path::new(&prev), cap - lines.len());
        older.append(&mut lines);
        lines = older;
    }
    lines
        .iter()
        .map(|l| serde_json::from_str::<Value>(l.trim()).unwrap_or_else(|_| json!({ "raw": l })))
        .collect()
}

/// hive:messages — upstream `voiceMessages` (hive.ts:2015): floor-wide or
/// per-agent mail in the VoiceMessage shape — inbox + outbox plus the
/// .done/.sent archives, deduped by id, subject+body run through
/// redactSecrets (the "renderer never sees a secret" gate). `id` narrows to a
/// single message lookup; `include_archived` defaults true upstream.
pub fn voice_messages(
    hive: &Hive,
    agent_id: Option<&str>,
    id: Option<&str>,
    limit: Option<usize>,
    include_archived: Option<bool>,
) -> Vec<Value> {
    let agents_dir = hive.root.join("agents");
    let only_agent = agent_id.map(|s| s.trim()).filter(|s| !s.is_empty());
    let want_id = id.map(|s| s.trim()).filter(|s| !s.is_empty());
    let include_archived = include_archived.unwrap_or(true);

    let owners: Vec<String> = match only_agent {
        Some(a) => vec![a.to_string()],
        None => fs::read_dir(&agents_dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|n| !n.starts_with('.'))
                    .collect()
            })
            .unwrap_or_default(),
    };

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<Value> = Vec::new();
    for owner in owners {
        let base = hive.agent_dir(&owner);
        let mut folders = vec![
            (base.join("inbox"), "inbox", false),
            (base.join("outbox"), "outbox", false),
        ];
        if include_archived {
            folders.push((base.join("inbox").join(".done"), "inbox", true));
            folders.push((base.join("outbox").join(".sent"), "outbox", true));
        }
        for (dir, direction, archived) in folders {
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            let mut files: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .collect();
            files.sort();
            for f in files {
                let Ok(t) = fs::read_to_string(&f) else {
                    continue;
                };
                let Ok(m) = serde_json::from_str::<Value>(&t) else {
                    continue;
                };
                let Some(mid) = m.get("id").and_then(|v| v.as_str()) else {
                    continue;
                };
                if !seen.insert(mid.to_string()) {
                    continue;
                }
                if let Some(w) = want_id {
                    if mid != w {
                        continue;
                    }
                }
                out.push(json!({
                    "id": mid,
                    "conversation": m.get("conversation"),
                    "from": m.get("from"),
                    "to": m.get("to"),
                    "act": m.get("act"),
                    "subject": crate::hive::redact_secrets(
                        m.get("subject").and_then(|v| v.as_str()).unwrap_or("")
                    ),
                    "body": crate::hive::redact_secrets(
                        m.get("body").and_then(|v| v.as_str()).unwrap_or("")
                    ),
                    "requires_reply": m.get("requires_reply")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    "direction": direction,
                    "owner": owner,
                    "archived": archived,
                    "created_at": m.get("created_at"),
                }));
            }
        }
    }
    out.sort_by(|a, b| {
        let ka = a.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        let kb = b.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        kb.cmp(ka)
    });
    if want_id.is_some() {
        out.truncate(1);
        return out;
    }
    // Upstream clamps the default to 12, the caller limit to [1,40].
    let lim = limit.map(|l| l.clamp(1, 40)).unwrap_or(12);
    out.truncate(lim);
    out
}

/// Case-insensitive substring search that reports BYTE offsets into the
/// ORIGINAL `hay` (`(start, end)` on char boundaries). Lowercasing the whole
/// line first (`hay.to_lowercase().find(..)`) shifts offsets whenever a char's
/// lowercase form has a different UTF-8 length, so the match is walked
/// char-by-char on `hay` itself instead. `needle` is already lowercase.
fn find_ci(hay: &str, needle: &[char]) -> Option<(usize, usize)> {
    for (start, _) in hay.char_indices() {
        let mut ni = 0;
        let mut end = start;
        for (off, c) in hay[start..].char_indices() {
            if ni == needle.len() {
                break;
            }
            let mut ok = true;
            for lc in c.to_lowercase() {
                if ni < needle.len() && needle[ni] == lc {
                    ni += 1;
                } else {
                    ok = false;
                    break;
                }
            }
            if !ok {
                ni = usize::MAX;
                break;
            }
            end = start + off + c.len_utf8();
        }
        if ni == needle.len() {
            return Some((start, end));
        }
    }
    None
}

/// hive:textSearch — literal, case-insensitive substring search across the
/// text files under hive/ (*.md, *.txt, *.json). Skips .git, anything
/// non-UTF-8/binary, and files over 256KB. Rows are {path, line, text}
/// (path relative to the hive root, line 1-based), capped at 100.
/// hive:textSearch (index.ts:4025) — board.md, tasks.json, and each agent's
/// memory.md, max 3 hits per file, ~40 chars of context either side of the
/// match. NOT a tree walk: inbox/outbox/registry are deliberately out of
/// scope (the memory-search surface is a curated set).
pub fn text_search(hive: &Hive, query: &str, limit: usize) -> Vec<Value> {
    const MAX_ROWS: usize = 100;
    let needle: Vec<char> = query.trim().to_lowercase().chars().collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let cap = limit.min(MAX_ROWS);
    let mut targets: Vec<(PathBuf, String)> = vec![
        (hive.root.join("board.md"), "board.md".into()),
        (hive.root.join("tasks.json"), "tasks.json".into()),
    ];
    let agents_dir = hive.root.join("agents");
    if let Ok(rd) = fs::read_dir(&agents_dir) {
        for e in rd.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                let id = e.file_name().to_string_lossy().to_string();
                targets.push((e.path().join("memory.md"), format!("{id}/memory.md")));
            }
        }
    }
    let mut out = Vec::new();
    for (path, source) in targets {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let mut hits = 0;
        for line in text.lines() {
            if hits >= 3 {
                break;
            }
            let Some((m_start, m_end)) = find_ci(line, &needle) else {
                continue;
            };
            // ~40 bytes of context either side, snapped to char boundaries —
            // multibyte text (em-dash, CJK, emoji) must never be sliced mid-char.
            let mut start = m_start.saturating_sub(40);
            while !line.is_char_boundary(start) {
                start -= 1;
            }
            let mut end = (m_end + 40).min(line.len());
            while !line.is_char_boundary(end) {
                end += 1;
            }
            let excerpt = line[start..end].trim();
            out.push(json!({ "source": source, "excerpt": excerpt }));
            hits += 1;
            if out.len() >= cap {
                return out;
            }
        }
    }
    out
}

/// session:resolveCwd — the cwd a respawn/resume should boot in: the
/// registry's recorded cwd, falling back to the agent's hive folder (its
/// always-existing workspace).
pub fn resolve_cwd(hive: &Hive, agent_id: &str) -> Value {
    let cwd = hive
        .registry()
        .into_iter()
        .find(|a| a.id == agent_id)
        .map(|a| a.cwd)
        .filter(|c| !c.trim().is_empty());
    match cwd {
        Some(c) => Value::String(c),
        None => Value::String(hive.agent_dir(agent_id).display().to_string()),
    }
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hive::RegistryEntry;

    fn tmp_hive(tag: &str) -> (PathBuf, Hive) {
        let root =
            std::env::temp_dir().join(format!("oa-hiveadmin-{tag}-{}", uuid::Uuid::new_v4()));
        let hive = Hive::ensure(&root).unwrap();
        (root, hive)
    }

    fn entry(id: &str, name: &str) -> RegistryEntry {
        RegistryEntry {
            id: id.into(),
            name: name.into(),
            provider: "claude".into(),
            role: "worker".into(),
            cwd: "D:\\work".into(),
            is_god: false,
            status: "idle".into(),
            session_id: None,
            command: "claude".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            ..Default::default()
        }
    }

    #[test]
    fn add_and_delete_task() {
        let (_r, hive) = tmp_hive("tasks");
        let card = add_task(&hive, &json!({ "title": "ship it" })).unwrap();
        let id = card["id"].as_str().unwrap();
        assert_eq!(id.len(), 8);
        assert!(id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() && !c.is_ascii_uppercase()));
        // createdAt is an ISO string upstream (HiveTask.createdAt: string).
        assert!(card["createdAt"].as_str().unwrap().contains('T'));

        // A caller-supplied id is kept; a duplicate id fails (addTask {ok:false}).
        let named = add_task(&hive, &json!({ "id": "abc12345", "title": "x" })).unwrap();
        assert_eq!(named["id"], "abc12345");
        assert!(add_task(&hive, &json!({ "id": "abc12345" })).is_err());

        let remaining = delete_task(&hive, id).unwrap();
        let ids: Vec<&str> = remaining
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["abc12345"]);
        assert!(delete_task(&hive, "nope").is_err());
    }

    #[test]
    fn rename_and_archive_round_trip() {
        let (_r, hive) = tmp_hive("reg");
        hive.register_agent(&entry("w-1", "Ada")).unwrap();

        rename_agent(&hive, "w-1", "Ada Lovelace").unwrap();
        let a = hive.registry().into_iter().find(|a| a.id == "w-1").unwrap();
        assert_eq!(a.name, "Ada Lovelace");
        assert!(rename_agent(&hive, "w-1", "   ").is_err());
        assert!(rename_agent(&hive, "ghost", "X").is_err());

        // The flag lives ON THE REGISTRY ROW (upstream shape) — it must also
        // survive a typed registry write (set_status rewrites registry.json
        // wholesale through the same RegistryEntry serde).
        set_archived(&hive, "w-1", true).unwrap();
        hive.set_status("w-1", "working").unwrap();
        let a = hive.registry().into_iter().find(|a| a.id == "w-1").unwrap();
        assert!(a.archived, "archived must outlive a registry rewrite");
        // Routing honors it: broadcast skips archived agents.
        let mut msg = crate::hive::Message::new("god", "broadcast", "inform", "x", "y");
        msg.from = "god".into();
        let inbox_w1 = hive.agent_dir("w-1").join("inbox");
        let before = fs::read_dir(&inbox_w1).unwrap().count();
        hive.send_routed(&msg).unwrap();
        assert_eq!(fs::read_dir(&inbox_w1).unwrap().count(), before);
        set_archived(&hive, "w-1", false).unwrap();
        assert!(
            !hive
                .registry()
                .into_iter()
                .find(|a| a.id == "w-1")
                .unwrap()
                .archived
        );

        set_hold(&hive, "w-1", true).unwrap();
        assert!(
            hive.registry()
                .into_iter()
                .find(|a| a.id == "w-1")
                .unwrap()
                .on_hold
        );
        assert!(set_hold(&hive, "ghost", true).is_err());
    }

    #[test]
    fn hold_toggle_patches_fleet_json_immediately() {
        let (_r, hive) = tmp_hive("hold-fleet");
        hive.register_agent(&entry("w-h", "Hal")).unwrap();
        hive.write_fleet_snapshot(&json!({
            "ts": 1,
            "agents": [{ "id": "w-h", "onHold": false }, { "id": "w-other", "onHold": false }]
        }))
        .unwrap();
        set_hold(&hive, "w-h", true).unwrap();
        let fleet: Value =
            serde_json::from_str(&fs::read_to_string(hive.root.join("fleet.json")).unwrap()).unwrap();
        assert_eq!(fleet["agents"][0]["onHold"], json!(true));
        assert_eq!(fleet["agents"][1]["onHold"], json!(false));
        // The hook-health field rides along on every fleet write.
        assert!(fleet.get("hooks").is_some());
        // No fleet file (or no row) is not an error.
        let (_r2, bare) = tmp_hive("hold-nofleet");
        bare.register_agent(&entry("w-h", "Hal")).unwrap();
        set_hold(&bare, "w-h", true).unwrap();
    }

    #[test]
    fn archive_orphans_skips_god_assistant_and_live() {
        let (_r, hive) = tmp_hive("orphans");
        let mut god = entry("god-1", "Michael");
        god.is_god = true;
        let mut asst = entry("asst-1", "Dwight");
        asst.is_assistant = true;
        for e in [god, asst, entry("live-1", "Jim"), entry("dead-1", "Pam")] {
            hive.register_agent(&e).unwrap();
        }
        let live: std::collections::HashSet<String> = ["live-1".to_string()].into();
        assert_eq!(hive.archive_orphaned_agents(&live), vec!["dead-1".to_string()]);
        let by = |id: &str| hive.registry().into_iter().find(|a| a.id == id).unwrap();
        assert!(by("dead-1").archived);
        assert!(!by("god-1").archived && !by("asst-1").archived && !by("live-1").archived);
        // Idempotent.
        assert!(hive.archive_orphaned_agents(&live).is_empty());
    }

    #[test]
    fn role_patch_rewrites_identity() {
        let (_r, hive) = tmp_hive("role");
        hive.register_agent(&entry("w-2", "Bob")).unwrap();
        patch_agent_role(&hive, "w-2", "reviewer").unwrap();
        let a = hive.registry().into_iter().find(|a| a.id == "w-2").unwrap();
        assert_eq!(a.role, "reviewer");
        let ident = fs::read_to_string(hive.agent_dir("w-2").join("identity.md")).unwrap();
        // identity_text's format (the one register_agent writes) — Role line
        // plus the capabilities/cwd lines a private re-format used to drop.
        assert!(ident.contains("- Role: reviewer"), "{ident}");
        assert!(ident.contains("- Capabilities:"), "{ident}");
        assert!(ident.contains("- Working directory:"), "{ident}");
        assert!(patch_agent_role(&hive, "w-2", "  ").is_err());
    }

    #[test]
    fn log_tail_spans_a_rotation_and_reads_big_files_from_the_end() {
        let (_r, hive) = tmp_hive("log-rot");
        // > one 64 KB chunk of lines, so the backwards read has to loop.
        let pad = "x".repeat(200);
        for i in 0..1000 {
            hive.log(&json!({ "event": "t", "i": i, "pad": pad })).unwrap();
        }
        let tail = hive_log(&hive, 500);
        assert_eq!(tail.len(), 500);
        assert_eq!(tail[0]["i"], json!(500));
        assert_eq!(tail[499]["i"], json!(999));
        // Rotate: the live log is empty, the tail continues into log.jsonl.1.
        let live = hive.root.join("log.jsonl");
        assert!(crate::hive::rotate_if_larger(&live, 10));
        assert!(hive.root.join("log.jsonl.1").exists());
        hive.log(&json!({ "event": "after", "i": 1000 })).unwrap();
        let tail = hive_log(&hive, 3);
        assert_eq!(tail.iter().map(|v| v["i"].as_u64().unwrap()).collect::<Vec<_>>(), vec![998, 999, 1000]);
        assert_eq!(hive_log(&hive, 500).len(), 500);
    }

    #[test]
    fn log_tail_is_oldest_first_with_raw() {
        let (_r, hive) = tmp_hive("log");
        for i in 0..10 {
            hive.log(&json!({ "event": "t", "i": i })).unwrap();
        }
        // Upstream logTail: file order (oldest→newest), the LAST `limit`.
        let tail = hive_log(&hive, 3);
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[0]["i"], json!(7));
        assert_eq!(tail[2]["i"], json!(9));
        // Malformed lines survive as {raw} instead of vanishing.
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(hive.root.join("log.jsonl"))
            .unwrap();
        use std::io::Write;
        writeln!(f, "not-json{{").unwrap();
        let tail = hive_log(&hive, 1);
        assert_eq!(tail[0], json!({ "raw": "not-json{" }));
        assert!(hive_log(&hive, 999).len() >= 10); // capped at 500, not 999
    }

    #[test]
    fn text_search_finds_workspace_lines() {
        let (_r, hive) = tmp_hive("search");
        let dir = hive.agent_dir("w-3");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("memory.md"),
            "nothing here\nThe QUICK brown fox\nbye\n",
        )
        .unwrap();
        let hits = text_search(&hive, "quick", 100);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["source"], json!("w-3/memory.md"));
        assert_eq!(hits[0]["excerpt"], json!("The QUICK brown fox"));
        assert!(text_search(&hive, "absent-token", 100).is_empty());
        assert!(text_search(&hive, "  ", 100).is_empty());
    }

    #[test]
    fn text_search_is_char_boundary_safe_on_multibyte() {
        let (_r, hive) = tmp_hive("search-mb");
        let dir = hive.agent_dir("w-9");
        fs::create_dir_all(&dir).unwrap();
        // Em-dash, CJK and emoji packed around the match so BOTH the 40-byte
        // left and right padding land mid-char; 'İ' lowercases to 2 chars,
        // which used to shift to_lowercase() offsets against the original.
        let pad = "—漢字😀".repeat(12);
        let line = format!("İİİ {pad} NEEDLE {pad} end");
        fs::write(dir.join("memory.md"), format!("{line}\n")).unwrap();
        let hits = text_search(&hive, "needle", 100);
        assert_eq!(hits.len(), 1);
        let ex = hits[0]["excerpt"].as_str().unwrap();
        assert!(ex.contains("NEEDLE"), "{ex}");
        // The match itself may be multibyte too.
        fs::write(dir.join("memory.md"), format!("{pad}漢字😀{pad}\n")).unwrap();
        let hits = text_search(&hive, "漢字😀", 100);
        assert!(!hits.is_empty());
        // find_ci reports offsets into the ORIGINAL text.
        let hay = "İİ abc";
        let (s0, e0) = find_ci(hay, &['a', 'b', 'c']).unwrap();
        assert_eq!(&hay[s0..e0], "abc");
    }

    #[test]
    fn corrupt_stores_are_errors_not_overwritten() {
        let (_r, hive) = tmp_hive("corrupt");
        hive.register_agent(&entry("w-7", "Gus")).unwrap();
        fs::write(hive.root.join("registry.json"), "{ not json").unwrap();
        assert!(hive.set_status("w-7", "working").is_err());
        // The corrupt file is untouched and a .bak copy exists.
        assert_eq!(
            fs::read_to_string(hive.root.join("registry.json")).unwrap(),
            "{ not json"
        );
        assert!(hive.root.join("registry.json.bak").exists());
        fs::write(hive.root.join("tasks.json"), "garbage").unwrap();
        assert!(hive.read_tasks().is_err());
        assert!(add_task(&hive, &json!({ "title": "x" })).is_err());
        assert_eq!(
            fs::read_to_string(hive.root.join("tasks.json")).unwrap(),
            "garbage"
        );
    }

    #[test]
    fn concurrent_task_adds_all_land() {
        let (_r, hive) = tmp_hive("conc");
        let hive = std::sync::Arc::new(hive);
        let hs: Vec<_> = (0..8)
            .map(|i| {
                let h = hive.clone();
                std::thread::spawn(move || {
                    add_task(&h, &json!({ "id": format!("t{i}"), "title": "x" })).unwrap();
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(hive.read_tasks().unwrap()["tasks"].as_array().unwrap().len(), 8);
    }

    #[test]
    fn messages_lists_inbox_and_done() {
        let (_r, hive) = tmp_hive("msgs");
        hive.register_agent(&entry("w-4", "Cid")).unwrap();
        let m1 = crate::hive::Message::new("god", "w-4", "inform", "s1", "b1");
        let m2 = crate::hive::Message::new("god", "w-4", "request", "s2", "b2");
        hive.send_virtual(&m1.from, "w-4", &m1.act, &m1.subject, &m1.body)
            .unwrap();
        hive.send_virtual(&m2.from, "w-4", &m2.act, &m2.subject, &m2.body)
            .unwrap();
        let inbox_id = hive.inbox("w-4").unwrap()[0].id.clone();
        hive.mark_done("w-4", &inbox_id).unwrap();
        let msgs = voice_messages(&hive, Some("w-4"), None, Some(50), None);
        assert_eq!(msgs.len(), 2);
        // VoiceMessage shape: direction+owner+archived, not a `done` flag —
        // one pending inbox, one filed .done.
        assert_eq!(msgs.iter().filter(|m| m["archived"] == true).count(), 1);
        assert_eq!(msgs.iter().filter(|m| m["archived"] == false).count(), 1);
        assert!(msgs.iter().all(|m| m["direction"] == "inbox"));
        assert!(msgs.iter().all(|m| m["owner"] == "w-4"));
    }

    #[test]
    fn voice_messages_redacts_and_dedupes() {
        let (_r, hive) = tmp_hive("vmsg");
        hive.register_agent(&entry("w-6", "Eve")).unwrap();
        // A message whose body carries a credential — renderer-facing reads
        // must never return the secret (upstream redactSecrets gate).
        let mut m = crate::hive::Message::new(
            "god",
            "w-6",
            "inform",
            "s",
            "here's the key sk-ant-1234567890abcdef123",
        );
        m.conversation = "cv".into();
        hive.send_routed(&m).unwrap();
        // Same id also written to outbox — dedup by id must collapse it.
        hive.send(&m).unwrap();
        let msgs = voice_messages(&hive, Some("w-6"), None, Some(40), None);
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0]["body"].as_str().unwrap().contains("[redacted]"));
        assert!(!msgs[0]["body"].as_str().unwrap().contains("sk-ant-"));
        // The whole-floor call (no agentId) sees the same message once.
        let all = voice_messages(&hive, None, None, Some(40), None);
        assert_eq!(all.iter().filter(|r| r["id"] == m.id).count(), 1);
        // id lookup narrows to the single message.
        let one = voice_messages(&hive, None, Some(&m.id), None, None);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0]["id"], m.id);
    }

    #[test]
    fn resolve_cwd_prefers_registry() {
        let (_r, hive) = tmp_hive("cwd");
        hive.register_agent(&entry("w-5", "Dee")).unwrap();
        assert_eq!(resolve_cwd(&hive, "w-5"), json!("D:\\work"));
        // Unknown agent falls back to its (would-be) hive folder.
        let v = resolve_cwd(&hive, "ghost");
        assert!(v.as_str().unwrap().ends_with("ghost"));
    }
}
