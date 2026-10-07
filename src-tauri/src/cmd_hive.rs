//! Tauri commands — the `hive:*`/`workers:*`/`pty:*`/`session:*` IPC surface
//! that indexes through hive_admin.rs rather than the core Hive impl.
//! Frontend calls these via `invoke()`; names mirror upstream's ipcMain
//! channels (hive:addTask → hive_add_task, …).

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::commands::state;

fn err_str(e: anyhow::Error) -> String {
    e.to_string()
}

// ─── task ledger ────────────────────────────────────────────────────────────

/// hive:addTask — append one card; the helper assigns id/createdAt when the
/// caller left them out and rejects a duplicate id.
#[tauri::command(async)]
pub fn hive_add_task(app: AppHandle, fields: Value) -> Result<Value, String> {
    let st = state(&app);
    crate::hive_admin::add_task(&st.hive, &fields).map_err(err_str)
}

/// hive:deleteTask — remove one card by id; returns the remaining list.
#[tauri::command(async)]
pub fn hive_delete_task(app: AppHandle, id: String) -> Result<Value, String> {
    let st = state(&app);
    crate::hive_admin::delete_task(&st.hive, &id).map_err(err_str)
}

/// hive:patchTask — per-card merge over the raw ledger entry (upstream
/// patchTask): every patch key lands on the card (null clears), `id` is
/// re-pinned so a stray id in the patch can't rename it.
#[tauri::command(async)]
pub fn hive_patch_task(app: AppHandle, id: String, patch: Value) -> Result<Value, String> {
    let st = state(&app);
    let patch = patch
        .as_object()
        .cloned()
        .ok_or_else(|| "patch must be an object".to_string())?;
    st.hive.update_task(&id, &patch).map_err(err_str)
}

// ─── registry edits ─────────────────────────────────────────────────────────

/// hive:patchAgentRole — durable job string + identity.md refresh.
#[tauri::command(async)]
pub fn hive_patch_agent_role(app: AppHandle, id: String, role: String) -> Result<Value, String> {
    let st = state(&app);
    crate::hive_admin::patch_agent_role(&st.hive, &id, &role).map_err(err_str)
}

/// hive:renameAgent — display name only; id/dir/mailboxes are unchanged.
#[tauri::command(async)]
pub fn hive_rename_agent(app: AppHandle, id: String, name: String) -> Result<Value, String> {
    let st = state(&app);
    crate::hive_admin::rename_agent(&st.hive, &id, &name).map_err(err_str)
}

/// hive:setArchived — retained-but-hidden floor flag (sidecar).
#[tauri::command(async)]
pub fn hive_set_archived(app: AppHandle, id: String, archived: bool) -> Result<Value, String> {
    let st = state(&app);
    let res = crate::hive_admin::set_archived(&st.hive, &id, archived).map_err(err_str)?;
    // Upstream setArchived dep (index.ts:4530): the floor hears every
    // archive/unarchive regardless of who initiated it.
    let _ = app.emit(
        if archived {
            "hive:agentArchived"
        } else {
            "hive:agentSpawned"
        },
        serde_json::json!({ "id": id }),
    );
    Ok(res)
}

/// hive:setAgentHold — 1:1 claimed by the human; paused-delivery marker.
#[tauri::command(async)]
pub fn hive_set_agent_hold(app: AppHandle, id: String, hold: bool) -> Result<Value, String> {
    let st = state(&app);
    crate::hive_admin::set_hold(&st.hive, &id, hold).map_err(err_str)
}

// ─── read surface ───────────────────────────────────────────────────────────

/// hive:agentDirectory — the consolidated per-agent directory (upstream
/// index.ts:3902): registry record joined with live telemetry + breaker +
/// context slot, INCLUDING archived agents (the floor can still speak to
/// them). `{godId, agents:[…]}`; a `files` param asks for one agent's hive
/// folder listing instead (the port's per-agent file surface).
#[tauri::command(async)]
pub fn hive_agent_directory(app: AppHandle, id: Option<String>) -> Result<Value, String> {
    let st = state(&app);
    if let Some(id) = id {
        return Ok(json!(crate::hive_admin::agent_directory(&st.hive, &id)));
    }
    let snap = st.telemetry.snapshot();
    let usage_by_id: std::collections::HashMap<String, Value> = snap
        .get("usage")
        .and_then(|u| u.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|u| {
                    u.get("agentId")
                        .and_then(|i| i.as_str())
                        .map(|i| (i.to_string(), u.clone()))
                })
                .collect()
        })
        .unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let (god_id, entries) = st.hive.read_registry_doc();
    let agents: Vec<Value> = entries
        .iter()
        .map(|a| {
            let u = usage_by_id.get(&a.id);
            let spans = snap
                .get("spans")
                .and_then(|s| s.get(&a.id))
                .and_then(|s| s.as_array())
                .cloned()
                .unwrap_or_default();
            let tokens = u
                .map(|u| {
                    u["input"].as_u64().unwrap_or(0)
                        + u["output"].as_u64().unwrap_or(0)
                        + u["cache_read"].as_u64().unwrap_or(0)
                        + u["cache_creation"].as_u64().unwrap_or(0)
                })
                .unwrap_or(0);
            let ctx = st.context_usage.lock().unwrap().get(&a.id).copied();
            let usd = u.and_then(|u| u["usd"].as_f64()).unwrap_or(0.0);
            let ts = u.and_then(|u| u["ts"].as_u64());
            json!({
                "id": a.id,
                "name": a.name,
                "role": if a.role.is_empty() { if a.is_god { "orchestrator" } else { "agent" } } else { &a.role },
                "provider": a.provider,
                "model": u.and_then(|u| u["model"].as_str()),
                "status": if a.status.is_empty() { "idle" } else { &a.status },
                "cwd": a.cwd,
                "cwdValid": a.cwd_valid,
                "archived": a.archived,
                "isGod": a.is_god,
                "isAssistant": a.is_assistant,
                "sessionId": a.session_id,
                "hasMemory": st.hive.has_memory(&a.id),
                "inboxBacklog": st.hive.inbox_backlog(&a.id),
                "breaker": st.breaker.lock().unwrap().level_for(&a.id),
                "tokens": tokens,
                "usd": (usd * 10000.0).round() / 10000.0,
                "lastTool": spans.last().and_then(|s| s["tool"].as_str()),
                "lastActiveSecAgo": ts.map(|t| now.saturating_sub(t) / 1000),
                "contextTokens": ctx.map(|(t, _)| t),
                "contextLimit": ctx.map(|(_, l)| l),
                "contextPct": ctx.and_then(|(t, l)| (l > 0).then(|| t * 100 / l)),
            })
        })
        .collect();
    Ok(json!({ "godId": god_id, "agents": agents }))
}

/// hive:agentUsage — cost-ledger fold PLUS the transcript reconciler sample
/// (UsageReader), keyed by the registry's cwd + session_id. When the id isn't
/// registered the argument is treated as a raw cwd — upstream's `hive:agentUsage`
/// took a cwd, so both call shapes resolve.
#[tauri::command(async)]
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
#[tauri::command(async)]
pub fn hive_agent_context(app: AppHandle, agent_id: String) -> Result<Value, String> {
    let st = state(&app);
    // `agent_id` is the registry namespace — live-ness resolves through the
    // pty→agent map (pty keys carry the renderer's `pty-` prefix).
    let live = st.pty_for_agent(&agent_id).is_some();
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
#[tauri::command(async)]
pub fn hive_log(app: AppHandle, limit: Option<u32>) -> Result<Vec<Value>, String> {
    let st = state(&app);
    Ok(crate::hive_admin::hive_log(
        &st.hive,
        limit.unwrap_or(200) as usize,
    ))
}

/// hive:messages — upstream voiceMessages: floor-wide or per-agent mail in
/// the VoiceMessage shape (deduped, secrets redacted). `id` narrows to a
/// single message lookup.
#[tauri::command(async)]
pub fn hive_messages(
    app: AppHandle,
    agent_id: Option<String>,
    id: Option<String>,
    limit: Option<u32>,
    include_archived: Option<bool>,
) -> Result<Vec<Value>, String> {
    let st = state(&app);
    Ok(crate::hive_admin::voice_messages(
        &st.hive,
        agent_id.as_deref(),
        id.as_deref(),
        limit.map(|l| l as usize),
        include_archived,
    ))
}

/// hive:textSearch — literal substring scan across hive text files.
#[tauri::command(async)]
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
#[tauri::command(async)]
pub fn session_resolve_cwd(app: AppHandle, agent_id: String) -> Result<Value, String> {
    let st = state(&app);
    Ok(crate::hive_admin::resolve_cwd(&st.hive, &agent_id))
}

// ─── pty / workers ──────────────────────────────────────────────────────────

/// pty:list — upstream `ptyManager.list()`: `{id, cwd, command, pid,
/// lastOutputAt, hasOutput}` per live PTY. The renderer's
/// waitForTerminalReady keys on `id`/`pid`/`hasOutput`; ids are PTY ids.
#[tauri::command(async)]
pub fn pty_list(app: AppHandle) -> Result<Vec<Value>, String> {
    let st = state(&app);
    let list: Vec<Value> = st
        .ptys
        .lock()
        .unwrap()
        .iter()
        .map(|(id, h)| {
            json!({
                "id": id,
                "cwd": h.cwd,
                "command": h.command,
                "pid": h.pid,
                "lastOutputAt": h.last_output_at.load(std::sync::atomic::Ordering::Relaxed),
                "hasOutput": h.has_output.load(std::sync::atomic::Ordering::Relaxed),
            })
        })
        .collect();
    Ok(list)
}

/// pty:redraw — upstream semantics: a SAME-SIZE resize forces the child's TUI
/// to repaint through the normal output stream, then the retained tail is
/// replayed over the data channels so output that arrived before this terminal
/// subscribed still paints. Without both halves a healthy-but-quiet PTY shows
/// a permanently blank pane (the god-TUI symptom).
/// The return value stays for compat; the renderer's requestInitialPtyRedraw
/// discards it — delivery rides the emitted events.
#[tauri::command(async)]
pub fn pty_redraw(app: AppHandle, id: String) -> Result<String, String> {
    let st = state(&app);
    let (tail, size) = {
        let ptys = st.ptys.lock().unwrap();
        match ptys.get(&id) {
            Some(h) => (h.tail(), h.size()),
            None => return Ok(String::new()),
        }
    };
    if let Some(h) = st.ptys.lock().unwrap().get(&id) {
        let _ = h.resize(size.0, size.1);
    }
    if !tail.is_empty() {
        let _ = app.emit(&format!("pty:data:{id}"), serde_json::json!(tail));
        let _ = app.emit(
            "pty-output",
            crate::pty::PtyOutputPayload {
                id: id.clone(),
                data: tail.clone(),
            },
        );
    }
    Ok(tail)
}

/// workers:list — upstream's ephemeral-worker tab snapshot:
/// `{live: WorkerSnapshot[], preserved: [], maxWorkers}` (the preserved-
/// worktree list is not tracked in this port yet — PORTING.md).
#[tauri::command(async)]
pub fn workers_list(app: AppHandle) -> Result<Value, String> {
    Ok(crate::workers::list(&app))
}

/// workers:registry — every registry entry joined with its live-PTY flag
/// and archived/onHold. The port's floor-roster surface (upstream folds the
/// same data into hive:agentDirectory).
#[tauri::command(async)]
pub fn workers_registry(app: AppHandle) -> Result<Vec<Value>, String> {
    let st = state(&app);
    // Registry rows are agent-keyed; live-ness is agent-namespace too
    // (st.ptys keys are pty ids — translate through the spawn-time mapping).
    let live: std::collections::HashSet<String> = st.live_agent_ids();
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
/// `id` is a worker/agent id — kill_agent resolves either namespace and
/// returns the upstream `{ok, error?}` envelope.
#[tauri::command(async)]
pub fn workers_stop(app: AppHandle, id: String) -> crate::commands::PtyOpResult {
    crate::commands::kill_agent(app, id)
}
