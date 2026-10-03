//! Tauri commands — the `hive:*`/`workers:*`/`pty:*`/`session:*` IPC surface
//! that indexes through hive_admin.rs rather than the core Hive impl.
//! Frontend calls these via `invoke()`; names mirror upstream's ipcMain
//! channels (hive:addTask → hive_add_task, …).

use serde_json::{json, Value};
use tauri::AppHandle;

use crate::commands::state;

fn err_str(e: anyhow::Error) -> String {
    e.to_string()
}

// ─── task ledger ────────────────────────────────────────────────────────────

/// hive:addTask — append one card; the helper assigns id/createdAt when the
/// caller left them out and rejects a duplicate id.
#[tauri::command]
pub fn hive_add_task(app: AppHandle, fields: Value) -> Result<Value, String> {
    let st = state(&app);
    crate::hive_admin::add_task(&st.hive, &fields).map_err(err_str)
}

/// hive:deleteTask — remove one card by id; returns the remaining list.
#[tauri::command]
pub fn hive_delete_task(app: AppHandle, id: String) -> Result<Value, String> {
    let st = state(&app);
    crate::hive_admin::delete_task(&st.hive, &id).map_err(err_str)
}

// ─── registry edits ─────────────────────────────────────────────────────────

/// hive:patchAgentRole — durable job string + identity.md refresh.
#[tauri::command]
pub fn hive_patch_agent_role(app: AppHandle, id: String, role: String) -> Result<Value, String> {
    let st = state(&app);
    crate::hive_admin::patch_agent_role(&st.hive, &id, &role).map_err(err_str)
}

/// hive:renameAgent — display name only; id/dir/mailboxes are unchanged.
#[tauri::command]
pub fn hive_rename_agent(app: AppHandle, id: String, name: String) -> Result<Value, String> {
    let st = state(&app);
    crate::hive_admin::rename_agent(&st.hive, &id, &name).map_err(err_str)
}

/// hive:setArchived — retained-but-hidden floor flag (sidecar).
#[tauri::command]
pub fn hive_set_archived(app: AppHandle, id: String, archived: bool) -> Result<Value, String> {
    let st = state(&app);
    crate::hive_admin::set_archived(&st.hive, &id, archived).map_err(err_str)
}

/// hive:setAgentHold — 1:1 claimed by the human; paused-delivery marker.
#[tauri::command]
pub fn hive_set_agent_hold(app: AppHandle, id: String, hold: bool) -> Result<Value, String> {
    let st = state(&app);
    crate::hive_admin::set_hold(&st.hive, &id, hold).map_err(err_str)
}

// ─── read surface ───────────────────────────────────────────────────────────

/// hive:agentDirectory — the agent's hive folder listing (confined read).
#[tauri::command]
pub fn hive_agent_directory(app: AppHandle, id: String) -> Result<Vec<Value>, String> {
    let st = state(&app);
    Ok(crate::hive_admin::agent_directory(&st.hive, &id))
}

/// hive:agentUsage — cost-ledger fold PLUS the transcript reconciler sample
/// (UsageReader), keyed by the registry's cwd + session_id. When the id isn't
/// registered the argument is treated as a raw cwd — upstream's `hive:agentUsage`
/// took a cwd, so both call shapes resolve.
#[tauri::command]
pub fn hive_agent_usage(app: AppHandle, agent_id: String) -> Result<Value, String> {
    let st = state(&app);
    let mut v = crate::hive_admin::agent_usage(&st.hive, &agent_id);
    let entry = st.hive.registry().into_iter().find(|a| a.id == agent_id);
    let (cwd, sid) = entry
        .map(|a| (a.cwd, a.session_id))
        .unwrap_or_else(|| (agent_id.clone(), None));
    let cwd = if cwd.trim().is_empty() {
        agent_id.clone()
    } else {
        cwd
    };
    v["transcript"] = json!(st.usage.sample(&cwd, sid.as_deref()));
    Ok(v)
}

/// hive:agentContext — the last Status-hook context slot (tokens, limit) for
/// this agent, plus live/registered flags. Null slot until the first hook
/// fires — same "unknown, not empty" reading upstream's gauge needs.
#[tauri::command]
pub fn hive_agent_context(app: AppHandle, agent_id: String) -> Result<Value, String> {
    let st = state(&app);
    let live = st.ptys.lock().unwrap().contains_key(&agent_id);
    let mut v = crate::hive_admin::agent_context(&st.hive, &agent_id, live);
    let slot = st.context_usage.lock().unwrap().get(&agent_id).copied();
    match slot {
        Some((tokens, limit)) => {
            v["tokens"] = json!(tokens);
            v["limit"] = json!(limit);
            v["pct"] = if limit > 0 {
                json!((tokens * 100) / limit)
            } else {
                Value::Null
            };
        }
        None => {
            v["tokens"] = Value::Null;
            v["limit"] = Value::Null;
            v["pct"] = Value::Null;
        }
    }
    Ok(v)
}

/// hive:log — newest-first tail of log.jsonl (default 200, hard cap 500).
#[tauri::command]
pub fn hive_log(app: AppHandle, limit: Option<u32>) -> Result<Vec<Value>, String> {
    let st = state(&app);
    Ok(crate::hive_admin::hive_log(
        &st.hive,
        limit.unwrap_or(200) as usize,
    ))
}

/// hive:messages — one agent's mailbox: pending inbox + .done history,
/// newest first.
#[tauri::command]
pub fn hive_messages(
    app: AppHandle,
    agent_id: String,
    limit: Option<u32>,
) -> Result<Vec<Value>, String> {
    let st = state(&app);
    Ok(crate::hive_admin::hive_messages(
        &st.hive,
        &agent_id,
        limit.unwrap_or(40) as usize,
    ))
}

/// hive:textSearch — literal substring scan across hive text files.
#[tauri::command]
pub fn hive_text_search(
    app: AppHandle,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<Value>, String> {
    let st = state(&app);
    Ok(crate::hive_admin::text_search(
        &st.hive,
        &query,
        limit.unwrap_or(100) as usize,
    ))
}

/// session:resolveCwd — where a resume should boot: registry cwd, else the
/// agent's hive folder.
#[tauri::command]
pub fn session_resolve_cwd(app: AppHandle, agent_id: String) -> Result<Value, String> {
    let st = state(&app);
    Ok(crate::hive_admin::resolve_cwd(&st.hive, &agent_id))
}

// ─── pty / workers ──────────────────────────────────────────────────────────

/// pty:list — live PTY ids.
#[tauri::command]
pub fn pty_list(app: AppHandle) -> Result<Vec<String>, String> {
    let st = state(&app);
    let ids: Vec<String> = st.ptys.lock().unwrap().keys().cloned().collect();
    Ok(ids)
}

/// pty:redraw — the buffered scrollback for one id.
///
/// DEVIATION: this port's PtyHandle keeps NO output buffer (pty.rs streams
/// bytes straight to `pty-output` events; nothing is retained main-side, so
/// there is nothing to replay). Returns "" — live and dead ids alike — until a
/// scrollback buffer lands in pty.rs.
#[tauri::command]
pub fn pty_redraw(app: AppHandle, id: String) -> Result<String, String> {
    let st = state(&app);
    let _ = st.ptys.lock().unwrap().get(&id);
    Ok(String::new())
}

/// workers:list — every registry entry joined with its live-PTY flag and the
/// sidecar's archived/onHold flags (upstream joins liveWorkers + registry;
/// this port's registry IS the live-worker table).
#[tauri::command]
pub fn workers_list(app: AppHandle) -> Result<Vec<Value>, String> {
    let st = state(&app);
    let live: std::collections::HashSet<String> =
        st.ptys.lock().unwrap().keys().cloned().collect();
    let extras = crate::hive_admin::all_extras(&st.hive);
    let mut out = Vec::new();
    for a in st.hive.registry() {
        let mut v = serde_json::to_value(&a).unwrap_or_else(|_| json!({}));
        let extra = extras.get(&a.id);
        v["live"] = json!(live.contains(&a.id));
        v["archived"] = extra
            .and_then(|e| e.get("archived"))
            .cloned()
            .unwrap_or(json!(false));
        v["onHold"] = extra
            .and_then(|e| e.get("onHold"))
            .cloned()
            .unwrap_or(json!(false));
        out.push(v);
    }
    Ok(out)
}

/// workers:stop — the same teardown as pty:kill/kill_agent (kill the PTY,
/// forget breaker/wake/telemetry, mark stopped, emit agent-status).
#[tauri::command]
pub fn workers_stop(app: AppHandle, id: String) -> Result<(), String> {
    crate::commands::kill_agent(app, id)
}
