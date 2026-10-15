//! IPC surface for semantic memory — port of index.ts `hive:*` MemPalace
//! handlers:
//!   hive:memoryStatus  → memory_status   (refresh: re-resolve CLI + re-arm mine loop)
//!   hive:searchMemory  → search_memory(query, wing?, limit?)
//!   hive:memoryWakeUp  → memory_wake_up(wing?)
//!   hive:mineNow       → mine_now(agent_id?)
//!
//! When the `mempalace` CLI isn't installed everything degrades to a result
//! object (`{"available": false, ...}` / `{ok:false, error:"semantic memory not
//! active"}`) — never a Tauri error, matching upstream's silent no-op.

use std::path::PathBuf;

use serde_json::{json, Value};
use tauri::AppHandle;

use crate::commands::state;
use crate::memory_palace;

/// <harness> = parent of the hive root (<harness>/hive).
fn harness_home(app: &AppHandle) -> PathBuf {
    let st = state(app);
    st.hive
        .root
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| st.hive.root.clone())
}

/// `hive:memoryStatus` → memory.refresh(): reset the bin cache, idempotently
/// arm the mine loop (a post-boot install otherwise reads "getting ready"
/// until restart), then report status. `available:false` when CLI absent.
#[tauri::command(async)]
pub fn memory_status(app: AppHandle) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("memory_status", || {
    let palace = memory_palace::instance(&harness_home(&app));
    Ok(palace.refresh())
    })
}

/// `hive:searchMemory` — `mempalace search <q> --results N [--wing <w>]`.
/// `limit` maps to upstream's `--results` (default 5 when unset/0).
#[tauri::command(async)]
pub fn search_memory(
    app: AppHandle,
    query: String,
    wing: Option<String>,
    limit: Option<u32>,
) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("search_memory", || {
    if query.trim().is_empty() {
        return Ok(json!({ "ok": false, "output": "", "error": "empty query" }));
    }
    let palace = memory_palace::instance(&harness_home(&app));
    Ok(palace.search(&query, wing.as_deref(), limit.unwrap_or(5)))
    })
}

/// `hive:memoryWakeUp` — `mempalace wake-up [--wing <w>]` session digest.
#[tauri::command(async)]
pub fn memory_wake_up(app: AppHandle, wing: Option<String>) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("memory_wake_up", || {
    let palace = memory_palace::instance(&harness_home(&app));
    Ok(palace.wake_up(wing.as_deref()))
    })
}

/// `hive:mineNow` — fire-and-forget mine pass, returns {ok:true} immediately
/// (upstream contract). `agent_id` narrows the pass to one agent's memory.md;
/// absent = every agent under <harness>/hive/agents whose memory changed.
#[tauri::command(async)]
pub fn mine_now(app: AppHandle, agent_id: Option<String>) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("mine_now", || {
    let palace = memory_palace::instance(&harness_home(&app));
    palace.mine_now(agent_id.filter(|s| !s.is_empty()));
    Ok(json!({ "ok": true }))
    })
}
