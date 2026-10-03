//! Hive admin helpers — pure logic behind the `hive:*`/`workers:*`/`pty:*`/
//! `session:*` IPC surface in cmd_hive.rs (the commands index.ts registers
//! that don't fit the core Hive impl).
//!
//! Storage notes:
//!  - tasks.json is `{"tasks": [...]}` of FLAT cards (taskLedger.ts semantics:
//!    edit raw entries, never re-serialize a partial model). Add/delete write
//!    the whole ledger; per-card patches stay in hive.rs `update_task`.
//!  - RegistryEntry has no archived/hold fields and no serde flatten, and
//!    `write_registry` re-serializes the typed struct — extra keys on an entry
//!    would be DROPPED on the next status/session write. Upstream stores
//!    `archived`/`onHold` on the registry row; we keep them in a sidecar
//!    `registry-extra.json` (`{"agents": {"<id>": {"archived": b, "onHold": b}}}`)
//!    that no typed write can clobber. `archived` hides an agent from floor
//!    lists; `onHold` marks paused-delivery (a 1:1 the human claimed).

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

/// Same temp-file + rename contract as hive's private atomic_write — the
/// ledger/registry are never observed half-written by the router or the UI.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

// ─── task ledger (taskLedger.ts add/delete) ─────────────────────────────────

fn write_tasks(hive: &Hive, doc: &Value) -> Result<()> {
    atomic_write(
        &hive.root.join("tasks.json"),
        serde_json::to_string_pretty(doc)?.as_bytes(),
    )?;
    let n = doc["tasks"].as_array().map(|t| t.len()).unwrap_or(0);
    let _ = hive.log(&json!({ "event": "tasks", "count": n }));
    hive.commit(&format!("hive: tasks ({n})"));
    Ok(())
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
        card.insert("createdAt".into(), json!(now_ms()));
    }

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
    Ok(remaining)
}

// ─── registry patching (rename / role) ──────────────────────────────────────

/// Patch raw registry.json entries, preserving every key the typed
/// RegistryEntry doesn't model (single-writer file, read-modify-write — the
/// same rule taskLedger's patchTaskInLedger exists to enforce).
fn patch_registry_entry(hive: &Hive, id: &str, patch: &Map<String, Value>) -> Result<bool> {
    let path = hive.root.join("registry.json");
    let text = fs::read_to_string(&path).unwrap_or_default();
    let mut doc: Value =
        serde_json::from_str(&text).unwrap_or_else(|_| json!({ "agents": [] }));
    let Some(agents) = doc.get_mut("agents").and_then(|a| a.as_array_mut()) else {
        anyhow::bail!("registry.json is not an object");
    };
    let Some(entry) = agents
        .iter_mut()
        .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id))
    else {
        return Ok(false);
    };
    let Some(obj) = entry.as_object_mut() else {
        anyhow::bail!("agent {id} is not an object");
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
        "event": "rename", "agent": id, "previousName": previous, "name": next
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
    patch_registry_entry(hive, id, &patch)?;
    let identity = format!(
        "# {}\n\n- id: `{}`\n- provider: `{}`\n- role: {}\n- cwd: `{}`\n",
        agent.name, agent.id, agent.provider, next, agent.cwd
    );
    let _ = fs::write(hive.agent_dir(id).join("identity.md"), identity);
    let _ = hive.log(&json!({ "event": "role", "agent": id, "role": next }));
    hive.commit(&format!("hive: role {id}"));
    Ok(json!({ "ok": true }))
}

// ─── archived / hold (sidecar registry-extra.json) ──────────────────────────

fn extras_path(hive: &Hive) -> PathBuf {
    hive.root.join("registry-extra.json")
}

fn read_extras(hive: &Hive) -> Value {
    let text = fs::read_to_string(extras_path(hive)).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_else(|_| json!({ "agents": {} }))
}

fn write_extras(hive: &Hive, doc: &Value) -> Result<()> {
    atomic_write(
        &extras_path(hive),
        serde_json::to_string_pretty(doc)?.as_bytes(),
    )
}

/// One agent's sidecar flags (`{"archived": bool, "onHold": bool}`) — absent
/// keys read as their falsy defaults, same as upstream's `!!a.archived`.
pub fn agent_extra(hive: &Hive, id: &str) -> Value {
    read_extras(hive)
        .get("agents")
        .and_then(|a| a.get(id))
        .cloned()
        .unwrap_or_else(|| json!({}))
}

/// The whole sidecar map — workers_list joins every entry against live PTYs.
pub fn all_extras(hive: &Hive) -> Map<String, Value> {
    read_extras(hive)
        .get("agents")
        .and_then(|a| a.as_object())
        .cloned()
        .unwrap_or_default()
}

fn set_extra(hive: &Hive, id: &str, key: &str, on: bool) -> Result<()> {
    let mut doc = read_extras(hive);
    if doc.get("agents").and_then(|a| a.as_object()).is_none() {
        doc["agents"] = json!({});
    }
    doc["agents"][id][key] = json!(on);
    write_extras(hive, &doc)
}

/// Flip the archived flag (closing a tab archives — retained, NOT deleted; a
/// respawn clears it). Best-effort no-op on an unknown id or an unchanged
/// flag, exactly like upstream setArchived.
pub fn set_archived(hive: &Hive, id: &str, archived: bool) -> Result<Value> {
    let known = hive.registry().iter().any(|a| a.id == id);
    if !known {
        return Ok(json!({ "ok": true }));
    }
    let cur = agent_extra(hive, id)
        .get("archived")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if cur == archived {
        return Ok(json!({ "ok": true }));
    }
    set_extra(hive, id, "archived", archived)?;
    let _ = hive.log(&json!({ "event": "archive", "agent": id, "archived": archived }));
    hive.commit(&format!("hive: {} {id}", if archived { "archive" } else { "unarchive" }));
    Ok(json!({ "ok": true }))
}

/// Put an agent on hold / take it off — upstream's setAgentHold: the human
/// claimed a 1:1, so auto-delivery and god's dispatch skip them.
pub fn set_hold(hive: &Hive, id: &str, hold: bool) -> Result<Value> {
    let known = hive.registry().iter().any(|a| a.id == id);
    if !known {
        anyhow::bail!("Agent not found");
    }
    let cur = agent_extra(hive, id)
        .get("onHold")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if cur == hold {
        return Ok(json!({ "ok": true, "onHold": hold }));
    }
    set_extra(hive, id, "onHold", hold)?;
    let _ = hive.log(&json!({ "event": "agent-hold", "agent": id, "onHold": hold }));
    Ok(json!({ "ok": true, "onHold": hold }))
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

/// hive:log — the newest-first tail of log.jsonl. Each line is one JSON event;
/// unparseable lines are skipped. `limit` is capped at 500.
pub fn hive_log(hive: &Hive, limit: usize) -> Vec<Value> {
    let cap = limit.min(500);
    let Ok(text) = fs::read_to_string(hive.root.join("log.jsonl")) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take(cap)
        .collect()
}

/// hive:messages — one agent's mailbox: pending inbox plus the .done audit
/// trail. Each row is the raw message object annotated with `done` so the UI
/// can tell backlog from history; sorted newest-first by created_at.
pub fn hive_messages(hive: &Hive, agent_id: &str, limit: usize) -> Vec<Value> {
    let inbox = hive.agent_dir(agent_id).join("inbox");
    let mut out: Vec<Value> = Vec::new();
    for (dir, done) in [(inbox.clone(), false), (inbox.join(".done"), true)] {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        let mut files: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        files.sort();
        for f in files {
            let Ok(t) = fs::read_to_string(&f) else { continue };
            let Ok(mut m) = serde_json::from_str::<Value>(&t) else {
                continue;
            };
            if m.is_object() {
                m["done"] = json!(done);
                out.push(m);
            }
        }
    }
    // ISO-8601 strings sort chronologically — descending puts newest first.
    out.sort_by(|a, b| {
        let ka = a.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        let kb = b.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        kb.cmp(ka)
    });
    out.truncate(limit.min(500));
    out
}

/// hive:textSearch — literal, case-insensitive substring search across the
/// text files under hive/ (*.md, *.txt, *.json). Skips .git, anything
/// non-UTF-8/binary, and files over 256KB. Rows are {path, line, text}
/// (path relative to the hive root, line 1-based), capped at 100.
pub fn text_search(hive: &Hive, query: &str, limit: usize) -> Vec<Value> {
    const MAX_FILE: u64 = 256 * 1024;
    const MAX_ROWS: usize = 100;
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let cap = limit.min(MAX_ROWS);
    let mut out = Vec::new();
    let mut stack = vec![hive.root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                if name != ".git" {
                    stack.push(p);
                }
                continue;
            }
            let is_text = p
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| matches!(x.to_ascii_lowercase().as_str(), "md" | "txt" | "json"));
            if !is_text {
                continue;
            }
            let Ok(meta) = p.metadata() else { continue };
            if meta.len() > MAX_FILE {
                continue;
            }
            // Binary check: a NUL in the first 8KB means skip (jsonl/message
            // blobs never legitimately carry one).
            let Ok(bytes) = fs::read(&p) else { continue };
            if bytes.iter().take(8192).any(|b| *b == 0) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            for (i, line) in text.lines().enumerate() {
                if line.to_lowercase().contains(&needle) {
                    let rel = p
                        .strip_prefix(&hive.root)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.push(json!({
                        "path": rel,
                        "line": i + 1,
                        "text": line.trim(),
                    }));
                    if out.len() >= cap {
                        return out;
                    }
                }
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
        let root = std::env::temp_dir().join(format!("oa-hiveadmin-{tag}-{}", uuid::Uuid::new_v4()));
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
        }
    }

    #[test]
    fn add_and_delete_task() {
        let (_r, hive) = tmp_hive("tasks");
        let card = add_task(&hive, &json!({ "title": "ship it" })).unwrap();
        let id = card["id"].as_str().unwrap();
        assert_eq!(id.len(), 8);
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric() && !c.is_ascii_uppercase()));
        assert!(card["createdAt"].as_u64().unwrap() > 0);

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

        // archived flag lives in the sidecar and SURVIVES a typed registry
        // write (set_status rewrites registry.json wholesale).
        set_archived(&hive, "w-1", true).unwrap();
        hive.set_status("w-1", "working").unwrap();
        assert_eq!(
            agent_extra(&hive, "w-1")["archived"], json!(true),
            "sidecar flag must outlive a registry rewrite"
        );
        set_archived(&hive, "w-1", false).unwrap();
        assert_eq!(agent_extra(&hive, "w-1")["archived"], json!(false));

        set_hold(&hive, "w-1", true).unwrap();
        assert_eq!(agent_extra(&hive, "w-1")["onHold"], json!(true));
        assert!(set_hold(&hive, "ghost", true).is_err());
    }

    #[test]
    fn role_patch_rewrites_identity() {
        let (_r, hive) = tmp_hive("role");
        hive.register_agent(&entry("w-2", "Bob")).unwrap();
        patch_agent_role(&hive, "w-2", "reviewer").unwrap();
        let a = hive.registry().into_iter().find(|a| a.id == "w-2").unwrap();
        assert_eq!(a.role, "reviewer");
        let ident = fs::read_to_string(hive.agent_dir("w-2").join("identity.md")).unwrap();
        assert!(ident.contains("- role: reviewer"));
        assert!(patch_agent_role(&hive, "w-2", "  ").is_err());
    }

    #[test]
    fn log_tail_is_newest_first() {
        let (_r, hive) = tmp_hive("log");
        for i in 0..10 {
            hive.log(&json!({ "event": "t", "i": i })).unwrap();
        }
        let tail = hive_log(&hive, 3);
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[0]["i"], json!(9));
        assert_eq!(tail[2]["i"], json!(7));
        assert!(hive_log(&hive, 999).len() >= 10); // capped at 500, not 999
    }

    #[test]
    fn text_search_finds_workspace_lines() {
        let (_r, hive) = tmp_hive("search");
        let dir = hive.agent_dir("w-3");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("notes.md"), "nothing here\nThe QUICK brown fox\nbye\n").unwrap();
        let hits = text_search(&hive, "quick", 100);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["line"], json!(2));
        assert!(hits[0]["path"].as_str().unwrap().ends_with("notes.md"));
        assert!(text_search(&hive, "absent-token", 100).is_empty());
        assert!(text_search(&hive, "  ", 100).is_empty());
    }

    #[test]
    fn messages_lists_inbox_and_done() {
        let (_r, hive) = tmp_hive("msgs");
        hive.register_agent(&entry("w-4", "Cid")).unwrap();
        let m1 = crate::hive::Message::new("god", "w-4", "inform", "s1", "b1");
        let m2 = crate::hive::Message::new("god", "w-4", "request", "s2", "b2");
        hive.send_virtual(&m1.from, "w-4", &m1.act, &m1.subject, &m1.body).unwrap();
        hive.send_virtual(&m2.from, "w-4", &m2.act, &m2.subject, &m2.body).unwrap();
        let inbox_id = hive.inbox("w-4").unwrap()[0].id.clone();
        hive.mark_done("w-4", &inbox_id).unwrap();
        let msgs = hive_messages(&hive, "w-4", 50);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs.iter().filter(|m| m["done"] == true).count(), 1);
        assert_eq!(msgs.iter().filter(|m| m["done"] == false).count(), 1);
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
