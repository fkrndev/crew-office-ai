//! Tauri commands — the IPC surface replacing Electron's ipcMain.handle.
//! Frontend calls these via `invoke()`; async work emits events back.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::breaker::CircuitBreaker;
use crate::config::{Config, ConfigStore};
use crate::control::{ControlRegistry, ControlSnapshot};
use crate::hive::{Hive, Message, RegistryEntry};
use crate::pty::{self, PtyHandle};
use crate::ptyenv::build_pty_env;

pub struct AppState {
    pub hive: std::sync::Arc<Hive>,
    pub config_store: ConfigStore,
    pub config: Mutex<Config>,
    /// Alive PTYs keyed by PTY id — the renderer's terminal id (`pty-<agent>`),
    /// NOT the registry/agent id (upstream opts.id vs hive.id).
    pub ptys: Mutex<HashMap<String, PtyHandle>>,
    /// pty id -> agent id (upstream `ptyToAgent`): populated at spawn, pruned
    /// on teardown. The two namespaces are separate — a renderer-spawned agent
    /// "god" lives in `st.ptys` under "pty-god".
    pub pty_to_agent: Mutex<HashMap<String, String>>,
    /// agent id -> pty id (upstream `ptyForAgent`): the inverse lookup every
    /// agent-namespaced caller (hooks, breaker, worker watchdog, missions)
    /// needs before touching `st.ptys`.
    pub agent_to_pty: Mutex<HashMap<String, String>>,
    pub breaker: Mutex<CircuitBreaker>,
    pub control: Mutex<ControlRegistry>,
    /// Transcript-based usage reader (usage.rs) — the breaker's fallback
    /// sample source when live OTel has nothing yet.
    pub usage: crate::usage::UsageReader,
    /// TCP loopback addr of the HookServer (`127.0.0.1:port`) → env HIVE_SOCK.
    pub sock_addr: Option<String>,
    /// The native hook-shim binary copied into `<harness>/bin/` — the analogue
    /// of upstream writing cth-hook.cjs into the hive's bin dir.
    pub hook_bin: Option<PathBuf>,
    /// Agents with a `/compact` pending — queued when a compact mission fires
    /// mid-work, drained at the next idle boundary (upstream's renderer-side
    /// compact queue, moved backend-side where agent status lives).
    pub compact_queue: Mutex<std::collections::HashSet<String>>,
    /// Inbox-wake watchdog backstop (workerwake.rs) — nudges idle workers
    /// whose mail landed AFTER their last Stop (the stall the renderer-side
    /// idle loop misses). Fed by hooks/spawn/kill.
    pub wake: Mutex<crate::workerwake::WorkerWakeWatchdog>,
    /// Inbound webhook gateway (webhook.rs) — loopback HTTP, secret-gated,
    /// routes verified posts into god's inbox.
    pub webhook: std::sync::Arc<crate::webhook::WebhookServer>,
    /// OTLP/HTTP-JSON collector (telemetry.rs) — live usage, tool spans,
    /// api_error feed for claude agents.
    pub telemetry: std::sync::Arc<crate::telemetry::TelemetryCollector>,
    /// Bound collector URL set at boot (spawn reads it for OTEL_* env).
    pub otel_endpoint: std::sync::OnceLock<String>,
    /// The trigger ledger (trigger_history.rs) — every inbound webhook + the
    /// operator's verdict on it. Own file, disposable, never a secret.
    pub trigger_history: crate::trigger_history::TriggerHistory,
    /// Latest context fill per live agent (tokens, limit) — fed by the Status
    /// hook/telemetry, read by the context-pressure trigger.
    pub context_usage: Mutex<HashMap<String, (u64, u64)>>,
    /// Last fire timestamp per context-trigger action ("compact"/"clear") —
    /// the cadence stamp (upstream's persisted context lastRun map).
    pub context_last_compact: Mutex<HashMap<String, u64>>,
    /// "agent\tsession" → last ledgered cumulative usd — the #56 dedup gate
    /// (only append when the counter moved).
    pub cost_moved: Mutex<HashMap<String, f64>>,
    /// The roster mirror (roster.rs) — UI floor state on disk with
    /// append-only backups + the empty-first-write guard.
    pub roster: crate::roster::RosterStore,
    /// Mission re-arm signal — upstream's `clearMissionTimers`+`syncMissions`
    /// on every config save. `missions:save` notifies; the scheduler loop
    /// wakes immediately instead of waiting out its sleep.
    pub mission_rearm: tokio::sync::Notify,
    /// Loopback port of the sync-IPC server (sync_api.rs) — the renderer's
    /// sendSync equivalents ride synchronous XHR here. 0 = not bound.
    pub sync_port: std::sync::atomic::AtomicU16,
    /// Closing-time protocol (closing_time.rs) — graceful shutdown via the
    /// hive rails: god brief + worker ACKs + verified COMPLETE.
    pub closing: crate::closing_time::ClosingTime,
    /// Slack Events-API adapter (slack.rs) — HMAC-gated inbound → god inbox.
    pub slack: std::sync::Arc<crate::slack::SlackServer>,
    /// Loopback reply rail — agents post back to Slack without ever seeing
    /// the bot token (upstream SlackReplyServer).
    pub slack_reply: std::sync::Arc<crate::slack::SlackReplyServer>,
    /// thread_ts values an agent already answered DIRECTLY via the reply
    /// rail — the done-summary poller skips these (fallback, not duplicator).
    pub directly_replied: std::sync::Arc<Mutex<std::collections::HashSet<String>>>,
    /// Durable harness state (db.rs) — kv + command history in SQLite/WAL.
    pub persist: Mutex<crate::db::PersistStore>,
    /// The (session_id, goal) last injected per agent — upstream's
    /// deliveredGoalByAgent: the durable standing goal is re-sent on a new
    /// session or a roster change, not every prompt (transcript bloat).
    pub delivered_goal: Mutex<HashMap<String, (Option<String>, Option<String>)>>,
    /// Live god-triggered ephemeral workers (workers.rs spawn-request
    /// watcher) — release-on-done + idle/token reaping state.
    pub live_workers: crate::workers::LiveWorkers,
    /// id → (worktreePath, origCwd) for agents spawned `isolate: true`
    /// (upstream worktreePaths/worktreeOrigins — torn down on kill).
    pub worktree_paths: Mutex<HashMap<String, (PathBuf, PathBuf)>>,
    /// Preserved worker worktrees awaiting integration-GC (upstream
    /// preservedWorktrees — the GC sweep drains it every ~60s).
    pub preserved_worktrees: Mutex<HashMap<PathBuf, crate::workers::PreservedWorktree>>,
    /// Live proxy-bridge sidecars per agent (qwen/crush) — stopped on
    /// teardown so a dead PTY never leaves an orphan listener.
    pub proxy_bridges: Mutex<HashMap<String, std::sync::Arc<crate::proxy::ProxyBridge>>>,
    /// The Slack public tunnel (upstream tunnelmole openTunnel) — killed on
    /// slack_stop; a None means the rail is loopback-only this session.
    pub slack_tunnel: Mutex<Option<(String, std::process::Child)>>,
}

impl AppState {
    pub fn new(harness_home: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&harness_home)?;
        let hive = Hive::ensure(&harness_home.join("hive"))?;
        let config_store = ConfigStore::new(harness_home.clone());
        let config = config_store.load();
        // Stage the hive-hook binary next to the hive, like upstream's
        // bin/cth-hook.cjs. Resolved from beside the running exe (dev:
        // target/debug/, bundle: install dir). Missing → hooks degrade off.
        let hook_bin = stage_hook_bin(&harness_home);
        let mut config = config;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if config.seed_missions(now) {
            let _ = config_store.save(&config);
        }
        let mut persist_store = crate::db::PersistStore::new(crate::db::db_path(&harness_home));
        let _ = persist_store.open(); // best-effort — a DB failure can't crash startup
        let context_last_run: HashMap<String, u64> = persist_store
            .get_kv("context:lastRun")
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        Ok(Self {
            hive: std::sync::Arc::new(hive),
            config_store,
            config: Mutex::new(config),
            ptys: Mutex::new(HashMap::new()),
            pty_to_agent: Mutex::new(HashMap::new()),
            agent_to_pty: Mutex::new(HashMap::new()),
            breaker: Mutex::new(CircuitBreaker::default()),
            control: Mutex::new(ControlRegistry::default()),
            usage: crate::usage::UsageReader::default(),
            compact_queue: Mutex::new(std::collections::HashSet::new()),
            wake: Mutex::new(crate::workerwake::WorkerWakeWatchdog::default()),
            webhook: std::sync::Arc::new(crate::webhook::WebhookServer::new()),
            telemetry: std::sync::Arc::new(crate::telemetry::TelemetryCollector::new()),
            otel_endpoint: std::sync::OnceLock::new(),
            trigger_history: crate::trigger_history::TriggerHistory::new(harness_home.clone()),
            context_usage: Mutex::new(HashMap::new()),
            // CONTEXT_LAST_RUN_KV_KEY (index.ts:823): the cadence survives
            // restarts — an overdue rule fires once on boot, not a full
            // interval late.
            context_last_compact: Mutex::new(context_last_run),
            cost_moved: Mutex::new(HashMap::new()),
            roster: crate::roster::RosterStore::new(harness_home.clone()),
            mission_rearm: tokio::sync::Notify::new(),
            sync_port: std::sync::atomic::AtomicU16::new(0),
            closing: crate::closing_time::ClosingTime::default(),
            slack: std::sync::Arc::new(crate::slack::SlackServer::new()),
            slack_reply: std::sync::Arc::new(crate::slack::SlackReplyServer::new()),
            directly_replied: std::sync::Arc::new(Mutex::new(std::collections::HashSet::new())),
            delivered_goal: Mutex::new(HashMap::new()),
            live_workers: Mutex::new(HashMap::new()),
            worktree_paths: Mutex::new(HashMap::new()),
            preserved_worktrees: Mutex::new(HashMap::new()),
            proxy_bridges: Mutex::new(HashMap::new()),
            slack_tunnel: Mutex::new(None),
            persist: Mutex::new(persist_store),
            sock_addr: None,
            hook_bin,
        })
    }
}

fn stage_hook_bin(harness_home: &PathBuf) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "hive-hook.exe"
    } else {
        "hive-hook"
    };
    let src = std::env::current_exe().ok()?.parent()?.join(name);
    if !src.is_file() {
        return None;
    }
    let dest = harness_home.join("bin").join(name);
    if let Some(p) = dest.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    std::fs::copy(&src, &dest).ok().map(|_| dest)
}

impl AppState {
    /// `ptyForAgent` (index.ts:1107) — the PTY id owning an agent id, if live.
    pub fn pty_for_agent(&self, agent_id: &str) -> Option<String> {
        self.agent_to_pty.lock().unwrap().get(agent_id).cloned()
    }

    /// `ptyToAgent.get` (index.ts) — the agent id owning a PTY id, if known.
    pub fn agent_for_pty(&self, pty_id: &str) -> Option<String> {
        self.pty_to_agent.lock().unwrap().get(pty_id).cloned()
    }

    /// Live AGENT ids — the value set of `pty_to_agent`. All "is this agent
    /// live" checks (roster, closing-time, breaker, worker registry) are in
    /// the agent namespace; `st.ptys` keys carry the `pty-` renderer prefix.
    pub fn live_agent_ids(&self) -> std::collections::HashSet<String> {
        self.pty_to_agent.lock().unwrap().values().cloned().collect()
    }

    /// Resolve EITHER namespace to a PTY id — renderer IPC carries pty ids
    /// (`pty:kill`, `pty:write`), internal callers (worker reaps, voice kill)
    /// carry agent ids.
    pub fn resolve_pty_id(&self, id: &str) -> Option<String> {
        if self.ptys.lock().unwrap().contains_key(id) {
            Some(id.to_string())
        } else {
            self.pty_for_agent(id)
        }
    }

    /// Record the spawn-time pty<->agent mapping (upstream `ptyToAgent.set`).
    pub fn map_pty(&self, pty_id: &str, agent_id: &str) {
        self.pty_to_agent
            .lock()
            .unwrap()
            .insert(pty_id.to_string(), agent_id.to_string());
        self.agent_to_pty
            .lock()
            .unwrap()
            .insert(agent_id.to_string(), pty_id.to_string());
    }

    /// Drop the mapping for a dead/removed PTY; returns the agent id it
    /// owned (None for unmapped PTYs — installer rails and legacy spawns).
    pub fn unmap_pty(&self, pty_id: &str) -> Option<String> {
        let agent = self.pty_to_agent.lock().unwrap().remove(pty_id);
        if let Some(a) = &agent {
            self.agent_to_pty.lock().unwrap().remove(a);
        }
        agent
    }
}

pub fn state<R: tauri::Runtime>(app: &AppHandle<R>) -> State<'_, AppState> {
    app.state::<AppState>()
}

// ─── agent lifecycle ────────────────────────────────────────────────────────

/// The INTERNAL spawn descriptor — what translate_spawn_request produces from
/// the wire shape. Kept identical to the port's pre-split contract so every
/// internal caller (workers, realtime voice, respawn) still speaks it.
#[derive(Debug, Clone, Deserialize)]
pub struct SpawnArgs {
    pub provider: String,
    pub name: String,
    pub role: Option<String>,
    pub cwd: String,
    /// Full command line (preset default + flags assembled by the frontend, or
    /// a raw custom command). Kept as one string — see pty::spawn's shell wrap.
    pub command: String,
    /// Typed-into-the-TUI seed prompt ('type-into-tui' delivery). When unset,
    /// non-hiveAware providers still get the hive prompt typed automatically.
    #[serde(default)]
    pub seed_prompt: Option<String>,
    /// Git isolation (upstream AgentSpawnOptions.isolate): give the agent its
    /// own `agent/<id>` worktree under <harness>/worktrees/ so it can't
    /// clobber other agents' (or the user's) working tree. Best-effort — a
    /// failure falls back to the shared cwd rather than blocking the spawn.
    #[serde(default)]
    pub isolate: bool,
    #[serde(default)]
    pub is_god: bool,
    /// hive.isAssistant (upstream AgentMeta) — the send-only prep assistant
    /// marker, excluded from broadcast fan-out.
    #[serde(default)]
    pub is_assistant: bool,
    /// hive.capabilities (upstream AgentMeta) — hire-declared skills carried
    /// into the registry entry.
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub no_auto_install: bool,
    /// Per-agent MCP overrides — catalog ids enabled for THIS spawn on top of
    /// the global mcpDefaults consent (e.g. `shardx` for a marketplace agent).
    /// mcp_disable removes a globally-consented id for this spawn.
    #[serde(default)]
    pub mcp_enable: Vec<String>,
    #[serde(default)]
    pub mcp_disable: Vec<String>,
    /// The UI-picked model — kept off argv for providers whose CLI lacks a
    /// model flag (openclaw `tui`); consumed by the per-provider seeders.
    #[serde(default)]
    pub model: Option<String>,
    /// Optional ShardX profile binding — the ACCOUNT's launcher profile this
    /// agent drives. Empty = auto `office-<agentId>` (one agent, one identity).
    /// Set = bind to a named profile so sibling agents on the same shop can
    /// share ONE login instead of each needing its own fingerprint+OTP.
    #[serde(default)]
    pub shardx_profile: Option<String>,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
}

fn default_cols() -> u16 {
    120
}
fn default_rows() -> u16 {
    30
}

/// The upstream `pty:spawn` wire payload verbatim (AgentSpawnOptions) — what
/// the verbatim Electron renderer sends through `window.cth.spawnPty`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtySpawnRequest {
    /// The PTY id — the terminal's map key. Renderer convention: `pty-<agentId>`.
    pub id: String,
    /// Requested working directory (tilde-expands in spawn_inner).
    #[serde(default)]
    pub cwd: String,
    /// The executable name — a bare binary (`claude`) or absolute path.
    #[serde(default)]
    pub command: String,
    /// Explicit provider override; beats hive.provider and binary sniffing
    /// (upstream inferAgentProvider).
    #[serde(default)]
    pub provider: Option<String>,
    /// The executable's argv — upstream passes argv separately; we reassemble
    /// a command line for the internal SpawnArgs.
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
    #[serde(default)]
    pub isolate: bool,
    /// Resume the registry-recorded session for `hive.id` (upstream `resume`).
    #[serde(default)]
    pub resume: bool,
    /// An explicit CLI session id — wins over `resume` (upstream resumeSessionId).
    #[serde(default)]
    pub resume_session_id: Option<String>,
    /// "Restart & Continue": refuse to spawn rather than start fresh when the
    /// resume can't be satisfied (upstream requireResume).
    #[serde(default)]
    pub require_resume: bool,
    /// The registry/AGENT_ID identity block (upstream AgentMeta). `hive.id` is
    /// the AGENT id — deliberately separate from the PTY `id` above.
    #[serde(default)]
    pub hive: Option<HiveSpawnMeta>,
    /// Type-into-TUI seed — returned to the renderer, which types it via the
    /// pty:write chain (upstream's seedPrompt contract).
    #[serde(default)]
    pub seed_prompt: Option<String>,
    /// The UI-picked model — carried separately because openclaw's `tui` has
    /// NO --model flag; it is seeded into the agent's config instead of argv.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub no_auto_install: bool,
    /// Per-agent MCP catalog ids to enable/disable for this spawn — rides on
    /// top of the global mcpDefaults consent (SpawnArgs.mcp_enable/disable).
    #[serde(default)]
    pub mcp_enable: Vec<String>,
    #[serde(default)]
    pub mcp_disable: Vec<String>,
    /// Optional ShardX profile name to bind (SpawnArgs.shardx_profile).
    #[serde(default)]
    pub shardx_profile: Option<String>,
}

/// The upstream AgentMeta block nested under `hive` (index.ts AgentSpawnOptions.hive).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HiveSpawnMeta {
    /// The AGENT id — registry entry, AGENT_ID env, hook payloads, hive paths.
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub is_god: bool,
    #[serde(default)]
    pub is_assistant: bool,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// The spawn result the upstream renderer reads (`res.ok` / res.error /
/// res.cwd / res.seedPrompt / res.resumed / res.resumeNotFound /
/// res.worktreePath) — camelCase, and failures resolve `{ok:false}` rather
/// than rejecting the invoke (spawn_fail classification is the renderer's).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtySpawnResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The PTY id the session registered under (echoed upstream `opts.id`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The effective cwd actually spawned into (tilde-expanded / worktree).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Type-into-TUI seed — the renderer submits it via the write chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed_prompt: Option<String>,
    pub resumed: bool,
    /// Explicit session id was requested but no matching transcript exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_not_found: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    /// The spawn succeeded but the CLI will boot authless — no provider key
    /// resolved from any source (keychain, env injection, seeded config).
    /// Carries the provider id so the renderer can point at the fix
    /// (upstream treats an ok-spawn as healthy; without this the agent card
    /// shows "starting up" while the TUI is already printing 401s).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_key: Option<String>,
    /// ShardX was armed for this spawn (consent granted) but the local setup
    /// gates aren't met — vendored bridge or stored token missing. The
    /// renderer turns this into a one-click bootstrap + respawn instead of
    /// silently fielding an agent that can't find its browser tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_shardx: Option<bool>,
}

impl PtySpawnResult {
    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(msg.into()),
            id: None,
            cwd: None,
            seed_prompt: None,
            resumed: false,
            resume_not_found: None,
            worktree_path: None,
            needs_key: None,
            needs_shardx: None,
        }
    }
}

#[derive(Serialize)]
pub struct Spawned {
    pub id: String,
}

/// How a spawn resolves its resume (upstream index.ts:2871): an explicit
/// `resumeSessionId` wins; `resume: true` reads the registry's recorded
/// session for the agent.
#[derive(Debug, Clone)]
pub(crate) enum ResumeRequest {
    /// `resume: true` — pick up the registry's recorded session.
    LastSession,
    /// `resumeSessionId` — an explicit id typed in Add Agent.
    Explicit(String),
}

/// Everything spawn_inner needs besides the SpawnArgs — the two id namespaces
/// (upstream's ptyToAgent split) plus resume/seed policy.
#[derive(Debug, Clone, Default)]
pub(crate) struct SpawnPlan {
    /// Forced `st.ptys` key — the renderer's terminal `id` ("pty-<agentId>").
    /// Generated from the agent name when absent (legacy internal calls).
    pub pty_id: Option<String>,
    /// Forced registry/AGENT_ID/hook-payload id — upstream `hive.id`.
    /// Falls back to the pty id for hive-less/internal spawns (the legacy
    /// collapse — worker and voice spawns deliberately keep pty id ==
    /// agent id, matching upstream's worker-<req> convention).
    pub agent_id: Option<String>,
    pub resume: Option<ResumeRequest>,
    /// Upstream requireResume ("Restart & Continue"): refuse outright rather
    /// than starting a fresh session when the resume can't be satisfied.
    pub require_resume: bool,
    /// Type the seed into the PTY ourselves after boot — the renderer-less
    /// paths (PENDING_INSTALLS relaunch, worker/voice/respawn) where nobody
    /// consumes the result's seedPrompt field.
    pub auto_type_seed: bool,
}

/// What spawn_inner produced — translated into the wire shape by
/// spawn_agent_core (PtySpawnResult) or Spawned by respawn_agent.
pub(crate) struct SpawnOutcome {
    /// The `st.ptys` key the session was registered under.
    pub pty_id: String,
    /// The effective cwd actually spawned into (tilde-expanded / worktree).
    pub cwd: String,
    /// The type-into-TUI seed the CALLER must deliver (renderer types it via
    /// the write chain). Still populated when auto_type_seed typed it — those
    /// callers drop the outcome anyway.
    pub seed_prompt: Option<String>,
    /// A resume session was actually applied to the spawned CLI.
    pub resumed: bool,
    /// A session was explicitly requested but couldn't be located.
    pub resume_not_found: bool,
    /// The isolate-worktree path when one was created.
    pub worktree_path: Option<String>,
    /// Spawned CLI will boot without credentials (no provider key resolved).
    /// Surfaces as PtySpawnResult.needsKey.
    pub needs_key: Option<String>,
    /// ShardX armed but its setup gates weren't met — PtySpawnResult.needsShardx.
    pub needs_shardx: bool,
}

impl SpawnOutcome {
    fn into_result(self) -> PtySpawnResult {
        PtySpawnResult {
            ok: true,
            error: None,
            id: Some(self.pty_id),
            cwd: Some(self.cwd),
            seed_prompt: self.seed_prompt,
            resumed: self.resumed,
            resume_not_found: self.resume_not_found.then_some(true),
            worktree_path: self.worktree_path,
            needs_key: self.needs_key,
            needs_shardx: self.needs_shardx.then_some(true),
        }
    }
}

/// One armed install-relaunch: the original SpawnArgs (no_auto_install set) +
/// the plan that produced the install PTY, so the relaunched spawn re-binds
/// the same agent identity and any requested resume.
#[derive(Debug, Clone)]
pub(crate) struct PendingInstall {
    pub args: SpawnArgs,
    pub plan: SpawnPlan,
}

/// Upstream `pendingInstallRelaunch` — install PTYs armed to re-run their
/// agent spawn on a clean exit (the retry carries `no_auto_install` so the
/// ladder can never loop). Keyed by PTY id.
pub(crate) static PENDING_INSTALLS: std::sync::LazyLock<Mutex<HashMap<String, PendingInstall>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Quote one argv token so `tokenize_command` round-trips it losslessly —
/// whitespace, empties and embedded quotes (`--settings {"a":1}`) all
/// survive via the CRT-style backslash escaping.
fn quote_spawn_token(t: &str) -> String {
    crate::misc::quote_command_token(t)
}

/// Reassemble upstream's (command, args[]) split into the single command
/// line SpawnArgs carries — index.ts passes exe+argv separately while the
/// port's spawn_inner retokenizes the line.
fn join_command(exe: &str, args: &[String]) -> String {
    let mut parts = vec![quote_spawn_token(exe)];
    parts.extend(args.iter().map(|a| quote_spawn_token(a)));
    parts.join(" ")
}

/// `inferAgentProvider` (shared/agentProvider.ts): an explicit provider wins;
/// else sniff the executable leaf (cmd-shim suffixes stripped).
fn infer_agent_provider(command: &str, explicit: Option<&str>) -> String {
    const KNOWN: &[&str] = &[
        "claude",
        "codex",
        "grok",
        "kimi",
        "gemini",
        "antigravity",
        "qwen",
        "opencode",
        "crush",
        "pi",
        "copilot",
        "cursor",
        "hermes",
        "openclaw",
        "custom",
    ];
    if let Some(p) = explicit
        .map(|s| s.trim().to_lowercase())
        .filter(|p| KNOWN.contains(&p.as_str()))
    {
        return p;
    }
    let leaf = command
        .trim()
        .split_whitespace()
        .next()
        .unwrap_or("")
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_lowercase();
    let bin = [".exe", ".cmd", ".bat", ".ps1"]
        .iter()
        .find_map(|ext| leaf.strip_suffix(ext))
        .unwrap_or(&leaf);
    match bin {
        "codex" => "codex",
        "grok" => "grok",
        "kimi" => "kimi",
        "gemini" => "gemini",
        "agy" | "antigravity" => "antigravity",
        "qwen" => "qwen",
        "opencode" => "opencode",
        "crush" => "crush",
        "pi" => "pi",
        "copilot" => "copilot",
        "cursor-agent" | "agent" => "cursor",
        "hermes" => "hermes",
        "openclaw" => "openclaw",
        // An unknown binary is a custom command; empty defaults to claude,
        // upstream's preset default.
        "" | "claude" => "claude",
        _ => "custom",
    }
    .to_string()
}

/// The wire request → (internal SpawnArgs, SpawnPlan) translation. Pure —
/// the unit tests exercise it without an AppHandle. Namespaces split HERE:
/// `req.id` is the PTY key, `hive.id` the registry/AGENT id.
pub(crate) fn translate_spawn_request(
    req: &PtySpawnRequest,
) -> std::result::Result<(SpawnArgs, SpawnPlan), String> {
    // Upstream pty:spawn validates id/cwd/command are strings — our typed
    // struct does that; the remaining check is the non-empty case.
    let pty_id = req.id.trim().to_string();
    if pty_id.is_empty() || req.command.trim().is_empty() {
        return Err("invalid SpawnOptions".into());
    }
    let hive = req.hive.as_ref();
    let agent_id = hive
        .map(|h| h.id.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| pty_id.clone());
    let provider = infer_agent_provider(
        &req.command,
        req.provider
            .as_deref()
            .or_else(|| hive.and_then(|h| h.provider.as_deref())),
    );
    let name = hive
        .and_then(|h| h.name.clone())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| agent_id.clone());
    // Upstream precedence: a typed resumeSessionId wins; `resume: true` then
    // asks the registry for this agent's recorded session.
    let resume = req
        .resume_session_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| ResumeRequest::Explicit(s.to_string()))
        .or(if req.resume {
            Some(ResumeRequest::LastSession)
        } else {
            None
        });
    Ok((
        SpawnArgs {
            provider,
            name,
            role: hive.and_then(|h| h.role.clone()),
            cwd: req.cwd.clone(),
            command: join_command(&req.command, &req.args),
            seed_prompt: req.seed_prompt.clone(),
            model: req.model.clone(),
            isolate: req.isolate,
            is_god: hive.map(|h| h.is_god).unwrap_or(false),
            is_assistant: hive.map(|h| h.is_assistant).unwrap_or(false),
            capabilities: hive.map(|h| h.capabilities.clone()).unwrap_or_default(),
            no_auto_install: req.no_auto_install,
            mcp_enable: req.mcp_enable.clone(),
            mcp_disable: req.mcp_disable.clone(),
            shardx_profile: req
                .shardx_profile
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from),
            cols: req.cols,
            rows: req.rows,
        },
        SpawnPlan {
            pty_id: Some(pty_id),
            agent_id: Some(agent_id),
            resume,
            require_resume: req.require_resume,
            // Renderer-facing spawns hand the seed back as seedPrompt — the
            // verbatim useHive flow types it through the write chain.
            auto_type_seed: false,
        },
    ))
}

/// `pty:spawn` (index.ts:2611) — the upstream wire contract. The renderer
/// invokes `spawn_agent` with `{args: <AgentSpawnOptions>}` (see cth.ts); the
/// nested shape deserializes into PtySpawnRequest. Failures resolve as
/// `{ok:false, error}` — the renderer's `res.ok` contract — rather than
/// rejecting the command.
#[tauri::command(async)]
pub fn spawn_agent(app: AppHandle, args: PtySpawnRequest) -> PtySpawnResult {
    spawn_agent_core(&app, args)
}

/// The command body, separated so tests can drive it through a mock runtime.
pub(crate) fn spawn_agent_core<R: tauri::Runtime>(
    app: &AppHandle<R>,
    req: PtySpawnRequest,
) -> PtySpawnResult {
    let (args, plan) = match translate_spawn_request(&req) {
        Ok(t) => t,
        Err(e) => return PtySpawnResult::err(e),
    };
    match spawn_inner(app.clone(), args, plan) {
        Ok(out) => out.into_result(),
        Err(e) => PtySpawnResult::err(e),
    }
}

/// Respawn a registry agent — same agent id, same hive folder/memory, a fresh
/// `pty-<id>` terminal, optionally resuming the recorded session
/// (claude --resume <session_id>). This is the restart/restore-team path:
/// the registry survives, only the PTY died.
#[tauri::command(async)]
pub fn respawn_agent(app: AppHandle, agent_id: String) -> Result<Spawned, String> {
    let st = state(&app);
    let entry = st
        .hive
        .registry()
        .into_iter()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| format!("no agent {agent_id}"))?;
    if st.pty_for_agent(&agent_id).is_some() {
        return Err(format!("agent {agent_id} is already running"));
    }
    let args = SpawnArgs {
        provider: entry.provider.clone(),
        name: entry.name.clone(),
        role: Some(entry.role.clone()),
        cwd: entry.cwd.clone(),
        command: entry.command.clone(),
        seed_prompt: None,
        isolate: false,
        is_god: entry.is_god,
        is_assistant: entry.is_assistant,
        capabilities: entry.capabilities.clone(),
        no_auto_install: false,
        // Per-agent consent + browser binding survive respawn via `extra` —
        // otherwise a restarted marketplace agent silently loses its tools.
        mcp_enable: entry
            .extra
            .get("mcpEnable")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        mcp_disable: entry
            .extra
            .get("mcpDisable")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        shardx_profile: entry
            .extra
            .get("shardxProfile")
            .and_then(|v| v["name"].as_str())
            .map(String::from),
        // The registry's recorded model (in `extra`) — respawns re-seed it
        // for flag-less providers (openclaw).
        model: entry
            .extra
            .get("model")
            .and_then(|v| v.as_str())
            .map(String::from),
        cols: 120,
        rows: 30,
    };
    // Claude resumes by session id; other providers start fresh (their resume
    // flags differ — PORTING.md notes the preset fields for later).
    let resume = (entry.provider == "claude")
        .then(|| entry.session_id.clone())
        .flatten();
    spawn_inner(
        app,
        args,
        SpawnPlan {
            pty_id: Some(format!("pty-{agent_id}")),
            agent_id: Some(agent_id),
            resume: resume.map(ResumeRequest::Explicit),
            require_resume: false,
            // No renderer consumed this spawn's result — keep the server-side
            // seed typing the old single-shot path had.
            auto_type_seed: true,
        },
    )
    .map(|o| Spawned { id: o.pty_id })
}

pub(crate) fn spawn_inner<R: tauri::Runtime>(
    app: AppHandle<R>,
    args: SpawnArgs,
    plan: SpawnPlan,
) -> Result<SpawnOutcome, String> {
    // (pty_id, agent_id) once the spawn passed the duplicate-id gate: any
    // later failure must not leak the registry status, isolation worktree,
    // proxy bridge or a half-registered PTY.
    // The bool: the agent had NO registry row before this call (a fresh hire).
    let mut claimed: Option<(String, String, bool)> = None;
    let res = spawn_inner_impl(app.clone(), args, plan, &mut claimed);
    if res.is_err() {
        if let Some((pty_id, agent_id, fresh)) = claimed {
            let st = state(&app);
            st.map_pty(&pty_id, &agent_id);
            if let Some(mut h) = st.ptys.lock().unwrap().remove(&pty_id) {
                let _ = h.kill();
            }
            // Shared teardown: proxy bridge stop, worktree removal (guarded —
            // never deletes unintegrated work), status → gone.
            teardown_pty_core(&app, &pty_id, "gone");
            // A fresh hire that never came up must not linger as a "gone"
            // registry ghost; an existing agent keeps its row (status gone).
            if fresh {
                let _ = st.hive.remove_agent(&agent_id);
            }
        }
    }
    res
}

/// Worktree dir segment for an agent id. Ids that need sanitising (or would
/// be `.`/`..`) get a stable hash suffix so two distinct ids can never share
/// a worktree/branch and the segment can never climb out of the worktree root.
fn worktree_seg(agent_id: &str) -> String {
    let seg: String = agent_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if seg == agent_id && !seg.is_empty() && !seg.chars().all(|c| c == '.') {
        return seg;
    }
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(agent_id.as_bytes());
    let tag: String = h.iter().take(4).map(|b| format!("{b:02x}")).collect();
    let stem = seg.trim_matches(|c| c == '.' || c == '-');
    format!("{}-{tag}", if stem.is_empty() { "agent" } else { stem })
}

/// Branch to base an isolation worktree on, from `git_ops::branch()`'s
/// `{branch, detached}` shape — a detached HEAD (or an unborn branch) bases
/// on `HEAD` itself.
fn worktree_base_from_branch(b: &serde_json::Value) -> Option<String> {
    if b["detached"].as_bool() == Some(true) {
        return Some("HEAD".into());
    }
    b["branch"]
        .as_str()
        .filter(|s| !s.starts_with('('))
        .map(str::to_string)
}

fn spawn_inner_impl<R: tauri::Runtime>(
    app: AppHandle<R>,
    args: SpawnArgs,
    plan: SpawnPlan,
    claimed: &mut Option<(String, String, bool)>,
) -> Result<SpawnOutcome, String> {
    let st = state(&app);
    // The PTY namespace key — the renderer's terminal `id` ("pty-<agentId>").
    // Upstream ptyManager.spawn refuses a duplicate id with "pty already
    // exists for id" — the restore flow matches on that exact text.
    let pty_id = plan.pty_id.clone().unwrap_or_else(|| {
        format!(
            "{}-{}",
            args.name.to_lowercase().replace(' ', "-"),
            &uuid::Uuid::new_v4().simple().to_string()[..4]
        )
    });
    if st.ptys.lock().unwrap().contains_key(&pty_id) {
        return Err(format!("pty already exists for id {pty_id}"));
    }
    // The registry/AGENT_ID namespace (upstream hive.id). A hive-less spawn
    // collapses to the pty id — the port's pre-split behaviour for internal
    // worker/voice spawns.
    let agent_id = plan
        .agent_id
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| pty_id.clone());
    // Remember whether this id is brand new BEFORE anything registers it —
    // the failure cleanup only deletes rows this call created.
    let fresh = !st.hive.registry().iter().any(|a| a.id == agent_id);
    *claimed = Some((pty_id.clone(), agent_id.clone(), fresh));

    // Git isolation (index.ts:2730): when requested and the cwd is a real
    // repo, give the agent its own `agent/<id>` worktree under
    // <harness>/worktrees/ — it can't clobber other agents' (or the user's)
    // working tree. Best-effort: a failure falls back to the shared cwd.
    let mut effective_cwd = args.cwd.clone();
    if args.isolate && crate::git_ops::is_repo(std::path::Path::new(&args.cwd)) {
        // Upstream worktree seg is `opts.hive?.id ?? opts.id` — the AGENT id.
        let seg = worktree_seg(&agent_id);
        let wt_root = st
            .hive
            .root
            .parent()
            .unwrap_or(&st.hive.root)
            .join("worktrees");
        let wt_path = wt_root.join(&seg);
        // Path-safety: the resolved worktree path must stay under wt_root.
        if wt_path.starts_with(&wt_root) {
            let base = crate::git_ops::branch(std::path::Path::new(&args.cwd))
                .and_then(|b| worktree_base_from_branch(&b))
                .unwrap_or_else(|| "HEAD".into());
            let branch = crate::git_ops::agent_branch_for(&wt_path);
            match crate::git_ops::add_worktree(
                std::path::Path::new(&args.cwd),
                &wt_path,
                Some(&branch),
                Some(&base),
            ) {
                Ok(_) => {
                    effective_cwd = wt_path.display().to_string();
                    // worktreePaths is keyed by PTY id upstream (opts.id) —
                    // teardown runs through the PTY namespace.
                    st.worktree_paths
                        .lock()
                        .unwrap()
                        .insert(pty_id.clone(), (wt_path.clone(), PathBuf::from(&args.cwd)));
                    if let Err(e) = crate::git_ops::link_worktree_deps(
                        std::path::Path::new(&args.cwd),
                        &wt_path,
                    ) {
                        log::warn!("[worktree] dependency link failed: {e}");
                    }
                }
                Err(e) => log::warn!("[worktree] addWorktree failed: {e}"),
            }
        }
    }

    // Per-agent ShardX browser identity — armed by global consent or this
    // spawn's mcp_enable (disable wins), never silently. The PROFILE NAME is
    // the account contract: an unset spawn field auto-derives office-<id>
    // (one agent, one identity); a bound name shares ONE account profile
    // across sibling agents on purpose — the right isolation unit is the
    // account, not the agent. Runs BEFORE register so the binding lands in
    // `extra` and respawns/cleanup can find it.
    let agent_dir = st.hive.agent_dir(&agent_id);
    let is_claude = args.provider == "claude";
    // ShardX can arm only providers whose config we write mcp servers into —
    // claude's --settings and hermes' config.yaml today.
    let shardx_capable = matches!(args.provider.as_str(), "claude" | "hermes");
    let shardx_armed = shardx_capable && {
        let consented = st
            .config
            .lock()
            .unwrap()
            .mcp_defaults
            .get("shardx")
            .map(|m| m.enabled)
            .unwrap_or(false);
        (consented || args.mcp_enable.iter().any(|i| i == "shardx"))
            && !args.mcp_disable.iter().any(|i| i == "shardx")
    };
    let shardx_name = args
        .shardx_profile
        .clone()
        .unwrap_or_else(|| shardx_profile_name(&agent_id));
    let shardx_profile = if shardx_armed {
        shardx_ensure_profile(&agent_id, &shardx_name, &st.hive.root)
    } else {
        None
    };
    // Consent said yes but the local gates aren't met — vendored bridge or
    // stored token missing. needsShardx tells the renderer to offer the
    // one-click setup instead of fielding an agent that can't find its tools.
    let needs_shardx = shardx_armed
        && !shardx_setup_ready(&st.hive.root, st.config_store.dir());

    let entry = RegistryEntry {
        id: agent_id.clone(),
        name: args.name.clone(),
        provider: args.provider.clone(),
        role: args.role.clone().unwrap_or_default(),
        capabilities: args.capabilities.clone(),
        cwd: effective_cwd.clone(),
        is_god: args.is_god,
        is_assistant: args.is_assistant,
        status: "idle".into(),
        session_id: None,
        command: args.command.clone(),
        // Port-only bookkeeping rides `extra` (RegistryEntry has no typed
        // fields): the picked model re-seeds flag-less providers (openclaw),
        // mcpEnable/Disable keep a respawn's per-agent consent, and
        // shardxProfile binds this agent to its browser identity (`auto`
        // marks the ones WE created — only those are cleanup-eligible).
        extra: {
            let mut e = serde_json::Map::new();
            if let Some(m) = &args.model {
                e.insert("model".into(), serde_json::json!(m));
            }
            if !args.mcp_enable.is_empty() {
                e.insert("mcpEnable".into(), serde_json::json!(args.mcp_enable));
            }
            if !args.mcp_disable.is_empty() {
                e.insert("mcpDisable".into(), serde_json::json!(args.mcp_disable));
            }
            if shardx_armed {
                e.insert(
                    "shardxProfile".into(),
                    serde_json::json!({
                        "name": shardx_name,
                        "id": shardx_profile,
                        "auto": args.shardx_profile.is_none(),
                    }),
                );
            }
            e
        },
        created_at: chrono::Utc::now().to_rfc3339(),
        ..Default::default()
    };
    // ensureAgent semantics run on EVERY spawn (upstream pty:spawn): a fresh
    // hire gets the full bootstrap; a respawn re-merges — refreshed
    // identity.md, re-validated cwd, cleared archived, preserved
    // sessionId/onHold/extra, mailbox dirs re-created.
    st.hive.register_agent(&entry).map_err(err_str)?;

    // Per-agent hive identity env (hive.ts:803) layered over the inherited env
    // minus parent-session markers (ptyEnv.ts).
    let auto = st.config.lock().unwrap().auto_mode;
    let mut agent_env = HashMap::from([
        ("AGENT_ID".to_string(), agent_id.clone()),
        ("AGENT_NAME".to_string(), args.name.clone()),
        ("HIVE_ROOT".to_string(), st.hive.root.display().to_string()),
        ("AGENT_DIR".to_string(), agent_dir.display().to_string()),
    ]);
    if auto {
        agent_env.insert("HIVE_AUTO_APPROVE".into(), "1".into());
    }
    if let Some(sock) = &st.sock_addr {
        agent_env.insert("HIVE_SOCK".into(), sock.clone());
        // Per-run secret the hook server requires on every envelope.
        agent_env.insert("HIVE_TOKEN".into(), crate::hooks::hook_token().to_string());
    }
    // Integration broker token (integrationBroker.ts grant(workerId,
    // enabledIds)) — MD_BROKER_URL/MD_BROKER_TOKEN env for worker spawn;
    // no token is granted when no integrations are enabled.
    {
        let home = st.hive.root.parent().unwrap_or(&st.hive.root).to_path_buf();
        let ids = crate::integrations::enabled_ids(&home);
        if !ids.is_empty() {
            // grant mints (or rotates) the worker's capability token.
            let token = crate::integrations::BROKER.grant(&agent_id, ids);
            agent_env.insert("MD_BROKER_URL".into(), crate::integrations::BROKER.url());
            agent_env.insert("MD_BROKER_TOKEN".into(), token);
        }
        // MemPalace env (memory.ts) — the worker sees the shared palace
        // path so `mempalace` calls inside the agent land in the same store.
        for (k, v) in crate::memory_palace::instance(&home).env() {
            agent_env.insert(k, v);
        }
    }
    // OTel wiring (hive.ts:983-989 — claude only): metrics/logs pushed as
    // OTLP/HTTP-JSON to the embedded collector, tagged by agent.id.
    // Keychain secrets configured for injection (config.secret_env_keys) —
    // values come out of the OS credential store at spawn, never from disk.
    // PROVIDER_KEY_<backend> entries map to the env var the CLIs actually read
    // (upstream BACKEND_KEY_ENV): without the translation a saved "OpenAI" key
    // reached the agent as PROVIDER_KEY_OPENAI — a name nothing consumes.
    // Reads go through the persist-aware getter — keys a flaky CredMan dropped
    // are still found in the DPAPI file store.
    for key in &st.config.lock().unwrap().secret_env_keys {
        if let Some(v) = crate::misc::secret_get_persist(key, st.config_store.dir()) {
            match crate::misc::provider_env_names(key) {
                Some(envs) => {
                    for e in envs {
                        agent_env.insert(e.to_string(), v.clone());
                    }
                }
                None => {
                    agent_env.insert(key.clone(), v);
                }
            }
        }
    }
    // Codex lifecycle bridge — a per-agent CODEX_HOME carrying our hook
    // tables + the user's auth, so a codex worker gets the full hive pipeline
    // (status, inbox drain, HITL deny, wake) without touching the user's
    // global config. Same shim; codex's payload is already claude-shaped.
    // W3 bundled read-only skills → <dir>/.claude/skills/ on every spawn
    // (same policy as identity.md — overwritten so the shipped set tracks
    // the app; missing/empty source is a no-op, never a spawn blocker).
    {
        if let Some(src) = crate::skills::bundled_skills_dir() {
            let _ = crate::hive::copy_dir(&src, &agent_dir.join(".claude").join("skills"));
        }
    }
    if args.provider == "codex" {
        if let Some(bin) = &st.hook_bin {
            if let Ok(home) = st.hive.install_codex_hooks(&agent_dir, bin) {
                agent_env.insert("CODEX_HOME".into(), home.display().to_string());
            }
        }
        // Rollout-dir exposure: sessions/archived_sessions surface under the
        // user's ~/.codex scan root (junction back to the worker home).
        st.hive.expose_codex_data(&agent_dir, &agent_id);
    }
    // Hermes lifecycle bridge — per-agent HERMES_HOME + config.yaml `hooks:`
    // block on the same shim (--hermes mode). Runtime dir is isolated too so
    // multiplex state never collides with the user's own hermes.
    let mut needs_key: Option<String> = None;
    if args.provider == "hermes" {
        // Python UTF-8 mode — hermes writes tool/MCP/hook stdin through
        // subprocess text pipes that default to the Windows ANSI codepage
        // (cp1252). Any non-ASCII in the payload (our workspace notes use →)
        // crashes its writer thread with UnicodeEncodeError. PYTHONUTF8=1
        // forces utf-8 on every pipe regardless of the machine's locale.
        agent_env.insert("PYTHONUTF8".into(), "1".into());
        agent_env.insert("PYTHONIOENCODING".into(), "utf-8".into());
        if let Some(bin) = &st.hook_bin {
            let seed = {
                let cfg = st.config.lock().unwrap();
                let root = st.config_store.dir().to_path_buf();
                let mut seed =
                    hermes_seed_plan(&cfg, &|k| crate::misc::secret_get_persist(k, &root));
                // Hermes reads mcp_servers from its own config.yaml — same
                // consent merge as claude's --settings (catalog ∩ defaults ∩
                // per-spawn lists), secrets resolved through the same store.
                let mcp_defaults: std::collections::HashMap<String, bool> = cfg
                    .mcp_defaults
                    .iter()
                    .map(|(k, v)| (k.clone(), v.enabled))
                    .collect();
                seed.mcp_servers = crate::mcp::build_default_mcp_servers(
                    &effective_cwd,
                    &st.hive.root,
                    &mcp_defaults,
                    &args.mcp_enable,
                    &args.mcp_disable,
                    &|key| crate::misc::secret_get_persist(key, &root),
                );
                // Same SHARDX_PROFILE env pin the claude path adds.
                if let (Some(spec), Some(pid)) =
                    (seed.mcp_servers.get_mut("agent-office-shardx"), shardx_profile.clone())
                {
                    spec.env.insert("SHARDX_PROFILE".into(), pid);
                }
                // config.yaml gets `${AGENTOFFICE_MCP_*}` refs (hermes expands ${VAR}
                // in mcp_servers from its process env); values ride agent_env.
                agent_env.extend(crate::mcp::externalize_mcp_secrets(&mut seed.mcp_servers));
                seed
            };
            if let Ok(home) = st.hive.install_hermes_hooks_seeded(&agent_dir, bin, &seed) {
                agent_env.insert("HERMES_HOME".into(), home.display().to_string());
                agent_env.insert(
                    "HERMES_RUNTIME_DIR".into(),
                    home.join("runtime").display().to_string(),
                );
                agent_env.insert("HERMES_ACCEPT_HOOKS".into(), "1".into());
            }
            // Authless detection — extracted so the 401 surface is pinned by
            // tests (see hermes_needs_key).
            if hermes_needs_key(&seed, &agent_env) {
                needs_key = Some("hermes".to_string());
            }
        }
        // OpenAI-compatible reroute — Settings → Agents & Models → Hermes base
        // URL lands here as OPENAI_BASE_URL so the agent hits DeepSeek/Ollama/
        // vLLM instead of api.openai.com (hermes reads it natively).
        let env_pair = {
            let cfg = st.config.lock().unwrap();
            crate::misc::openai_base_url_env(&cfg, "hermes")
        };
        if let Some((k, v)) = env_pair {
            agent_env.insert(k, v);
        }
    }
    // OpenClaw bridge — `openclaw tui --local` has NO --model flag, so the
    // UI-picked model rides a per-agent openclaw.json (OPENCLAW_CONFIG_PATH)
    // instead of argv. The same file carries a custom endpoint as a
    // models.providers."office-compat" entry; OPENAI_BASE_URL goes in as env
    // too (openai-routing honors it on the openai-compat route), and provider
    // API keys already ride in via secret_env_keys above.
    if args.provider == "openclaw" {
        let (env_pair, base_url, model) = {
            let cfg = st.config.lock().unwrap();
            (
                crate::misc::openai_base_url_env(&cfg, "openclaw"),
                cfg.provider_base_urls.get("openclaw").cloned(),
                args.model
                    .clone()
                    .or_else(|| cfg.provider_default_models.get("openclaw").cloned()),
            )
        };
        if let Some((k, v)) = env_pair {
            agent_env.insert(k, v);
        }
        match crate::misc::seed_openclaw_config(
            &agent_dir,
            base_url.as_deref(),
            model.as_deref(),
        ) {
            Ok(Some(path)) => {
                agent_env.insert("OPENCLAW_CONFIG_PATH".into(), path.display().to_string());
            }
            Ok(None) => {}
            Err(e) => log::warn!("[openclaw] config seed failed: {e}"),
        }
    }
    // Gemini lifecycle bridge — per-agent system-settings.json bound via
    // GEMINI_CLI_SYSTEM_SETTINGS_PATH; the shim's --gemini mode translates.
    if args.provider == "gemini" {
        if let Some(bin) = &st.hook_bin {
            if let Ok(p) = st.hive.install_gemini_hooks(&agent_dir, bin) {
                agent_env.insert(
                    "GEMINI_CLI_SYSTEM_SETTINGS_PATH".into(),
                    p.display().to_string(),
                );
            }
        }
    }
    // Grok bridge — GLOBAL hook file under ~/.grok/hooks (trusted dir; the
    // shim no-ops outside hive workers via the AGENT_ID/HIVE_SOCK check).
    if args.provider == "grok" {
        if let Some(bin) = &st.hook_bin {
            let _ = st.hive.install_grok_hooks(bin);
        }
    }
    // Antigravity (`agy`) bridge — global hook groups under ~/.gemini,
    // runtime-scoped to hive workers by the shim's AGENT_ID check.
    if args.provider == "antigravity" {
        if let Some(bin) = &st.hook_bin {
            let _ = st.hive.install_agy_hooks(bin);
        }
    }
    // Pi / OpenCode — bundled-file bridges (extensions + plugins posting to
    // HIVE_SOCK directly); per-agent dirs isolate from the user's globals.
    if args.provider == "pi" {
        if let Ok(home) = crate::bridges::install_pi_hooks(&agent_dir) {
            agent_env.insert("PI_CODING_AGENT_DIR".into(), home.display().to_string());
        }
    }
    if args.provider == "opencode" {
        if let Ok(home) = crate::bridges::install_opencode_plugin(&agent_dir) {
            agent_env.insert("OPENCODE_CONFIG_DIR".into(), home.display().to_string());
        }
    }
    // Proxy-tier bridges (qwen, crush): a loopback ProxyBridge tees the
    // agent's LLM traffic into the same hook payloads — Status/CostSample/
    // PostToolUse/Stop without any hook surface at all. The upstream is the
    // user's REAL base URL — the provider's providerBaseUrls override first
    // (this is how a DeepSeek key + api.deepseek.com reach a BYOK CLI), then
    // the parent env, then OpenAI itself (upstream index.ts:2766).
    if args.provider == "qwen" || args.provider == "crush" {
        if let Some(sock) = &st.sock_addr {
            let upstream = st
                .config
                .lock()
                .unwrap()
                .provider_base_urls
                .get(args.provider.as_str())
                .cloned()
                .or_else(|| std::env::var("OPENAI_BASE_URL").ok())
                .unwrap_or_else(|| "https://api.openai.com".into());
            let bridge = std::sync::Arc::new(crate::proxy::ProxyBridge::new(
                agent_id.clone(),
                crate::proxy::ProxyApi::OpenAi,
                upstream,
                sock.clone(),
                None,
            ));
            match bridge.clone().start() {
                Ok(port) => {
                    st.proxy_bridges
                        .lock()
                        .unwrap()
                        .insert(agent_id.clone(), bridge);
                    let loopback = format!("http://127.0.0.1:{port}");
                    if args.provider == "qwen" {
                        agent_env.insert("OPENAI_BASE_URL".into(), loopback);
                    } else if let Ok((cfg, data)) =
                        crate::bridges::install_crush_config(&agent_dir, &loopback, true)
                    {
                        agent_env.insert("CRUSH_GLOBAL_CONFIG".into(), cfg.display().to_string());
                        agent_env.insert("CRUSH_GLOBAL_DATA".into(), data.display().to_string());
                    }
                }
                Err(e) => {
                    // Upstream proxy-bind failure (hive.ts): log proxy-degraded
                    // + emit hive:degraded — the agent still spawns, just
                    // without hive events.
                    let msg = format!(
                        "proxy bridge failed to bind — agent spawns without hive events: {e}"
                    );
                    let _ = st.hive.log(&serde_json::json!({
                        "kind": "proxy-degraded", "agentId": agent_id,
                        "name": args.name, "provider": args.provider,
                    }));
                    let _ = app.emit(
                        "hive:degraded",
                        serde_json::json!({
                            "agentId": agent_id, "name": args.name,
                            "reason": "proxy-bind", "message": msg,
                        }),
                    );
                }
            }
        }
    }
    if is_claude {
        if let Some(ep) = st.otel_endpoint.get() {
            agent_env.insert("CLAUDE_CODE_ENABLE_TELEMETRY".into(), "1".into());
            agent_env.insert("OTEL_METRICS_EXPORTER".into(), "otlp".into());
            agent_env.insert("OTEL_LOGS_EXPORTER".into(), "otlp".into());
            agent_env.insert("OTEL_EXPORTER_OTLP_PROTOCOL".into(), "http/json".into());
            agent_env.insert("OTEL_EXPORTER_OTLP_ENDPOINT".into(), ep.clone());
            agent_env.insert("OTEL_METRIC_EXPORT_INTERVAL".into(), "5000".into());
            agent_env.insert("OTEL_LOGS_EXPORT_INTERVAL".into(), "2000".into());
            agent_env.insert(
                "OTEL_RESOURCE_ATTRIBUTES".into(),
                format!("agent.id={},agent.name={}", entry.id, entry.name),
            );
        }
    }

    // The ensured ShardX profile rides the agent's own env as $SHARDX_PROFILE
    // plus a SHARDX.md note in its dir — the agent-visible contract for WHICH
    // browser identity it owns.
    if shardx_armed {
        if let Some(pid) = &shardx_profile {
            agent_env.insert("SHARDX_PROFILE".into(), pid.clone());
        }
        let _ = std::fs::write(
            agent_dir.join("SHARDX.md"),
            shardx_workspace_note(&shardx_name, shardx_profile.as_deref(), &st.hive.root),
        );
    }

    let parent: HashMap<String, String> = std::env::vars().collect();
    // withHiveRuntimeFallback: the hive's bundled-runtime dir (if any) is
    // APPENDED to the child PATH so provider subprocesses that shell out to
    // `node` work even on machines without a system install.
    // Use the login-shell PATH (shellEnv.ts userShellPath) — a GUI-launched
    // app on macOS inherits no nvm/homebrew PATH, so a bare `claude` would
    // ENOENT without this.
    let mut path = crate::pty::user_shell_path();
    let runtime_dir = st.hive.root.join("bin").join("runtime");
    if runtime_dir.is_dir()
        && !path
            .split(if cfg!(windows) { ';' } else { ':' })
            .any(|d| d == runtime_dir.display().to_string())
    {
        if !path.is_empty() {
            path.push(if cfg!(windows) { ';' } else { ':' });
        }
        path.push_str(&runtime_dir.display().to_string());
    }
    let mut env = build_pty_env(&parent, &path, Some(&agent_env), cfg!(windows));

    // cwd: expand `~` first (existsSync never expands it — pty.ts:559), then
    // upstream's hard error on a missing workspace rather than a silent
    // reroute; an EMPTY cwd falls back to the agent's own folder.
    let cwd = if effective_cwd.is_empty() {
        agent_dir.display().to_string()
    } else {
        let expanded = crate::misc::expand_tilde(&effective_cwd);
        if !std::path::Path::new(&expanded).is_dir() {
            return Err(format!("cwd does not exist: {expanded}"));
        }
        expanded
    };

    // ── hive wiring per provider class (hive.ts spawnAgentCore, condensed) ──
    // hiveAware (claude): per-session settings.json + --append-system-prompt.
    // Others: the same prompt rides in as the typed first turn — the uniform
    // 'type-into-tui' delivery, which also covers flag/positional providers.
    let prompt_opts = crate::hive::PromptOpts {
        semantic_memory: crate::memory_palace::instance(&st.hive.root).available(),
        knowledge_graph: crate::kg::status(&st.hive.root)["docs"]
            .as_u64()
            .unwrap_or(0)
            > 0,
        kg_cli_path: Some(
            st.hive
                .root
                .join("bin")
                .join("kg.cjs")
                .display()
                .to_string(),
        ),
        hive_node: st
            .hive
            .root
            .join("bin")
            .join("runtime")
            .join(if cfg!(windows) { "node.exe" } else { "node" })
            .display()
            .to_string(),
        orchestrator_may_spawn: true,
        runtime: Some(crate::hive::RuntimeInfo {
            version: env!("CARGO_PKG_VERSION").into(),
            packaged: !cfg!(debug_assertions),
            app_path: std::env::current_exe()
                .ok()
                .map(|p| p.display().to_string()),
        }),
    };
    let mut injected = st.hive.injected_prompt(&entry, &prompt_opts);
    if shardx_armed {
        injected.push_str(&format!(
            "\n\nShardX browser (armed): your dedicated launcher profile is `{shardx_name}`{}\
             — pass it to every shardx profile/browser tool call and never start or reuse \
             another profile (linked-account bans). Details: SHARDX.md in your agent dir, \
             or $SHARDX_PROFILE on your env.",
            shardx_profile
                .as_ref()
                .map(|pid| format!(" (id `{pid}`)"))
                .unwrap_or_default()
        ));
    }
    let mut command_line = args.command.clone();
    let mut seed = args.seed_prompt.clone();

    if is_claude {
        if let (Some(_sock), Some(bin)) = (&st.sock_addr, &st.hook_bin) {
            let shim_cmd = format!("\"{}\"", bin.display());
            // W3 MCP merge — catalog ∩ consent (global mcpDefaults + per-spawn
            // mcp_enable/disable), secrets resolved from the keychain so a
            // token never lands in settings.json plaintext.
            let mcp_defaults: std::collections::HashMap<String, bool> = st
                .config
                .lock()
                .unwrap()
                .mcp_defaults
                .iter()
                .map(|(k, v)| (k.clone(), v.enabled))
                .collect();
            let mut mcp_servers = crate::mcp::build_default_mcp_servers(
                &cwd,
                &st.hive.root,
                &mcp_defaults,
                &args.mcp_enable,
                &args.mcp_disable,
                // Same persist-aware getter as the hermes path — a key a flaky
                // CredMan dropped is still found in the DPAPI file store.
                // Values are swapped for `${AGENTOFFICE_MCP_*}` refs below and ride
                // the spawned process env instead of settings.json.
                &|key| crate::misc::secret_get_persist(key, st.config_store.dir()),
            );
            // The agent's own launcher profile id rides the MCP server's env
            // too — the server ignores it today, but a future default-profile
            // flag picks it up for free.
            if let (Some(spec), Some(pid)) =
                (mcp_servers.get_mut("agent-office-shardx"), shardx_profile.clone())
            {
                spec.env.insert("SHARDX_PROFILE".into(), pid);
            }
            // Secrets never touch disk: refs in the file, values in the env.
            env.extend(crate::mcp::externalize_mcp_secrets(&mut mcp_servers));
            let settings = st.hive.hook_settings(&shim_cmd, &mcp_servers);
            let settings_path = agent_dir.join("settings.json");
            let _ = std::fs::write(
                &settings_path,
                serde_json::to_string_pretty(&settings).unwrap_or_default(),
            );
            command_line = format!(
                "{} --settings {}",
                command_line,
                pty::cmd_quote(&settings_path.display().to_string())
            );
        }
    } else if seed.is_none() {
        seed = Some(injected.clone());
    }

    // Resolve the program for a direct-argv spawn (exe) vs cmd-wrap (scripts).
    let first_tok = crate::misc::tokenize_command(&command_line)
        .into_iter()
        .next()
        .unwrap_or_default();

    // Missing-CLI auto-install (spawnAgentCore → pendingInstallRelaunch): the
    // provider binary can't be resolved — run the bundled installer in the
    // same terminal slot, then re-run THIS spawn on exit-0 (with
    // no_auto_install so the ladder never loops). A provider with no installer
    // spec, or a machine where no rung exists, falls through to the normal
    // spawn and dies on its own missing-binary error — same as upstream.
    if !args.no_auto_install && pty::resolve_program(&first_tok, &path).is_none() {
        let npm_available = pty::resolve_program("npm", &path).is_some();
        if let Some((script, relaunch)) =
            crate::misc::install_script(&args.provider, &first_tok, npm_available)
        {
            // Install rail: the PTY id stays this slot's key, but the
            // ptyToAgent mapping stays UNSET for it (upstream does the same —
            // an install PTY has no agent/worktree teardown of its own).
            let handle = pty::spawn(
                &pty_id,
                &script,
                &cwd,
                &env,
                args.cols,
                args.rows,
                app.clone(),
            )
            .map_err(err_str)?;
            // Arm auto restart-and-continue AFTER the PTY actually spawned
            // (upstream: `res.ok && rung.command`) — a failed spawn must not
            // leave a stale relaunch arm on a reusable pty id. The relaunch
            // keeps THIS spawn's identity + resume plan; the seed is typed
            // server-side on relaunch since no renderer consumed this result
            // (upstream discards it the same way).
            if relaunch {
                let mut retry = args.clone();
                retry.no_auto_install = true;
                let mut retry_plan = plan.clone();
                retry_plan.auto_type_seed = true;
                PENDING_INSTALLS.lock().unwrap().insert(
                    pty_id.clone(),
                    PendingInstall {
                        args: retry,
                        plan: retry_plan,
                    },
                );
            }
            let spawn_gen = handle.spawn_gen;
            st.ptys.lock().unwrap().insert(pty_id.clone(), handle);
            st.hive.set_status(&agent_id, "idle").map_err(err_str)?;
            // A child that already exited ran its teardown before the insert.
            pty::reap_if_already_exited(&app, &pty_id, spawn_gen);
            return Ok(SpawnOutcome {
                pty_id,
                cwd,
                seed_prompt: None,
                resumed: false,
                resume_not_found: false,
                worktree_path: None,
                needs_key,
                needs_shardx,
            });
        }
    }

    // Resume (upstream index.ts:2871 / Add-Agent #2): an explicit
    // resumeSessionId wins; `resume: true` reads the registry's recorded
    // session for THIS agent id. Claude gates --resume on the transcript
    // actually reaching this cwd's project dir (upstream seedSessionTranscript)
    // — a typed id that can't be found falls back to a FRESH session flagged
    // resumeNotFound. Other providers keep the port's existing `--resume`
    // pass-through (upstream uses the preset's per-provider resume flag —
    // PORTING.md notes the divergence).
    let sid = match &plan.resume {
        Some(ResumeRequest::Explicit(s)) => Some(s.clone()),
        Some(ResumeRequest::LastSession) => st.hive.last_session(&agent_id),
        None => None,
    };
    let mut resume_not_found = false;
    let resume_session = match sid {
        Some(sid) if is_claude => {
            if crate::usage::seed_session_transcript(&cwd, &sid) {
                Some(sid)
            } else {
                // A session id existed but no transcript could reach this
                // cwd's project dir — fresh session instead. Upstream flags
                // resumeNotFound ONLY for an explicitly TYPED id (the Add
                // Agent field surfaces it); a stale registry session just
                // falls through quietly.
                resume_not_found = matches!(plan.resume, Some(ResumeRequest::Explicit(_)));
                None
            }
        }
        other => other,
    };
    // Upstream requireResume (Restart & Continue): refuse outright instead of
    // quietly starting a fresh session — the caller killed the old PTY first
    // and would rather keep the card dead than lose the conversation.
    if plan.require_resume && plan.resume.is_some() && resume_session.is_none() {
        return Err(
            "Existing session could not be resumed; no replacement process was started.".into(),
        );
    }
    let resumed = resume_session.is_some();

    let handle = match pty::resolve_program(&first_tok, &path) {
        Some(pty::Resolved::Exe(prog)) => {
            let mut av: Vec<String> = crate::misc::tokenize_command(&command_line)
                .into_iter()
                .skip(1)
                .collect();
            if is_claude {
                av.push("--append-system-prompt".into());
                av.push(injected.clone());
            }
            if let Some(sid) = &resume_session {
                av.push("--resume".into());
                av.push(sid.clone());
            }
            pty::spawn_argv(
                &pty_id,
                &prog,
                &av,
                &cwd,
                &env,
                args.cols,
                args.rows,
                app.clone(),
            )
        }
        Some(pty::Resolved::Script(prog)) => {
            // resolveWindowsShimSpawn (pty.ts): decode the .cmd shim to its
            // real interpreter + script and spawn argv — cmd.exe cannot carry
            // the multi-line hive prompt (newlines + parens, ~8191-char cap).
            // Every miss falls through to the cmd route below.
            if let Some((exe, script)) =
                pty::resolve_windows_shim_spawn(&prog, |n| pty::resolve_program(n, &path))
            {
                let mut av: Vec<String> = script
                    .map(|s| vec![s.display().to_string()])
                    .unwrap_or_default();
                av.extend(
                    crate::misc::tokenize_command(&command_line)
                        .into_iter()
                        .skip(1)
                        .map(String::from),
                );
                if is_claude {
                    av.push("--append-system-prompt".into());
                    av.push(injected.clone());
                }
                if let Some(sid) = &resume_session {
                    av.push("--resume".into());
                    av.push(sid.clone());
                }
                pty::spawn_argv(
                    &pty_id,
                    &exe,
                    &av,
                    &cwd,
                    &env,
                    args.cols,
                    args.rows,
                    app.clone(),
                )
            } else {
                // Fallback when the shim can't be decoded: route through cmd.exe
                // as RAW tokens — portable-pty CRT-quotes each arg, which is the
                // same line shape upstream's buildCmdCommandLine emits verbatim.
                // (cmd_sanitize still applies: cmd cannot carry a literal newline
                // at all — upstream truncates at it; collapsing is strictly less
                // destructive.)
                let mut parts = vec![prog.display().to_string()];
                parts.extend(
                    crate::misc::tokenize_command(&command_line)
                        .into_iter()
                        .skip(1)
                        .map(String::from),
                );
                if is_claude {
                    parts.push("--append-system-prompt".into());
                    parts.push(pty::cmd_sanitize(&injected));
                }
                if let Some(sid) = &resume_session {
                    parts.push("--resume".into());
                    parts.push(sid.clone());
                }
                pty::spawn_cmd(&pty_id, &parts, &cwd, &env, args.cols, args.rows, app.clone())
            }
        }
        None => {
            let mut parts: Vec<String> = crate::misc::tokenize_command(&command_line)
                .into_iter()
                .map(String::from)
                .collect();
            if is_claude {
                parts.push("--append-system-prompt".into());
                parts.push(pty::cmd_sanitize(&injected));
            }
            if let Some(sid) = &resume_session {
                parts.push("--resume".into());
                parts.push(sid.clone());
            }
            pty::spawn_cmd(&pty_id, &parts, &cwd, &env, args.cols, args.rows, app.clone())
        }
    }
    .map_err(err_str)?;
    st.wake.lock().unwrap().note_spawn(
        &pty_id,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    );
    let spawn_gen = handle.spawn_gen;
    st.ptys.lock().unwrap().insert(pty_id.clone(), handle);
    // upstream ptyToAgent.set(opts.id, opts.hive.id) — every agent-keyed
    // lookup (hooks, breaker, watchdog, missions) resolves through this.
    st.map_pty(&pty_id, &agent_id);
    st.hive.set_status(&agent_id, "idle").map_err(err_str)?;
    // The waiter thread started inside pty::spawn, before the handle was
    // registered: a child that already exited (~1ms) ran its teardown against
    // nothing and would leave this dead handle + mapping behind forever.
    pty::reap_if_already_exited(&app, &pty_id, spawn_gen);

    // 'type-into-tui' seed delivery: upstream returns the seed to the
    // renderer, which types it via the write chain (seedPrompt → submitToPty).
    // Only the renderer-less paths (PENDING_INSTALLS relaunch, worker/voice/
    // respawn spawns) auto-type here — auto_type_seed marks those.
    if plan.auto_type_seed {
        if let Some(text) = &seed {
            let app2 = app.clone();
            let pid2 = pty_id.clone();
            let text = text.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                let Some(st) = app2.try_state::<AppState>() else {
                    return;
                };
                crate::pty::write_pty(
                    &st,
                    &pid2,
                    &format!(
                        "{}\r",
                        text.split_whitespace().collect::<Vec<_>>().join(" ")
                    ),
                );
            });
        }
    }

    let worktree_path = st
        .worktree_paths
        .lock()
        .unwrap()
        .get(&pty_id)
        .map(|(p, _)| p.display().to_string());
    let _ = app.emit(
        "agent-status",
        serde_json::json!({ "id": agent_id, "status": "idle" }),
    );
    Ok(SpawnOutcome {
        pty_id,
        cwd,
        seed_prompt: seed,
        resumed,
        resume_not_found,
        worktree_path,
        needs_key,
        needs_shardx,
    })
}

/// The upstream pty-manager `{ok, error?}` envelope — write/resize/kill all
/// RESOLVE this instead of rejecting (submitToPty reads `wrote.ok` and
/// Restart & Continue reads `killed.ok` + matches `no pty:`). Note the exact
/// upstream miss text: `no pty: <id>` (with the colon).
#[derive(Debug, Clone, Serialize)]
pub struct PtyOpResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl PtyOpResult {
    pub fn ok() -> Self {
        Self {
            ok: true,
            error: None,
        }
    }
    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(msg.into()),
        }
    }
}

#[tauri::command(async)]
pub fn pty_write(app: AppHandle, id: String, data: String) -> PtyOpResult {
    pty_write_core(&app, &id, &data)
}

/// `pty:write` (index.ts:3035) — upstream resolves `{ok:false,'no pty: id'}`
/// on a dead session; it never rejects (submitToPty reads `wrote.ok`).
pub(crate) fn pty_write_core<R: tauri::Runtime>(
    app: &AppHandle<R>,
    id: &str,
    data: &str,
) -> PtyOpResult {
    let st = state(app);
    // Clone the writer out and release the map lock first: a blocking write
    // into a wedged agent must not stall every other PTY operation.
    let writer = st.ptys.lock().unwrap().get(id).map(|h| h.writer());
    match writer {
        Some(w) => match w.write(data) {
            Ok(()) => PtyOpResult::ok(),
            Err(e) => PtyOpResult::err(err_str(e)),
        },
        None => PtyOpResult::err(format!("no pty: {id}")),
    }
}

#[tauri::command(async)]
pub fn pty_resize(app: AppHandle, id: String, cols: u16, rows: u16) -> PtyOpResult {
    pty_resize_core(&app, &id, cols, rows)
}

/// `pty:resize` (index.ts:3039) — same `{ok, error?}` envelope.
pub(crate) fn pty_resize_core<R: tauri::Runtime>(
    app: &AppHandle<R>,
    id: &str,
    cols: u16,
    rows: u16,
) -> PtyOpResult {
    let st = state(app);
    let ptys = st.ptys.lock().unwrap();
    match ptys.get(id) {
        Some(h) => match h.resize(cols, rows) {
            Ok(()) => PtyOpResult::ok(),
            Err(e) => PtyOpResult::err(err_str(e)),
        },
        None => PtyOpResult::err(format!("no pty: {id}")),
    }
}

#[tauri::command(async)]
pub fn kill_agent(app: AppHandle, id: String) -> PtyOpResult {
    kill_agent_core(&app, &id)
}

/// `pty:kill` (index.ts:3047) — kill the process, then run the shared
/// teardownPty. `id` may carry EITHER namespace: the renderer passes the PTY
/// id (`pty-<agent>`), internal callers (worker reaps, voice kill) pass agent
/// ids — resolve_pty_id accepts both.
pub(crate) fn kill_agent_core<R: tauri::Runtime>(app: &AppHandle<R>, id: &str) -> PtyOpResult {
    let st = state(app);
    let Some(pty_id) = st.resolve_pty_id(id) else {
        // Upstream still runs teardownPty on an unknown id (idempotent no-op)
        // and reports `no pty: <id>` — Restart & Continue matches that exact
        // text as "already gone, proceed with the respawn".
        teardown_pty_core(app, id, "gone");
        return PtyOpResult::err(format!("no pty: {id}"));
    };
    if let Some(mut h) = st.ptys.lock().unwrap().remove(&pty_id) {
        let _ = h.kill();
    }
    teardown_pty_core(app, &pty_id, "gone");
    PtyOpResult::ok()
}

/// teardownPty (index.ts:443) — the shared PTY lifecycle cleanup, run by BOTH
/// an explicit kill and the natural-exit handler. `pty_id` is the PTY
/// namespace key; every agent-keyed cleanup resolves the agent id through the
/// pty→agent map first (an unmapped PTY — install rail or legacy — collapses
/// to the pty id, where the agent-keyed steps harmlessly no-op). Idempotent:
/// a double call is a harmless no-op.
pub(crate) fn teardown_pty_core<R: tauri::Runtime>(app: &AppHandle<R>, pty_id: &str, status: &str) {
    let st = state(app);
    // Upstream `ptyToAgent.get(id)` — the agent id this PTY owned. Resolved
    // FIRST: the map drop below is exactly what makes the mapping unreadable
    // after teardown (same ordering hazard upstream documents).
    let agent_id = st.unmap_pty(pty_id).unwrap_or_else(|| pty_id.to_string());
    // Dead agents can't leak a tripped breaker level or a pending steer.
    st.breaker.lock().unwrap().forget(&agent_id);
    st.wake.lock().unwrap().forget(&agent_id, Some(pty_id));
    st.telemetry.forget(&agent_id);
    // A dead worker's broker capability token dies with it
    // (integrationBroker.ts revoke-on-teardown).
    crate::integrations::BROKER.revoke(&agent_id);
    // Kill this agent's proxy-bridge sidecar (qwen/crush) — a dead PTY never
    // leaves an orphan loopback listener (teardownPty stopProxyBridge).
    if let Some(bridge) = st.proxy_bridges.lock().unwrap().remove(&agent_id) {
        bridge.stop();
    }
    // teardownPty step 2: worker/live tracking + the isolated-worktree gate
    // (workers: preserve unintegrated work + defer scratch GC; regular:
    // upstream's immediate force-remove). worktree_paths is pty-keyed,
    // live_workers agent-keyed — hence the two ids.
    let was_worker = st.live_workers.lock().unwrap().contains_key(&agent_id);
    crate::workers::finalize_teardown(&st, pty_id, &agent_id);
    if was_worker {
        // Mirrors teardownPty's hive:agentArchived broadcast — the dead
        // worker's floor card comes down on every teardown route.
        let _ = app.emit("hive:agentArchived", serde_json::json!({ "id": agent_id }));
    }
    let _ = st.hive.set_status(&agent_id, status);
    let _ = app.emit(
        "agent-status",
        serde_json::json!({ "id": agent_id, "status": status }),
    );
}

// ─── hive surface ───────────────────────────────────────────────────────────

/// hive:registry — upstream returns the registry DOCUMENT `{godId, agents:{
/// <id>: entry}}` so the renderer can key by id and reach godId.
#[tauri::command(async)]
pub fn hive_registry(app: AppHandle) -> Result<serde_json::Value, String> {
    let (god_id, agents) = state(&app).hive.read_registry_doc();
    let map: serde_json::Map<String, serde_json::Value> = agents
        .into_iter()
        .filter_map(|a| serde_json::to_value(&a).ok().map(|v| (a.id.clone(), v)))
        .collect();
    Ok(serde_json::json!({ "godId": god_id, "agents": map }))
}

/// hive:send — upstream's exact contract: `{ok, message}` where the partial
/// is normalized and routed SYNCHRONOUSLY (index.ts:3503 — no outbox hop, so
/// virtual senders like 'human'/'system' never materialize an agents/<from>
/// dir).
#[tauri::command(async)]
pub fn hive_send(
    app: AppHandle,
    partial: crate::hive::PartialMessage,
    from: Option<String>,
) -> Result<serde_json::Value, String> {
    let sender = from.unwrap_or_else(|| "system".into());
    let msg = state(&app)
        .hive
        .send_partial(&partial, &sender)
        .map_err(err_str)?;
    Ok(serde_json::json!({ "ok": true, "message": msg }))
}

#[tauri::command(async)]
pub fn hive_inbox(app: AppHandle, agent_id: String) -> Result<Vec<Message>, String> {
    state(&app).hive.inbox(&agent_id).map_err(err_str)
}

#[tauri::command(async)]
pub fn hive_mark_done(app: AppHandle, agent_id: String, msg_id: String) -> Result<(), String> {
    state(&app)
        .hive
        .mark_done(&agent_id, &msg_id)
        .map_err(err_str)
}

#[tauri::command(async)]
pub fn hive_board(app: AppHandle) -> Result<String, String> {
    state(&app).hive.read_board().map_err(err_str)
}

#[tauri::command(async)]
pub fn hive_tasks(app: AppHandle) -> Result<serde_json::Value, String> {
    state(&app).hive.read_tasks().map_err(err_str)
}

// ─── memory ─────────────────────────────────────────────────────────────────

#[tauri::command(async)]
pub fn read_memory(app: AppHandle, agent_id: String) -> Result<String, String> {
    state(&app).hive.read_memory(&agent_id).map_err(err_str)
}

#[tauri::command(async)]
pub fn append_memory(app: AppHandle, agent_id: String, entry: String) -> Result<(), String> {
    state(&app)
        .hive
        .append_memory(&agent_id, &entry)
        .map_err(err_str)
}

// ─── config ─────────────────────────────────────────────────────────────────

#[tauri::command(async)]
/// config:get — upstream readConfig(): the persisted file deep-filled with
/// defaults and returned as the camelCase object the renderer expects. The
/// raw file merge preserves unknown keys (officeTheme, terminalTheme, the
/// onboarding fields the typed Config doesn't model).
pub fn get_config(app: AppHandle) -> Result<serde_json::Value, String> {
    let st = state(&app);
    let typed = serde_json::to_value(&*st.config.lock().unwrap()).map_err(|e| e.to_string())?;
    let mut merged = typed.as_object().cloned().unwrap_or_default();
    if let Some(raw) = st.config_store.raw_value().as_object() {
        for (k, v) in raw {
            merged.insert(k.clone(), v.clone());
        }
    }
    Ok(serde_json::Value::Object(merged))
}

#[tauri::command(async)]
pub fn set_auto_mode(app: AppHandle, enabled: bool) -> Result<(), String> {
    let st = state(&app);
    {
        let mut cfg = st.config.lock().unwrap();
        cfg.auto_mode = enabled;
        st.config_store.save(&cfg).map_err(err_str)?;
    }
    let _ = app.emit("config:changed", serde_json::json!({ "autoMode": enabled }));
    Ok(())
}

// ─── operator control (control.ts IPC equivalents) ──────────────────────────

#[tauri::command(async)]
pub fn control_pause(
    app: AppHandle,
    agent_id: String,
    on: bool,
) -> Result<ControlSnapshot, String> {
    let st = state(&app);
    let mut c = st.control.lock().unwrap();
    c.pause(&agent_id, on);
    Ok(c.snapshot(&agent_id))
}

#[tauri::command(async)]
pub fn control_resume(app: AppHandle, agent_id: String) -> Result<ControlSnapshot, String> {
    let st = state(&app);
    let mut c = st.control.lock().unwrap();
    c.resume(&agent_id);
    Ok(c.snapshot(&agent_id))
}

#[tauri::command(async)]
pub fn control_halt(app: AppHandle, agent_id: String) -> Result<ControlSnapshot, String> {
    let st = state(&app);
    let mut c = st.control.lock().unwrap();
    c.halt(&agent_id);
    Ok(c.snapshot(&agent_id))
}

#[tauri::command(async)]
pub fn control_steer(
    app: AppHandle,
    agent_id: String,
    text: String,
) -> Result<ControlSnapshot, String> {
    let st = state(&app);
    let mut c = st.control.lock().unwrap();
    c.steer(&agent_id, &text);
    Ok(c.snapshot(&agent_id))
}

#[tauri::command(async)]
pub fn control_gate_tool(
    app: AppHandle,
    agent_id: String,
    tool: String,
    on: bool,
) -> Result<ControlSnapshot, String> {
    let st = state(&app);
    let mut c = st.control.lock().unwrap();
    c.gate_tool(&agent_id, &tool, on);
    Ok(c.snapshot(&agent_id))
}

#[tauri::command(async)]
pub fn control_pause_delivery(
    app: AppHandle,
    agent_id: String,
    on: bool,
) -> Result<ControlSnapshot, String> {
    let st = state(&app);
    let mut c = st.control.lock().unwrap();
    c.pause_auto_delivery(&agent_id, on);
    Ok(c.snapshot(&agent_id))
}

#[tauri::command(async)]
pub fn control_snapshot(app: AppHandle, agent_id: String) -> Result<ControlSnapshot, String> {
    Ok(state(&app).control.lock().unwrap().snapshot(&agent_id))
}

// ─── missions (scheduler) ───────────────────────────────────────────────────

#[tauri::command(async)]
pub fn missions_list(app: AppHandle) -> Result<Vec<crate::missions::ScheduledMission>, String> {
    Ok(state(&app).config.lock().unwrap().missions.clone())
}

#[tauri::command(async)]
pub fn missions_save(
    app: AppHandle,
    missions: Vec<crate::missions::ScheduledMission>,
) -> Result<(), String> {
    let st = state(&app);
    {
        let mut cfg = st.config.lock().unwrap();
        cfg.missions = missions;
        st.config_store.save(&cfg).map_err(err_str)?;
    }
    // Upstream's clearMissionTimers+re-arm on write: wake the scheduler loop
    // so the new cadence applies now, not after the current sleep.
    st.mission_rearm.notify_waiters();
    let _ = app.emit("missions:updated", serde_json::json!({}));
    Ok(())
}

// ─── secrets (OS keychain → agent env) ──────────────────────────────────────

/// Read a credential — the caller's only path to a value (slack secrets and
/// spawn-time env injection both go through here).
fn secret_get(app: &AppHandle, key: &str) -> Option<String> {
    // Keychain first, DPAPI fallback file — same store spawn injection reads.
    let root = state(app).config_store.dir().to_path_buf();
    crate::misc::secret_get_persist(key, &root)
}

/// Store a secret in the OS credential store and register its env key for
/// injection. The value never touches config.json or log.jsonl.
#[tauri::command(async)]
pub fn secret_set(app: AppHandle, key: String, value: String) -> Result<(), String> {
    let key = key.trim().to_string();
    if key.is_empty() || value.is_empty() {
        return Err("key and value required".into());
    }
    // Verify-after-write, DPAPI-file fallback — a CredMan-dropped write lands
    // in the encrypted file store instead of pretending to persist.
    let st = state(&app);
    crate::misc::secret_set_persist(&key, &value, st.config_store.dir())?;
    let mut cfg = st.config.lock().unwrap();
    if !cfg.secret_env_keys.contains(&key) {
        cfg.secret_env_keys.push(key);
        st.config_store.save(&cfg).map_err(err_str)?;
    }
    Ok(())
}

/// Remove a secret — from the keychain AND the injection list.
#[tauri::command(async)]
pub fn secret_delete(app: AppHandle, key: String) -> Result<(), String> {
    let st = state(&app);
    crate::misc::secret_delete_persist(&key, st.config_store.dir());
    let mut cfg = st.config.lock().unwrap();
    cfg.secret_env_keys.retain(|k| k != &key);
    st.config_store.save(&cfg).map_err(err_str)
}

/// The injection-list key NAMES — values are never exposed to the frontend.
#[tauri::command(async)]
pub fn secrets_list(app: AppHandle) -> Result<Vec<String>, String> {
    Ok(state(&app).config.lock().unwrap().secret_env_keys.clone())
}

// ─── webhooks (inbound HTTP → god's inbox) ──────────────────────────────────

#[tauri::command(async)]
pub fn webhooks_list(app: AppHandle) -> Result<Vec<crate::webhook::WebhookEndpoint>, String> {
    Ok(state(&app).config.lock().unwrap().webhooks.clone())
}

/// Persist the endpoint list and hot-swap it on the running server — a
/// removed id stops resolving on the very next request (upstream
/// setEndpoints semantics; the port never rebinds).
#[tauri::command(async)]
pub fn webhooks_save(
    app: AppHandle,
    webhooks: Vec<crate::webhook::WebhookEndpoint>,
) -> Result<u16, String> {
    let st = state(&app);
    {
        let mut cfg = st.config.lock().unwrap();
        cfg.webhooks = webhooks.clone();
        st.config_store.save(&cfg).map_err(err_str)?;
    }
    let port = st.config.lock().unwrap().webhook_port;
    ensure_webhook_server(&app, &webhooks, port);
    Ok(port)
}

/// Start the loopback server if endpoints exist and it isn't bound yet.
/// Returns the port actually serving (None = nothing to serve).
fn ensure_webhook_server(
    app: &AppHandle,
    endpoints: &[crate::webhook::WebhookEndpoint],
    port: u16,
) -> Option<u16> {
    let st = state(app);
    if st.webhook.set_endpoints(endpoints) == 0 {
        return None;
    }
    st.webhook.prune_held(app, &st.trigger_history);
    use std::sync::atomic::Ordering::SeqCst;
    // Shared with webhook_start/webhook_stop: a stop clears BOUND and latches,
    // so a save after a stop must not claim a listener that no longer exists.
    if crate::cmd_misc::WEBHOOK_LATCHED.load(SeqCst) {
        return None;
    }
    if crate::cmd_misc::WEBHOOK_BOUND.swap(true, SeqCst) {
        return Some(port); // already bound; set_endpoints already applied
    }
    let wh = st.webhook.clone();
    let hive = st.hive.clone();
    let app2 = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = wh.run(port, hive, app2) {
            log::warn!("[webhook] server stopped: {e:#}");
        }
        // The serve loop returned (bind failure or a stop) — nothing listens.
        crate::cmd_misc::WEBHOOK_BOUND.store(false, SeqCst);
    });
    Some(port)
}

/// Approve a held inbound — upstream triggerHistory:decide with
/// decision='approved'. Takes the HISTORY ENTRY id (the pending row the UI
/// lists), releases the message down the identical card+god path, and flips
/// the ledger. Idempotent: already-decided entries are a no-op.
#[tauri::command(async)]
pub fn webhook_approve(app: AppHandle, token: String) -> Result<serde_json::Value, String> {
    let st = state(&app);
    st.webhook
        .decide(&app, &st.hive, &token, "approved")
        .ok_or_else(|| "unknown or non-pending entry".to_string())
}

#[tauri::command(async)]
pub fn webhook_reject(app: AppHandle, token: String) -> Result<serde_json::Value, String> {
    let st = state(&app);
    st.webhook
        .decide(&app, &st.hive, &token, "rejected")
        .ok_or_else(|| "unknown or non-pending entry".to_string())
}

/// triggerHistory:decide — the operator's verdict on a held message.
/// 'approved' releases the identical dispatch path an auto-allowed message
/// would take; 'rejected' just flips. Idempotent: only a `pending` entry can
/// be decided (upstream index.ts:4300).
#[tauri::command(async)]
pub fn trigger_history_decide(
    app: AppHandle,
    arg: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let st = state(&app);
    let id = arg.get("id").and_then(|v| v.as_str()).unwrap_or_default();
    let decision = match arg.get("decision").and_then(|v| v.as_str()) {
        Some("approved") => "approved",
        Some("rejected") => "rejected",
        _ => return Err("decision must be 'approved' or 'rejected'".to_string()),
    };
    if id.is_empty() {
        return Err("missing id".to_string());
    }
    Ok(st
        .webhook
        .decide(&app, &st.hive, id, decision)
        .unwrap_or(serde_json::Value::Null))
}

/// Pending inbound rows — upstream's surface is the trigger ledger itself
/// (entries still at decision:'pending').
#[tauri::command(async)]
pub fn webhook_pending(app: AppHandle) -> Result<Vec<serde_json::Value>, String> {
    Ok(state(&app).webhook.pending(&app))
}

// ─── hire deep-link queue (agent-office://hire?src=...) ─────────────────────

/// Where a deep-link hire request is parked until the UI drains it —
/// upstream's hire:drainPending contract.
pub fn pending_hire_path(home: &std::path::Path) -> std::path::PathBuf {
    home.join("pending-hire.json")
}

/// Park a pending hire source (called by the deep-link handler + tests).
pub fn park_pending_hire(home: &std::path::Path, src: &str) {
    let _ = std::fs::write(
        pending_hire_path(home),
        serde_json::json!({ "src": src }).to_string(),
    );
}

/// hire:drainPending — read + clear the queued deep-link source.
#[tauri::command(async)]
pub fn hire_drain_pending(app: AppHandle) -> Result<Option<String>, String> {
    let st = state(&app);
    let home = st.hive.root.parent().unwrap_or(&st.hive.root).to_path_buf();
    let p = pending_hire_path(&home);
    if !p.exists() {
        return Ok(None);
    }
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&p).map_err(|e| e.to_string())?)
            .unwrap_or_default();
    let _ = std::fs::remove_file(&p);
    Ok(v.get("src").and_then(|s| s.as_str()).map(String::from))
}

// ─── config:update (generic patch — upstream's settings write path) ─────────

/// config:update — merge a partial JSON patch into config.json. Only keys
/// that exist on Config survive (serde drops the rest, same as upstream's
/// typed settings write); one level of object merge for nested knobs.
#[tauri::command(async)]
/// config:update — writeConfig(patch): shallow merge at the top level (a
/// nested object REPLACES its subtree wholesale, matching upstream's spread),
/// onto the RAW file so unknown keys survive, then re-parse the typed Config.
/// Returns the same merged view get_config produces.
pub fn config_update(
    app: AppHandle,
    patch: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let st = state(&app);
    let patch_obj = patch.as_object().cloned().unwrap_or_default();
    // Goes through ConfigStore::update so patch keys the typed Config doesn't
    // model (onboardingComplete, godProvider, officeTheme…) persist — the old
    // save(&cfg) path re-read the PRE-patch file and silently dropped them,
    // which is why the wizard's Finish button could never complete.
    let (cfg, _raw) = st.config_store.update(&patch_obj)?;
    *st.config.lock().unwrap() = cfg;
    // onConfigWritten → syncMissions re-arm (upstream re-arms on EVERY
    // config write, not just missions:save).
    st.mission_rearm.notify_waiters();
    get_config(app)
}

/// webhook:setConfig — secret + port + enable toggle; the port applies on
/// next boot. `secret` rotates the legacy endpoint's shared secret.
#[tauri::command(async)]
pub fn webhook_set_config(
    app: AppHandle,
    secret: Option<String>,
    port: Option<u16>,
    enabled: Option<bool>,
) -> Result<bool, String> {
    // Mirror a legacy setConfig/generateSecret edit onto the `legacy`
    // WebhookTrigger (upstream upsertLegacyWebhookTrigger): creates the row
    // only once a secret exists — an enabled endpoint without a secret would
    // be an open door, so a bare "enable" is deliberately a no-op.
    {
        let st = state(&app);
        let mut cfg = st.config.lock().unwrap();
        let prior = cfg.webhooks.iter().find(|e| e.id == "legacy").cloned();
        let next_secret = secret
            .clone()
            .or_else(|| prior.as_ref().map(|p| p.secret.clone()));
        if let Some(sec) = next_secret.filter(|s| !s.is_empty()) {
            let row = crate::webhook::WebhookEndpoint {
                id: "legacy".into(),
                name: prior
                    .as_ref()
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "Default webhook".into()),
                secret: sec,
                schema: prior.as_ref().map(|p| p.schema.clone()).unwrap_or_default(),
                mode: prior
                    .as_ref()
                    .map(|p| p.mode.clone())
                    .unwrap_or_else(|| crate::trigger_history::DEFAULT_TRIGGER_MODE.into()),
                enabled: enabled
                    .or_else(|| prior.as_ref().map(|p| p.enabled))
                    .unwrap_or(false),
            };
            match cfg.webhooks.iter_mut().find(|e| e.id == "legacy") {
                Some(ep) => *ep = row,
                None => cfg.webhooks.push(row),
            }
            st.config_store.save(&cfg).map_err(err_str)?;
        } else if enabled.is_some() {
            // Secret cleared (or never set) but a disable still applies —
            // dropping the public surface immediately is safe.
            if let Some(ep) = cfg.webhooks.iter_mut().find(|e| e.id == "legacy") {
                ep.enabled = enabled.unwrap_or(ep.enabled);
                st.config_store.save(&cfg).map_err(err_str)?;
            }
        }
    }
    let st = state(&app);
    if let Some(p) = port {
        let mut cfg = st.config.lock().unwrap();
        cfg.webhook_port = p;
        st.config_store.save(&cfg).map_err(err_str)?;
    }
    // Hot-swap endpoints on the live server, same as webhooks_save — a
    // disable or secret rotation must not need an app restart.
    {
        let (eps, port) = {
            let cfg = st.config.lock().unwrap();
            (cfg.webhooks.clone(), cfg.webhook_port)
        };
        ensure_webhook_server(&app, &eps, port);
    }
    Ok(true)
}

/// hire:openFile — pick a .json manifest then run the same validation the
/// deep link uses (upstream: dialog → validate → prefill).
#[tauri::command]
pub fn hire_open_file(app: AppHandle) -> Result<serde_json::Value, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app
        .dialog()
        .file()
        .add_filter("Hire manifest", &["json"])
        .blocking_pick_file();
    let Some(p) = picked else {
        return Ok(serde_json::json!({ "ok": false, "error": "cancelled" }));
    };
    let path = p.as_path().ok_or("no path")?;
    let raw = crate::hire::read_manifest_file(path)?;
    let (manifest, errors, warns) = crate::hire::validate_hire_manifest(&raw);
    if let Some(m) = manifest {
        Ok(
            serde_json::json!({ "ok": true, "manifest": m, "warnings": warns, "path": path.display().to_string() }),
        )
    } else {
        Ok(serde_json::json!({ "ok": false, "errors": errors }))
    }
}

// ─── app lifecycle (close gating + info) ────────────────────────────────────

/// app:confirmClose — kick the closing-time protocol (same rail as
/// closing_time_start; frontend shows "Confirm close" → this).
#[tauri::command(async)]
pub fn app_confirm_close(app: AppHandle) -> Result<usize, String> {
    closing_time_start(app)
}

/// app:cancelClose — abort the in-flight closing-time protocol.
#[tauri::command(async)]
pub fn app_cancel_close(app: AppHandle) -> Result<bool, String> {
    closing_time_cancel(app)
}

/// app:info — version + home path for the About surface.
#[tauri::command(async)]
pub fn app_info(app: AppHandle) -> Result<serde_json::Value, String> {
    let st = state(&app);
    // Upstream 'app:info' (index.ts) — version + the two newest CHANGELOG
    // sections for the voice get_app_info tool. Our extra name/home/
    // liveAgents fields are additive.
    let mut changelog = String::new();
    for p in [
        std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| p.join("CHANGELOG.md"))),
        Some(std::path::PathBuf::from("CHANGELOG.md")),
    ]
    .into_iter()
    .flatten()
    {
        if let Ok(c) = std::fs::read_to_string(&p) {
            changelog = c;
            break;
        }
    }
    let top: String = if changelog.is_empty() {
        String::new()
    } else {
        changelog
            .split("\n## ")
            .skip(1)
            .take(2)
            .map(|s| format!("## {s}"))
            .collect::<Vec<_>>()
            .join("\n")
            .chars()
            .take(8000)
            .collect()
    };
    Ok(serde_json::json!({
        "name": "Agent Office",
        "version": env!("CARGO_PKG_VERSION"),
        "changelog": top,
        "home": st.hive.root.parent().unwrap_or(&st.hive.root).display().to_string(),
        "liveAgents": st.ptys.lock().unwrap().len(),
    }))
}

/// skills:reveal — open a skill's directory in the OS file manager.
#[tauri::command(async)]
pub fn skills_reveal(app: AppHandle, id: String) -> Result<bool, String> {
    let st = state(&app);
    let cwds: Vec<String> = st.hive.registry().iter().map(|a| a.cwd.clone()).collect();
    let bundled = crate::skills::bundled_skills_dir();
    let dir = crate::skills::list_local_skills(&cwds, bundled.as_deref())
        .into_iter()
        .find(|s| s.id == id)
        .map(|s| s.path)
        .ok_or_else(|| "unknown skill".to_string())?;
    #[cfg(windows)]
    std::process::Command::new("explorer")
        .arg(&dir)
        .spawn()
        .ok();
    #[cfg(not(windows))]
    std::process::Command::new("open").arg(&dir).spawn().ok();
    Ok(true)
}

// ─── memory reflector (condense oversized memory.md via headless claude) ────

/// Manually condense one agent's memory.md — skips the trigger (on-demand);
/// the full backup→summarize→rebuild→verify→swap pipeline still runs.
#[tauri::command(async)]
pub fn reflect_now(
    app: AppHandle,
    agent_id: Option<String>,
) -> Result<Vec<serde_json::Value>, String> {
    let st = state(&app);
    let settings = crate::reflect::ReflectSettings {
        enabled: true,
        ..Default::default()
    };
    let hive_root = st.hive.root.clone();
    let path = crate::pty::user_shell_path();
    let claude = match pty::resolve_program("claude", &path) {
        Some(pty::Resolved::Exe(p)) | Some(pty::Resolved::Script(p)) => p.display().to_string(),
        None => "claude".to_string(),
    };
    if let Some(id) = agent_id {
        return Ok(vec![crate::reflect::condense_one(
            &hive_root, &id, &claude, &settings, true,
        )]);
    }
    Ok(crate::reflect::reflect_scan(&hive_root, &claude, &settings))
}

// ─── git pane (read surface; safe_resolve confinement on the caller path) ──

/// One git snapshot for the pane — is_repo? branch? counts? ahead/behind?
/// The `cwd` argument is the agent's workspace directory.
#[tauri::command(async)]
pub fn git_snapshot(app: AppHandle, cwd: String) -> Result<serde_json::Value, String> {
    let _ = app;
    let p = std::path::Path::new(&cwd);
    if !crate::git_ops::is_repo(p) {
        return Ok(serde_json::json!({ "isRepo": false }));
    }
    Ok(serde_json::json!({
        "isRepo": true,
        "branch": crate::git_ops::branch(p),
        "status": crate::git_ops::status(p),
        "aheadBehind": crate::git_ops::ahead_behind(p),
        "unintegratedWork": crate::git_ops::worktree_has_unintegrated_work(p),
    }))
}

// ─── confined fs browser (fs_ops.safe_resolve policy) ───────────────────────

/// List a directory inside an agent's workspace — `root` is the agent's cwd
/// (or any path the caller confines); `rel` must stay inside it.
#[tauri::command(async)]
pub fn fs_list(
    app: AppHandle,
    root: String,
    rel: String,
) -> Result<Vec<serde_json::Value>, String> {
    let _ = app;
    crate::fs_ops::list_dir(std::path::Path::new(&root), &rel)
        .ok_or_else(|| "path escapes the workspace".to_string())
}

#[tauri::command(async)]
pub fn fs_read(app: AppHandle, root: String, rel: String) -> Result<serde_json::Value, String> {
    let _ = app;
    match crate::fs_ops::read_file(std::path::Path::new(&root), &rel) {
        Some(crate::fs_ops::FsRead::Text(t)) => Ok(serde_json::json!({"kind": "text", "data": t})),
        Some(crate::fs_ops::FsRead::Image { mime, base64 }) => {
            Ok(serde_json::json!({"kind": "image", "mime": mime, "data": base64}))
        }
        Some(crate::fs_ops::FsRead::TooLarge(n)) => Err(format!("file too large ({n} bytes)")),
        None => Err("path escapes the workspace or is unreadable".into()),
    }
}

// ─── persist store (kv + command history) ───────────────────────────────────

#[tauri::command(async)]
pub fn kv_get(app: AppHandle, key: String) -> Result<serde_json::Value, String> {
    Ok(state(&app)
        .persist
        .lock()
        .unwrap()
        .get_kv(&key)
        .unwrap_or(serde_json::Value::Null))
}

#[tauri::command(async)]
pub fn kv_set(app: AppHandle, key: String, value: serde_json::Value) -> Result<bool, String> {
    state(&app).persist.lock().unwrap().set_kv(&key, &value);
    Ok(true)
}

/// Record one user-submitted prompt (db.ts addHistory).
#[tauri::command(async)]
pub fn command_history_add(
    app: AppHandle,
    agent_id: String,
    cwd: Option<String>,
    text: String,
) -> Result<bool, String> {
    state(&app)
        .persist
        .lock()
        .unwrap()
        .add_history(&agent_id, cwd.as_deref(), &text);
    Ok(true)
}

#[tauri::command(async)]
pub fn command_history_list(
    app: AppHandle,
    agent_id: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<serde_json::Value>, String> {
    Ok(state(&app)
        .persist
        .lock()
        .unwrap()
        .list_history(agent_id.as_deref(), limit.unwrap_or(100)))
}

#[tauri::command(async)]
pub fn command_history_search(
    app: AppHandle,
    query: String,
    limit: Option<i64>,
) -> Result<Vec<serde_json::Value>, String> {
    Ok(state(&app)
        .persist
        .lock()
        .unwrap()
        .search_history(&query, limit.unwrap_or(50)))
}

// ─── hire manifests (agent-office://hire deep link + file import) ──────────

/// Validate + return a hire manifest (prefill only — never auto-spawns).
#[tauri::command(async)]
pub fn hire_validate(raw: serde_json::Value) -> serde_json::Value {
    let (m, errors, consent) = crate::hire::validate_hire_manifest(&raw);
    serde_json::json!({ "ok": m.is_some(), "manifest": m, "errors": errors, "consentRequired": consent })
}

/// Import from an https URL — SSRF-gated (resolved addresses blocklisted).
#[tauri::command(async)]
pub fn hire_fetch(url: String) -> Result<serde_json::Value, String> {
    let raw = crate::hire::fetch_manifest(&url)?;
    let (m, errors, consent) = crate::hire::validate_hire_manifest(&raw);
    if let Some(m) = m {
        Ok(serde_json::json!({ "ok": true, "manifest": m, "consentRequired": consent }))
    } else {
        Err(format!("invalid hire manifest: {}", errors.join("; ")))
    }
}

/// Import from a local file — same byte cap + validator.
#[tauri::command(async)]
pub fn hire_import_file(path: String) -> Result<serde_json::Value, String> {
    let raw = crate::hire::read_manifest_file(std::path::Path::new(&path))?;
    let (m, errors, consent) = crate::hire::validate_hire_manifest(&raw);
    if let Some(m) = m {
        Ok(serde_json::json!({ "ok": true, "manifest": m, "consentRequired": consent }))
    } else {
        Err(format!("invalid hire manifest: {}", errors.join("; ")))
    }
}

// ─── slack adapter (Events API → god inbox; reply rail) ─────────────────────

/// Start the Slack listener. The signing secret comes from the credential
/// store (SLACK_SIGNING_SECRET) — never from disk or args.
#[tauri::command(async)]
pub fn slack_start(app: AppHandle, port: u16, channel_id: Option<String>) -> Result<bool, String> {
    let st = state(&app);
    let secret = secret_get(&app, "SLACK_SIGNING_SECRET")
        .ok_or_else(|| "SLACK_SIGNING_SECRET not in credential store".to_string())?;
    let hive = st.hive.clone();
    let trigger_history = st.trigger_history.clone();
    let app2 = app.clone();
    let app3 = app.clone();
    st.slack
        .start(port, secret, channel_id, move |text, channel, _ts, thread_ts, files| {
            // Download file attachments with the bot token (stays in main —
            // only local paths reach the card/message). Dest dir lives under
            // the harness home, out of the repo and out of MemPalace.
            let bot_token = secret_get(&app3, "SLACK_BOT_TOKEN");
            let files_dir = crate::misc::harness_home(&app3).join("slack-files");
            let local_files = crate::slack::download_slack_files(
                &files,
                bot_token.as_deref(),
                &files_dir,
            );
            let mut text = text.trim().to_string();
            let slack_raw_text = text.clone();
            if !local_files.is_empty() {
                let lines = local_files
                    .iter()
                    .map(|f| format!("- {} ({})", f["path"].as_str().unwrap_or(""), f["name"].as_str().unwrap_or("")))
                    .collect::<Vec<_>>()
                    .join("\n");
                text = if text.is_empty() {
                    format!("Attached files:\n{lines}")
                } else {
                    format!("{text}\n\nAttached files:\n{lines}")
                };
            }

            // The kanban card — carries slack:{channel,thread_ts} so the
            // done-observer can post the summary back in-thread. Title stays
            // the user's raw text (human-facing).
            let task_id = format!(
                "slack-{}-{}",
                thread_ts,
                uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
            );
            let card_title: String = text.chars().take(80).collect();
            let _ = hive.add_task(&serde_json::json!({
                "id": task_id,
                "title": card_title,
                "description": text,
                "status": "todo",
                "dependsOn": [],
                "priority": 1,
                "createdAt": chrono::Utc::now().to_rfc3339(),
                "slack": { "channel": channel, "thread_ts": thread_ts },
            }));

            // God gets the AUTONOMOUS REQUEST PROTOCOL prepended — the
            // authoritative policy block, built per message with the resolved
            // helper path so god hands the worker an exact reply command.
            let helper = crate::misc::write_slack_reply_script(&crate::misc::harness_home(&app3))
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "slack-reply".into());
            let preamble =
                crate::slack::autonomous_request_protocol(&channel, &thread_ts, &helper);
            // slack:incomingMessage (upstream index.ts) — the renderer's live
            // Slack feed gets the message even while main routes it to god.
            let mut ipc_msg = serde_json::json!({
                "text": slack_raw_text, "channel": channel, "ts": _ts, "thread_ts": thread_ts,
                "autonomyPreamble": preamble,
            });
            if !local_files.is_empty() {
                ipc_msg["files"] = serde_json::json!(local_files);
            }
            let _ = app3.emit("slack:incomingMessage", &ipc_msg);
            let instruction = format!("{preamble}{text}\n\n(tracked as kanban card {task_id})");
            if let Ok(_d) = hive.send_virtual("slack", "god", "request", &card_title, &instruction) {
                trigger_history.append(crate::trigger_history::TriggerHistoryEntry {
                    id: String::new(),
                    source: "webhook".into(),
                    source_id: "slack".into(),
                    source_name: "slack".into(),
                    direction: "inbound".into(),
                    peer: format!("#{channel}"),
                    title: Some(card_title.clone()),
                    body: text.clone(),
                    kind: crate::trigger_history::classify_inbound_kind(&text).into(),
                    decision: Some("auto-allowed".into()),
                    correlation_id: Some(format!("{channel}:{thread_ts}")),
                    task_id: Some(task_id.clone()),
                    at: 0,
                });
                let _ = app2.emit("hive-deliver", serde_json::json!({"to": "god"}));
            }
            // Immediate "queued" acknowledgement in the originating thread —
            // posted server-side with the bot token (renderer did this in
            // upstream; the port's slack flow is all main-side).
            if let Some(tok) = &bot_token {
                let _ = crate::slack::post_slack_reply(
                    tok,
                    &channel,
                    &thread_ts,
                    ":hourglass_flowing_sand: *Received.* Your request has been queued — the team is on it and will reply here when done.",
                );
            }
        })
        ?;

    // Re-seed the done-observer baseline on every start (upstream
    // startSlackDoneObserver sets slackDoneBaseline = null).
    reset_slack_done_baseline();

    // The loopback reply rail — agents post {channel,thread_ts,text} WITHOUT
    // ever seeing the bot token (upstream SlackReplyServer). Per-session
    // token + the discovery file under the harness home.
    let app4 = app.clone();
    match st
        .slack_reply
        .start(0, move || secret_get(&app4, "SLACK_BOT_TOKEN"), {
            let st2 = std::sync::Arc::clone(&st.directly_replied);
            move |thread_ts| {
                st2.lock().unwrap().insert(thread_ts);
            }
        }) {
        Ok(reply_port) => {
            let cfg_path = crate::misc::harness_home(&app).join("slack-reply.json");
            let _ = std::fs::write(
                &cfg_path,
                serde_json::json!({ "port": reply_port, "token": st.slack_reply.token })
                    .to_string(),
            );
        }
        Err(e) => log::warn!("[slack] reply endpoint failed to start: {e}"),
    }
    // Public tunnel (upstream slack.ts openTunnel — Slack's servers need a
    // reachable Request URL). Non-fatal: the local handler is the security
    // boundary and stays up even if the tunnel can't be established.
    if let Some((_, mut old)) = st.slack_tunnel.lock().unwrap().take() {
        crate::misc::kill_process_tree(&mut old); // never orphan a previous tunnel
    }
    match crate::misc::start_tunnel(port) {
        Ok((url, child)) => *st.slack_tunnel.lock().unwrap() = Some((url, child)),
        Err(e) => log::warn!("[slack] tunnel unavailable: {e}"),
    }
    Ok(true)
}

#[tauri::command(async)]
pub fn slack_stop(app: AppHandle) -> Result<bool, String> {
    let st = state(&app);
    st.slack.stop();
    st.slack_reply.stop();
    if let Some((_, mut child)) = st.slack_tunnel.lock().unwrap().take() {
        crate::misc::kill_process_tree(&mut child);
    }
    // The discovery file advertises a dead socket once the rail is down —
    // drop it so a stale helper doesn't talk to an unrelated listener.
    let _ = std::fs::remove_file(crate::misc::harness_home(&app).join("slack-reply.json"));
    Ok(true)
}

#[tauri::command(async)]
pub fn slack_status(app: AppHandle) -> Result<serde_json::Value, String> {
    let st = state(&app);
    Ok(serde_json::json!({
        "running": st.slack.is_running(),
        "port": *st.slack.url_port.lock().unwrap(),
        "replyPort": *st.slack_reply.port.lock().unwrap(),
        "url": st.slack_tunnel.lock().unwrap().as_ref().map(|(u, _)| u.clone()),
    }))
}

/// Reply rail — agents post back into the Slack thread WITHOUT the bot
/// token (it comes from the credential store at send time). The
/// channel+thread are explicit (CLAUSE-1 — no channel-root posts).
#[tauri::command(async)]
pub fn slack_reply(
    app: AppHandle,
    channel: String,
    thread_ts: String,
    text: String,
) -> Result<bool, String> {
    let token = secret_get(&app, "SLACK_BOT_TOKEN")
        .ok_or_else(|| "SLACK_BOT_TOKEN not in credential store".to_string())?;
    crate::slack::post_slack_reply(&token, &channel, &thread_ts, &text)?;
    Ok(true)
}

/// pollSlackDoneTasks (index.ts:1611) — watch the kanban for Slack-origin
/// cards that reach 'done' and post ONE summary reply in-thread. The
/// notified-ledger is persisted so a restart can't double-post; a live
/// baseline means a summary only fires on a transition observed this
/// session; threads an agent answered DIRECTLY via the reply rail are
/// skipped (the poller is a fallback, not a duplicator).
/// Ids already 'done' when the observer started — baselined (never notified)
/// so a summary only fires on a live …→done transition (upstream
/// slackDoneBaseline; re-seeded on every slack_start).
static SLACK_DONE_BASELINE: std::sync::OnceLock<Mutex<Option<std::collections::HashSet<String>>>> =
    std::sync::OnceLock::new();

/// Reset the done-observer's baseline — startSlackDoneObserver's
/// `slackDoneBaseline = null` (re-seeds on the poller's next tick).
pub(crate) fn reset_slack_done_baseline() {
    if let Some(m) = SLACK_DONE_BASELINE.get() {
        *m.lock().unwrap() = None;
    }
}

pub(crate) fn poll_slack_done_tasks(app: &AppHandle) {
    let st = state(app);
    let Some(token) = secret_get(app, "SLACK_BOT_TOKEN") else {
        return; // can't post without the token — nothing to do
    };
    let Ok(doc) = st.hive.read_tasks() else {
        return;
    };
    let Some(tasks) = doc["tasks"].as_array() else {
        return;
    };

    // Exactly-once across re-reads AND restarts — ids only, persisted under
    // the harness home (out of the repo, out of MemPalace).
    let notified_path = crate::misc::harness_home(app).join("slack-done-notified.json");
    let mut notified: std::collections::HashSet<String> = std::fs::read_to_string(&notified_path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .map(|v| v.into_iter().collect())
        .unwrap_or_default();
    // The tick that seeds the baseline posts nothing — mirroring upstream's
    // `if (slackDoneBaseline === null) { baseline = …; return; }`.
    let baseline_cell = SLACK_DONE_BASELINE.get_or_init(|| Mutex::new(None));
    let baseline = {
        let mut g = baseline_cell.lock().unwrap();
        match g.as_ref() {
            Some(b) => b.clone(),
            None => {
                *g = Some(
                    tasks
                        .iter()
                        .filter(|t| t["status"].as_str() == Some("done"))
                        .filter_map(|t| t["id"].as_str().map(|s| s.to_string()))
                        .collect(),
                );
                return;
            }
        }
    };

    let persist = |set: &std::collections::HashSet<String>| {
        let v: Vec<&String> = set.iter().collect();
        let _ = std::fs::write(
            &notified_path,
            serde_json::to_string(&v).unwrap_or_default(),
        );
    };
    for t in tasks {
        if t["status"].as_str() != Some("done") {
            continue;
        }
        let id = t["id"].as_str().unwrap_or("");
        if baseline.contains(id) || notified.contains(id) {
            continue;
        }
        let slack = t.get("slack");
        let (channel, thread_ts) = match slack {
            Some(s) => (
                s["channel"].as_str().unwrap_or("").to_string(),
                s["thread_ts"].as_str().unwrap_or("").to_string(),
            ),
            None => continue,
        };
        if channel.is_empty() || thread_ts.is_empty() {
            continue;
        }
        // FALLBACK-ONLY: an agent that already posted a direct reply gave the
        // human its substantive answer — don't double-post.
        if st.directly_replied.lock().unwrap().contains(&thread_ts) {
            notified.insert(id.to_string());
            persist(&notified);
            continue;
        }
        // Never post a bare checkmark with no substance.
        if t.get("result")
            .and_then(|r| r.as_str())
            .or_else(|| t.get("description").and_then(|d| d.as_str()))
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            notified.insert(id.to_string());
            persist(&notified);
            continue;
        }
        match crate::slack::post_slack_reply(
            &token,
            &channel,
            &thread_ts,
            &crate::slack::done_summary(t),
        ) {
            Ok(()) => {
                notified.insert(id.to_string());
                persist(&notified);
            }
            Err(e) if crate::slack::is_terminal_slack_error(&e) => {
                // Permanent config/auth error — record the id so we stop
                // hammering every tick; fix the bot scope and later posts work.
                notified.insert(id.to_string());
                persist(&notified);
                log::warn!("[slack] done-summary for task {id} — giving up (terminal error: {e})");
            }
            Err(e) => {
                log::warn!("[slack] done-summary for task {id} failed — will retry: {e}");
            }
        }
    }
}

// ─── closing time (graceful shutdown protocol) ──────────────────────────────

/// Press "closing time": brief god + steer every live agent, then the router
/// observer tracks worker ACKs and the verified COMPLETE.
#[tauri::command(async)]
pub fn closing_time_start(app: AppHandle) -> Result<usize, String> {
    closing_time_start_core(&app)
}

/// Runtime-generic core — the command wrapper is Wry-pinned but tests drive
/// this against mock_app().
pub(crate) fn closing_time_start_core<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<usize, String> {
    let st = state(app);
    // closing-time steers AGENT ids — translate the live PTY map through the
    // pty→agent mapping (pty keys carry the renderer's `pty-` prefix).
    let live: std::collections::HashSet<String> = st.live_agent_ids();
    // start() mails god (disk write + git commit) — never under the global
    // control lock, which every hook handler takes. It steers into a scratch
    // registry; the queued notes are replayed into the real one afterwards.
    let mut scratch = crate::control::ControlRegistry::default();
    let n = st.closing.start(&st.hive, &mut scratch, &live)?;
    {
        let god_id = st.hive.god_id().unwrap_or_else(|| "god".to_string());
        let mut control = st.control.lock().unwrap();
        for id in live.iter().chain(std::iter::once(&god_id)) {
            while let Some(note) = scratch.take_steer(id) {
                control.steer(id, &note);
            }
        }
    }
    nudge_idle_god(&st);
    emit_closing(&app, "started");
    Ok(n)
}

#[tauri::command(async)]
pub fn closing_time_cancel(app: AppHandle) -> Result<bool, String> {
    let st = state(&app);
    // cancel() mails god — do that against a scratch registry, then clear the
    // real queued steers under a short-lived lock.
    let was_active = st.closing.is_active();
    let mut scratch = crate::control::ControlRegistry::default();
    st.closing.cancel(&st.hive, &mut scratch);
    if was_active {
        let god_id = st.hive.god_id().unwrap_or_else(|| "god".to_string());
        let live = st.live_agent_ids();
        let mut control = st.control.lock().unwrap();
        for id in live.iter().chain(std::iter::once(&god_id)) {
            control.clear_steers(id);
        }
    }
    emit_closing(&app, "cancelled");
    Ok(true)
}

#[tauri::command(async)]
pub fn closing_time_status(app: AppHandle) -> Result<serde_json::Value, String> {
    let st = state(&app);
    let (acked, total) = st.closing.progress();
    Ok(serde_json::json!({
        "active": st.closing.is_active(), "acked": acked, "total": total
    }))
}

/// Emit the protocol state to the floor UI (upstream 'app:closingTime').
/// The watchdog deliberately never wakes god (workerwake.rs explain() →
/// "god"), and the closing-time mail + steer only land at the next hook
/// boundary — so an idle orchestrator would sit on the shutdown brief until
/// the 6-minute timeout. Same fix the mission nudge uses: type into the PTY
/// when god is confirmed idle (typing into a busy terminal is harmful).
/// Returns whether a write went out — the observable half for tests.
pub(crate) fn nudge_idle_god(st: &AppState) -> bool {
    let god_id = st.hive.god_id().unwrap_or_else(|| "god".to_string());
    let Some(pty_id) = st.pty_for_agent(&god_id) else {
        return false;
    };
    let mut ptys = st.ptys.lock().unwrap();
    let Some(h) = ptys.get_mut(&pty_id) else {
        return false;
    };
    // Two idle signals, either suffices — registry status is the primary
    // (a Stop hook flips it), but a provider whose REPL never emits Stop
    // (hermes sits at a bare prompt between turns) leaves it stuck on
    // "working" forever. PTY-quiet-≥12s is the same evidence the worker
    // wake watchdog uses for engines without reliable idle hooks.
    let idle_by_status = st
        .hive
        .registry()
        .iter()
        .any(|a| a.id == god_id && a.status == "idle");
    let last_out = h.last_output_at.load(std::sync::atomic::Ordering::Relaxed);
    let quiet_ms = crate::closing_time::now_ms_pub().saturating_sub(last_out);
    let idle_by_quiet = last_out > 0 && quiet_ms >= crate::workerwake::WORKER_WAKE_IDLE_MS;
    if !(idle_by_status || idle_by_quiet) {
        return false;
    }
    let writer = h.writer();
    drop(ptys);
    writer
        .write(
            "CLOSING TIME — check your inbox for the shutdown brief and run the protocol now.\r",
        )
        .is_ok()
}

/// Emit the protocol state to the floor UI (upstream 'app:closingTime').
pub(crate) fn emit_closing<R: tauri::Runtime>(app: &AppHandle<R>, phase: &str) {
    let st = state(app);
    let (acked, total) = st.closing.progress();
    let _ = app.emit(
        "app:closingTime",
        serde_json::json!({ "phase": phase, "acked": acked, "total": total }),
    );
}

/// fleet.json heartbeat — upstream's ~8s fleetTimer keeps the snapshot god
/// is told to Read fresh even when no hook fires. Called from the breaker
/// beat; identical row shape to hooks.rs's hook-time write.
pub(crate) fn write_fleet_snapshot(app: &AppHandle) {
    let st = state(app);
    let agents = crate::hooks::fleet_rows(&st);
    let _ = st.hive.write_fleet_snapshot(&serde_json::json!({
        "ts": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        "agents": agents,
    }));
}

// ─── roster mirror (UI floor state on disk) ─────────────────────────────────

#[tauri::command(async)]
pub fn roster_read(app: AppHandle) -> Result<Option<serde_json::Value>, String> {
    Ok(state(&app).roster.read())
}

/// app:syncPort — the loopback sync-IPC port (sync_api.rs); the shim needs
/// it before its first sendSync-equivalent call.
#[tauri::command(async)]
pub fn sync_port(app: AppHandle) -> Result<u16, String> {
    Ok(state(&app)
        .sync_port
        .load(std::sync::atomic::Ordering::Relaxed))
}

#[tauri::command(async)]
pub fn roster_write(app: AppHandle, snap: serde_json::Value) -> Result<bool, String> {
    // A declined empty-first-write is not an error for the caller — the file
    // was protected, which is the outcome the renderer wants either way.
    match state(&app).roster.write(&snap) {
        Ok(()) => Ok(true),
        Err(e) if e.starts_with("skipped:") => Ok(false),
        Err(e) => Err(e),
    }
}

#[tauri::command(async)]
pub fn roster_archive(app: AppHandle) -> Result<bool, String> {
    state(&app).roster.archive();
    Ok(true)
}

/// Lifetime spend folded from cost-ledger.jsonl — the reset-safe number
/// (telemetry's cumulative-since-boot counter understates post-restart).
#[tauri::command(async)]
pub fn cost_lifetime(app: AppHandle) -> Result<serde_json::Value, String> {
    let st = state(&app);
    let agents = st.hive.lifetime_usd().unwrap_or_default();
    let floor_total: f64 = agents.values().sum();
    Ok(serde_json::json!({
        "agents": agents,
        "floor_total": floor_total,
    }))
}

// ─── task ledger writes (patchTaskInLedger semantics) ───────────────────────

#[tauri::command(async)]
pub fn task_update(
    app: AppHandle,
    id: String,
    patch: serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value, String> {
    state(&app).hive.update_task(&id, &patch).map_err(err_str)
}

/// The ASK ME answer flow: sets the last open humanQA entry's `a`, unblocks
/// the card, and mails god the answer — the two-channel contract upstream
/// spells out in the orchestrator prompt.
#[tauri::command(async)]
pub fn task_answer(
    app: AppHandle,
    id: String,
    answer: String,
) -> Result<serde_json::Value, String> {
    state(&app).hive.task_answer(&id, &answer).map_err(err_str)
}

// ─── skills (local discovery + catalog + hardened install) ─────────────────

#[tauri::command(async)]
pub fn skills_local(cwds: Vec<String>) -> Result<Vec<crate::skills::LocalSkill>, String> {
    // Bundled skills staged next to the running exe (app resources/skills/).
    let bundled = crate::skills::bundled_skills_dir();
    Ok(crate::skills::list_local_skills(&cwds, bundled.as_deref()))
}

#[tauri::command(async)]
pub fn skills_catalog(
    app: AppHandle,
    force: Option<bool>,
) -> Result<crate::skills::CatalogResult, String> {
    let cache = state(&app).hive.root.join("skills-catalog.json");
    Ok(crate::skills::load_catalog(&cache, force.unwrap_or(false)))
}

#[tauri::command(async)]
/// Upstream returns {ok, path?} | {ok:false, error} — the renderer's res.ok
/// check fails on a bare Ok(String), so wrap instead of Result<>.
pub fn skills_install(url: String, name: String) -> Value {
    match crate::skills::install_skill(&url, &name) {
        Ok(path) => serde_json::json!({ "ok": true, "path": path }),
        Err(e) => serde_json::json!({ "ok": false, "error": e }),
    }
}

/// Offline catalog — the skill pack shipped in resources/skills. No network.
#[tauri::command(async)]
pub fn skills_bundled() -> Vec<crate::skills::BundledSkill> {
    crate::skills::list_bundled_skills()
}

// ── ShardX browser ops ──────────────────────────────────────────────────────
/// The launcher listens on 127.0.0.1:40325 (Settings → Automation API). A TCP
/// connect is enough to prove the app is running — the API itself is Bearer
/// authed so we never poke it unauthenticated.
pub const SHARDX_API_DEFAULT: &str = "http://127.0.0.1:40325";
const SHARDX_API_PORT: u16 = 40325;
const SHARDX_TOKEN_KEY: &str = "SHARDX_TOKEN";

/// Status of the whole ShardX chain — the UI gates the toggle on these.
#[tauri::command(async)]
pub fn shardx_status(app: AppHandle) -> Value {
    shardx_status_inner(&app)
}

fn shardx_status_inner<R: tauri::Runtime>(app: &AppHandle<R>) -> Value {
    let st = state(app);
    let launcher = std::net::TcpStream::connect(("127.0.0.1", SHARDX_API_PORT)).is_ok();
    let mcp_dir = st.hive.root.join("mcp-servers").join("shardx");
    let mcp_installed = mcp_dir.join("index.js").exists() && mcp_dir.join("node_modules").is_dir();
    let token_set =
        crate::misc::secret_get_persist(SHARDX_TOKEN_KEY, st.config_store.dir())
            .is_some_and(|t| !t.is_empty());
    // Live counts so the card surfaces "library empty" / sessions without
    // anyone opening the launcher GUI. None when the API is unreachable.
    let (fingerprints, profiles, running) = if launcher && token_set {
        shardx_api_counts(st.config_store.dir()).unwrap_or_default()
    } else {
        (None, None, None)
    };
    serde_json::json!({
        "launcherRunning": launcher,
        "mcpInstalled": mcp_installed,
        "tokenSet": token_set,
        "fingerprintCount": fingerprints,
        "profileCount": profiles,
        "runningCount": running,
        "api": SHARDX_API_DEFAULT,
        "mcpDir": mcp_dir.display().to_string(),
    })
}

/// (fingerprints, profiles, running) live counts — the health signal the
/// empty-library bug hid. Each is independently optional so one bad
/// endpoint can't blank the whole row.
fn shardx_api_counts(store_dir: &std::path::Path) -> Option<(Option<usize>, Option<usize>, Option<usize>)> {
    let (api, auth, http) = shardx_api(store_dir)?;
    Some((
        shardx_count_path(&http, &api, &auth, "/fingerprints"),
        shardx_count_path(&http, &api, &auth, "/profiles"),
        shardx_count_path(&http, &api, &auth, "/running"),
    ))
}

/// GET a list endpoint and report its array length — the single request the
/// status counts share. None on any failure so one dead endpoint can't
/// blank the row.
fn shardx_count_path(http: &ureq::Agent, api: &str, auth: &str, path: &str) -> Option<usize> {
    http.get(&format!("{api}{path}"))
        .header("Authorization", auth)
        .call()
        .ok()
        .and_then(|mut r| r.body_mut().read_json::<Value>().ok())
        .and_then(|v| v.as_array().map(|a| a.len()))
}

/// Store the launcher's Bearer token in the OS keychain — never config.json.
#[tauri::command(async)]
pub fn shardx_set_token(app: AppHandle, token: String) -> Result<(), String> {
    let dir = state(&app).config_store.dir().to_path_buf();
    shardx_set_token_inner(&token, &dir)
}

fn shardx_set_token_inner(token: &str, store_dir: &std::path::Path) -> Result<(), String> {
    let t = token.trim();
    if t.is_empty() {
        return Err("empty token".into());
    }
    // Verify-after-write, DPAPI-file fallback — a dropped token write breaks
    // every shardx call (401s) silently.
    crate::misc::secret_set_persist(SHARDX_TOKEN_KEY, t, store_dir).map(|_| ())
}

/// Downloads that hang forever leave the settings card's every button
/// disabled — ureq's default agent has NO timeout, so a blocked GitHub
/// connection kept `busy` pinned indefinitely. Bound every fetch.
fn shardx_http(timeout_secs: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(timeout_secs)))
        .build()
        .new_agent()
}

/// Same bounded-wait for child processes: npm install behind a proxy or a
/// dead network blocks the settings card forever otherwise.
fn wait_or_kill(mut child: std::process::Child, secs: u64) -> bool {
    for _ in 0..secs {
        match child.try_wait() {
            Ok(Some(s)) => return s.success(),
            Ok(None) => std::thread::sleep(std::time::Duration::from_secs(1)),
            Err(_) => return false,
        }
    }
    crate::misc::kill_process_tree(&mut child);
    false
}

/// Hard ceiling for any single ShardX download (launcher exe ~100MB, engine
/// zip a few hundred MB) — a hostile/broken server can't fill the disk.
const MAX_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;

/// `<dest>.part` — downloads land here and are renamed only once complete, so
/// a killed/failed transfer never leaves a truncated file under the final name.
fn part_path(dest: &Path) -> PathBuf {
    let mut p = dest.as_os_str().to_owned();
    p.push(".part");
    PathBuf::from(p)
}

/// Stream a response body straight to disk. ureq's buffered readers
/// (read_to_vec/read_to_string/read_json) carry a 10 MiB default cap —
/// binary downloads (launcher exe ~100MB, repo tarballs, fingerprint
/// bundles) exceed it, so they stream through as_reader() instead. Writes to
/// `<dest>.part` and renames on success; capped at MAX_DOWNLOAD_BYTES.
fn download_to_file(resp: &mut ureq::http::Response<ureq::Body>, dest: &Path) -> Result<u64, String> {
    use std::io::Read as _;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let part = part_path(dest);
    let result = (|| -> Result<u64, String> {
        let mut out = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        let mut capped = resp.body_mut().as_reader().take(MAX_DOWNLOAD_BYTES + 1);
        let n = std::io::copy(&mut capped, &mut out).map_err(|e| e.to_string())?;
        if n > MAX_DOWNLOAD_BYTES {
            return Err(format!("download exceeds the {} MiB cap", MAX_DOWNLOAD_BYTES >> 20));
        }
        drop(out);
        std::fs::rename(&part, dest).map_err(|e| e.to_string())?;
        Ok(n)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// The system bsdtar. Bare `tar` on Windows can resolve to a GNU tar earlier
/// on PATH (Git for Windows, MSYS) that mishandles `C:\` paths and .zip —
/// System32's tar.exe is the one the extract flags were written for.
fn system_tar() -> PathBuf {
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let p = PathBuf::from(root).join("System32").join("tar.exe");
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("tar")
}

/// Resolve a hostname via DNS-over-HTTPS (Cloudflare's JSON API over the
/// 1.1.1.1 IP literal — the poisoned system resolver never sees the
/// query). The escape hatch for ISP DNS hijacks: Indonesian ISPs answer
/// `pub-*.r2.dev` with an Internet Positif block-page IP, and every TLS
/// handshake then fails on a cert that doesn't match the hostname.
fn doh_resolve(host: &str) -> Option<String> {
    let mut resp = shardx_http(15)
        .get(&format!("https://1.1.1.1/dns-query?name={host}&type=A"))
        .header("accept", "application/dns-json")
        .call()
        .ok()?;
    let v: Value = resp.body_mut().read_json().ok()?;
    v["Answer"]
        .as_array()?
        .iter()
        .filter(|a| a["type"].as_u64() == Some(1))
        .filter_map(|a| a["data"].as_str())
        .find(|ip| ip.parse::<std::net::Ipv4Addr>().is_ok())
        .map(String::from)
}

/// Download `url` to `dest` with a DNS-hijack fallback: try the direct
/// fetch first; when it fails, resolve the host over DoH and re-fetch
/// through `curl --resolve host:443:<ip>` — the connection goes to the
/// real IP while SNI and Host keep the original domain, so TLS verifies
/// normally. curl ships in Windows' System32 and on every POSIX box.
fn download_url(url: &str, dest: &Path, timeout_secs: u64) -> Result<u64, String> {
    match (|| {
        let mut resp = shardx_http(timeout_secs)
            .get(url)
            .header("User-Agent", "agent-office")
            .call()
            .map_err(|e| e.to_string())?;
        download_to_file(&mut resp, dest)
    })() {
        Ok(n) => Ok(n),
        Err(e) => doh_download(url, dest)
            .map_err(|e2| format!("{e} | DoH-pinned retry also failed: {e2}")),
    }
}

fn doh_download(url: &str, dest: &Path) -> Result<u64, String> {
    let host = url
        .strip_prefix("https://")
        .and_then(|s| s.split('/').next())
        .ok_or("not an https url")?
        .to_string();
    let ip = doh_resolve(&host).ok_or("DoH resolution failed")?;
    // Bounded like the direct path (never hang the settings card), and
    // written to <dest>.part then renamed — curl dying mid-transfer must not
    // leave a truncated file under the final name.
    let part = part_path(dest);
    let st = std::process::Command::new("curl")
        .args(["-fsSL", "--resolve", &format!("{host}:443:{ip}")])
        .args(["--connect-timeout", "20", "--max-time", "900"])
        .args(["--max-filesize", &MAX_DOWNLOAD_BYTES.to_string()])
        .arg("-o")
        .arg(&part)
        .arg(url)
        .status()
        .map_err(|e| format!("curl spawn failed: {e}"))?;
    if !st.success() {
        let _ = std::fs::remove_file(&part);
        return Err(format!("curl --resolve exit {}", st.code().unwrap_or(-1)));
    }
    if let Err(e) = std::fs::rename(&part, dest) {
        let _ = std::fs::remove_file(&part);
        return Err(e.to_string());
    }
    std::fs::metadata(dest).map_err(|e| e.to_string()).map(|m| m.len())
}

/// ProxyShard/ShardBrowser commit the vendored MCP bridge is built from —
/// pinned (not refs/heads/main) so a compromised or broken push can't reach
/// users. Bump: `curl -s https://api.github.com/repos/ProxyShard/ShardBrowser/commits/main`
/// → `.sha`, review the `mcp/` diff (and package.json scripts/deps), update
/// this constant. Pinned 2026-10-14 (commit dated 2026-09-13).
const SHARDX_MCP_COMMIT: &str = "4880c3fd1a6360bc205daa5bc5542b16277201c3";

/// Vendor the ShardX MCP source into <harness>/mcp-servers/shardx/ and npm
/// install it with the browser download skipped (patchright only ever
/// connectOverCDP to a running ShardX profile — its own chromium is dead
/// weight, and 300MB+ of it). Windows ships bsdtar as tar.exe, so the GitHub
/// tarball extracts without new deps.
fn vendor_shardx_mcp(dest: &std::path::Path) -> Result<(), String> {
    let tmp = std::env::temp_dir().join(format!("shardx-mcp-{}.tgz", uuid::Uuid::new_v4()));
    let out = std::env::temp_dir().join(format!("shardx-src-{}", uuid::Uuid::new_v4()));
    let step = || -> Result<(), String> {
        let mut resp = shardx_http(120)
            .get(&format!(
                "https://codeload.github.com/ProxyShard/ShardBrowser/tar.gz/{SHARDX_MCP_COMMIT}"
            ))
            .call()
            .map_err(|e| format!("download failed: {e}"))?;
        download_to_file(&mut resp, &tmp)?;
        std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        let ok = std::process::Command::new(system_tar())
            .args(["-xzf"])
            .arg(&tmp)
            .arg("-C")
            .arg(&out)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            return Err("tar extract failed".into());
        }
        // The repo ships mcp/ at root (newer) or rust/shardx-launcher/mcp/.
        let root = std::fs::read_dir(&out).map_err(|e| e.to_string())?.flatten().next();
        let Some(root) = root else { return Err("empty archive".into()) };
        for cand in [root.path().join("mcp"), root.path().join("rust").join("shardx-launcher").join("mcp")] {
            if cand.join("index.js").exists() {
                std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
                crate::hive::copy_dir(&cand, &dest).map_err(|e| e.to_string())?;
                // npm install — browser download flags keep node_modules
                // small; a dead registry connection is killed at 4 min.
                // --ignore-scripts: neither the package nor any dependency
                // (patchright, @modelcontextprotocol/sdk, zod) has an
                // install script the bridge needs (verified at the pinned
                // commit), so lifecycle scripts never run. No lockfile
                // ships, hence `install` rather than `ci`.
                let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
                let child = std::process::Command::new(npm)
                    .args(["install", "--ignore-scripts"])
                    .current_dir(&dest)
                    .env("PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD", "1")
                    .env("PATCHRIGHT_SKIP_BROWSER_DOWNLOAD", "1")
                    .spawn()
                    .map_err(|e| format!("npm install failed to start: {e}"))?;
                if !wait_or_kill(child, 240) {
                    return Err("npm install failed or timed out".into());
                }
                return Ok(());
            }
        }
        Err("mcp/ not found in the archive".into())
    };
    let res = step();
    // The tarball and the extraction dir are scratch — drop them on success
    // AND failure (the .tgz was never deleted before).
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_dir_all(&out);
    res
}

#[tauri::command(async)]
pub fn shardx_setup(app: AppHandle) -> Value {
    let st = state(&app);
    let dest = st.hive.root.join("mcp-servers").join("shardx");
    match vendor_shardx_mcp(&dest) {
        Ok(()) => serde_json::json!({ "ok": true, "path": dest.display().to_string() }),
        Err(e) => serde_json::json!({ "ok": false, "error": e }),
    }
}

/// Pull the OS-appropriate launcher asset from the latest GitHub release into
/// ~/Downloads — portable exe on Windows (no admin), AppImage on Linux, dmg on
/// macOS arm64. Returns the path; the user still runs it (first-launch CDN
/// fetch of the engine is theirs too).
#[tauri::command(async)]
pub fn shardx_download_launcher() -> Value {
    let dest_dir = dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_default();
    match shardx_download_launcher_at(
        "https://api.github.com/repos/ProxyShard/ShardBrowser/releases/latest",
        &dest_dir,
    ) {
        Ok(p) => serde_json::json!({ "ok": true, "path": p }),
        Err(e) => serde_json::json!({ "ok": false, "error": e }),
    }
}

/// The fetch itself, with the release-API URL and destination dir injected —
/// a test points both at a localhost stub and a temp dir, no GitHub or
/// ~/Downloads required.
fn shardx_download_launcher_at(release_api: &str, dest_dir: &Path) -> Result<String, String> {
    let rel: Value = shardx_http(30)
        .get(release_api)
        .header("User-Agent", "agent-office")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_json()
        .map_err(|e| e.to_string())?;
    let want = if cfg!(windows) {
        "portable-win-x64.exe"
    } else if cfg!(target_os = "macos") {
        "aarch64.dmg"
    } else {
        "amd64.AppImage"
    };
    let assets = rel["assets"].as_array().cloned().unwrap_or_default();
    let asset = assets
        .iter()
        .find(|a| a["name"].as_str().unwrap_or("").contains(want))
        .ok_or("no asset for this OS")?;
    let url = asset["browser_download_url"]
        .as_str()
        .ok_or("no asset for this OS")?
        .to_string();
    // GitHub exposes `digest: "sha256:<hex>"` on release assets; absent on
    // older releases (then the download is unverified, as before).
    let digest = asset["digest"].as_str().map(String::from);
    let name = url.rsplit('/').next().unwrap_or("shardx-launcher");
    let dest = dest_dir.join(name);
    let mut resp = shardx_http(300)
        .get(&url)
        .header("User-Agent", "agent-office")
        .call()
        .map_err(|e| e.to_string())?;
    download_to_file(&mut resp, &dest)?;
    if let Some(d) = digest {
        verify_sha256_digest(&dest, &d)?;
    }
    Ok(dest.display().to_string())
}

/// Check `path` against a GitHub-style `sha256:<hex>` digest. A mismatch
/// DELETES the file and errors; a digest in an unknown format (or no
/// `sha256:` prefix) is ignored so a future algorithm never blocks installs.
fn verify_sha256_digest(path: &Path, digest: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::io::Read as _;
    let Some(want) = digest.trim().strip_prefix("sha256:") else {
        return Ok(());
    };
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    drop(f);
    let got: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if got.eq_ignore_ascii_case(want.trim()) {
        return Ok(());
    }
    let _ = std::fs::remove_file(path);
    Err(format!("sha256 mismatch for {} (expected {want}, got {got}) — file deleted", path.display()))
}

// ── ShardX bootstrap: one consent → the whole chain ─────────────────────────

/// The launcher's own settings — `$CONFIG/shardx-launcher/settings.json`
/// (upstream store.rs config_root). `ensure_secret` persists `api_secret`
/// (the HS256 signing key) + `api_port` on first launch; reading them lets us
/// mint a valid Bearer token ourselves, so the user never copies anything.
fn shardx_launcher_settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("shardx-launcher").join("settings.json"))
}

fn shardx_launcher_secret() -> Option<(String, u16)> {
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(shardx_launcher_settings_path()?).ok()?)
            .ok()?;
    let secret = v["api_secret"].as_str()?.trim().to_string();
    if secret.is_empty() {
        return None;
    }
    let port = v["api_port"]
        .as_u64()
        .filter(|p| *p > 0 && *p <= u16::MAX as u64)
        .unwrap_or(SHARDX_API_PORT as u64) as u16;
    Some((secret, port))
}

fn b64url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The exact JWT the launcher verifies — HS256, sub "shardx-api", iat+exp —
/// the same claims shape api.rs mint()/long_lived_token() produces for the
/// Settings UI. 10-year default, matching upstream.
fn shardx_mint_token(secret: &str, ttl_secs: u64) -> Result<String, String> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let header = b64url(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = b64url(
        serde_json::json!({
            "sub": "shardx-api", "iat": now, "exp": now.saturating_add(ttl_secs)
        })
        .to_string()
        .as_bytes(),
    );
    let input = format!("{header}.{payload}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|e| e.to_string())?;
    mac.update(input.as_bytes());
    Ok(format!("{input}.{}", b64url(&mac.finalize().into_bytes())))
}

/// settings.json → minted token. Pure file read + sign — no keychain — so a
/// fixture exercises the whole derivation in a unit test.
fn shardx_token_from_settings(path: &std::path::Path) -> Result<String, String> {
    let v: Value = serde_json::from_str(
        &std::fs::read_to_string(path).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let secret = v["api_secret"].as_str().unwrap_or("").trim().to_string();
    if secret.is_empty() {
        return Err("api_secret missing or empty".into());
    }
    shardx_mint_token(&secret, 60 * 60 * 24 * 365 * 10)
}

/// Mint the automation token WITHOUT storing it — the settings card prefills
/// a copyable field with it (the operator may want the token for their own
/// scripts). Read-only; the store path stays shardx_set_token / auto_token.
#[tauri::command(async)]
pub fn shardx_token_preview(app: AppHandle) -> Value {
    let _ = app;
    match shardx_launcher_settings_path() {
        Some(p) => match shardx_token_from_settings(&p) {
            Ok(token) => serde_json::json!({ "ok": true, "token": token }),
            Err(e) => serde_json::json!({ "ok": false, "error": e }),
        },
        None => serde_json::json!({ "ok": false, "error": "launcher settings.json not found" }),
    }
}

/// `<config>/shardx-launcher/fingerprints` — the launcher's library dir.
/// Each .json is one FingerprintConfig; `GET /fingerprints` lists them.
fn shardx_fingerprints_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("shardx-launcher").join("fingerprints"))
}

/// CDN host for the fingerprint bundle — same PUB_BASE the launcher's
/// runtime.rs downloads `ShardX-Fingerprints.zip` from on first run.
const SHARDX_FINGERPRINTS_URL: &str =
    "https://pub-e57a7c60f6934eb09a6600bf2fc59cdc.r2.dev/ShardX-Fingerprints.zip";

/// Engine archive key for this host — same constant set the launcher's
/// runtime.rs host_spec() selects.
fn shardx_engine_key() -> &'static str {
    if cfg!(target_os = "windows") {
        "ShardX-Windows.zip"
    } else if cfg!(target_os = "macos") {
        "ShardX-Mac-arm64.zip"
    } else {
        "ShardX-Linux.zip"
    }
}

/// Engine dir inside the launcher runtime — the host_spec key minus `.zip`.
fn shardx_engine_root() -> &'static str {
    if cfg!(target_os = "windows") {
        "ShardX-Windows"
    } else if cfg!(target_os = "macos") {
        "ShardX-Mac-arm64"
    } else {
        "ShardX-Linux"
    }
}

/// `<data>/shardx-launcher/runtime` — where the launcher expects its
/// engine + manifest.json (mirrors runtime.rs runtime_dir()).
fn shardx_runtime_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("shardx-launcher").join("runtime"))
}

/// Pre-seed the ShardX browser engine so the launcher's own first-run
/// download never has to reach the CDN — the same ISP DNS hijack that
/// serves Internet Positif for pub-*.r2.dev would otherwise kill it
/// outright. Steps: skip when chrome.exe already exists → download the
/// engine zip (direct, then DoH-pinned curl) → PK check → extract into
/// runtime/ → stamp manifest.json with the values runtime_install()
/// checks (installed_chromium_version + the remote archive etags, so
/// engine_outdated() and install_fingerprints() both see "up to date").
fn shardx_seed_engine() -> Result<Value, String> {
    let runtime = shardx_runtime_dir().ok_or("data dir unavailable")?;
    let bin = runtime
        .join(shardx_engine_root())
        .join(if cfg!(windows) { "chrome.exe" } else { "chrome" });
    if bin.exists() {
        return Ok(serde_json::json!({ "ok": true, "already": true }));
    }
    let key = shardx_engine_key();
    let url = format!(
        "https://pub-e57a7c60f6934eb09a6600bf2fc59cdc.r2.dev/{key}"
    );
    let tmp = std::env::temp_dir().join(format!("shardx-eng-{}.zip", uuid::Uuid::new_v4()));
    let result = (|| -> Result<(), String> {
        download_url(&url, &tmp, 300).map_err(|e| format!("engine download failed: {e}"))?;
        {
            use std::io::Read as _;
            let mut f = std::fs::File::open(&tmp).map_err(|e| e.to_string())?;
            let mut magic = [0u8; 2];
            let is_zip = f.read_exact(&mut magic).is_ok() && &magic == b"PK";
            drop(f);
            if !is_zip {
                return Err("engine download was not a zip (blocked by network?)".into());
            }
        }
        std::fs::create_dir_all(&runtime).map_err(|e| e.to_string())?;
        let extracted = std::process::Command::new(system_tar())
            .arg("-xf")
            .arg(&tmp)
            .arg("-C")
            .arg(&runtime)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
            || std::process::Command::new("unzip")
                .arg("-qo")
                .arg(&tmp)
                .arg("-d")
                .arg(&runtime)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
        if !extracted {
            return Err("engine zip extract failed".into());
        }
        if !bin.exists() {
            return Err("engine extracted but chrome binary is missing".into());
        }
        // Remote manifest (GitHub raw — reachable even under the R2 hijack)
        // carries the etags + chromium version runtime_install() compares
        // against. Unreachable → nulls; an unknown local value is never a
        // mismatch in engine_outdated()/install_fingerprints().
        let remote = shardx_fetch_remote_manifest();
        let remote_ver = remote
            .get("chromium_version")
            .and_then(|v| v.as_str())
            .map(String::from);
        // On-disk truth beats the remote manifest: the extracted engine
        // dir carries a `<ver>.manifest` marker on Windows.
        let on_disk_ver = std::fs::read_dir(&runtime.join(shardx_engine_root()))
            .ok()
            .and_then(|rd| {
                rd.flatten().find_map(|e| {
                    let p = e.path();
                    if p.extension().and_then(|s| s.to_str()) != Some("manifest") {
                        return None;
                    }
                    let stem = p.file_stem()?.to_str()?.to_string();
                    (stem.split('.').count() >= 2 && stem.starts_with(|c: char| c.is_ascii_digit()))
                        .then_some(stem)
                })
            });
        let ver = on_disk_ver.or_else(|| remote_ver.clone());
        let archives = remote.get("archives").cloned().unwrap_or(Value::Null);
        let etag = |k: &str| archives.get(k).and_then(|v| v.as_str());
        let manifest = serde_json::json!({
            "browser_etag": etag(key),
            "widevine_etag": null,
            "fingerprints_etag": etag("ShardX-Fingerprints.zip"),
            "applied_chromium_version": ver,
            "installed_chromium_version": ver,
            "installed_engine_build": remote.get("engine_build")
                .and_then(|v| v.as_str().map(String::from)
                    .or_else(|| v.as_u64().map(|n| n.to_string()))),
        });
        std::fs::write(
            runtime.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap_or_default(),
        )
        .map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_file(&tmp);
    result.map(|_| serde_json::json!({ "ok": true }))
}

/// The launcher's remote version manifest (runtime.json on GitHub raw) —
/// Value::Null when unreachable; callers treat every field as optional.
fn shardx_fetch_remote_manifest() -> Value {
    shardx_http(30)
        .get("https://raw.githubusercontent.com/ProxyShard/ShardBrowser/main/runtime.json")
        .header("User-Agent", "agent-office")
        .call()
        .ok()
        .and_then(|mut r| r.body_mut().read_json::<Value>().ok())
        .unwrap_or(Value::Null)
}

/// Seed the fingerprint library when empty — mirrors the launcher's own
/// install_fingerprints(): download the bundle, extract, copy every .json
/// (top dir `shardx-fingerprints` or flat) into the library dir. Returns a
/// step json: {ok, already|seeded}. Skips the download when the library
/// already has entries. Windows' bsdtar extracts zips; elsewhere falls
/// back to `unzip`.
fn shardx_seed_fingerprints(secrets_dir: &std::path::Path) -> Result<Value, String> {
    let (api, auth, http) =
        shardx_api(secrets_dir).ok_or("launcher not running or no token")?;
    let lib = shardx_fingerprints_dir().ok_or("config dir unavailable")?;
    shardx_seed_fingerprints_at(&http, &api, &auth, SHARDX_FINGERPRINTS_URL, &lib)
}

/// The seed pipeline with its edges injected — a test drives the same logic
/// against a localhost stub serving a fixture zip, no CDN and no %APPDATA%.
fn shardx_seed_fingerprints_at(
    http: &ureq::Agent,
    api: &str,
    auth: &str,
    bundle_url: &str,
    lib_dir: &Path,
) -> Result<Value, String> {
    let mut existing = http
        .get(&format!("{api}/fingerprints"))
        .header("Authorization", auth)
        .call()
        .map_err(|e| format!("fingerprints list failed: {e}"))?;
    let count = existing
        .body_mut()
        .read_json::<Value>()
        .ok()
        .and_then(|v| v.as_array().map(|a| a.len()))
        .unwrap_or(0);
    if count > 0 {
        return Ok(serde_json::json!({ "ok": true, "already": count }));
    }

    let tmp = std::env::temp_dir().join(format!("shardx-fp-{}.zip", uuid::Uuid::new_v4()));
    let out = std::env::temp_dir().join(format!("shardx-fp-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<usize, String> {
        download_url(bundle_url, &tmp, 300)
            .map_err(|e| format!("bundle download failed: {e}"))?;
        // A DNS-hijack / captive portal serves an HTML block page with a 200.
        let mut f = std::fs::File::open(&tmp).map_err(|e| e.to_string())?;
        let mut magic = [0u8; 2];
        use std::io::Read as _;
        let is_zip = f.read_exact(&mut magic).is_ok() && &magic == b"PK";
        drop(f);
        if !is_zip {
            return Err("bundle download was not a zip (blocked by network?)".into());
        }
        std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        let extracted = std::process::Command::new(system_tar())
            .arg("-xf")
            .arg(&tmp)
            .arg("-C")
            .arg(&out)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
            || std::process::Command::new("unzip")
                .arg("-qo")
                .arg(&tmp)
                .arg("-d")
                .arg(&out)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
        if !extracted {
            return Err("zip extract failed".into());
        }
        std::fs::create_dir_all(lib_dir).map_err(|e| e.to_string())?;
        // Bundle wraps files in shardx-fingerprints/ when present, else flat.
        let walk = out.join("shardx-fingerprints");
        let walk = if walk.is_dir() { walk } else { out.clone() };
        let mut seeded = 0usize;
        for ent in std::fs::read_dir(&walk).map_err(|e| e.to_string())?.flatten() {
            let p = ent.path();
            if p.extension().and_then(|s| s.to_str()) == Some("json") {
                std::fs::copy(&p, lib_dir.join(p.file_name().unwrap()))
                    .map_err(|e| e.to_string())?;
                seeded += 1;
            }
        }
        if seeded == 0 {
            return Err("bundle had no fingerprint json files".into());
        }
        Ok(seeded)
    })();
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_dir_all(&out);
    result.map(|n| serde_json::json!({ "ok": true, "seeded": n }))
}

/// Read the launcher's secret, mint the long-lived token, keychain it.
/// `settings_path` is injected (bootstrap_impl passes the real one) so the
/// "settings missing" branch is exercised without touching %APPDATA%.
/// Returns the configured api_port so callers can sanity-check SHARDX_API.
fn shardx_auto_token(store_dir: &std::path::Path, settings_path: Option<&Path>) -> Result<u16, String> {
    let path = settings_path.ok_or_else(|| {
        "launcher settings.json not found — run the ShardX launcher once".to_string()
    })?;
    let token = shardx_token_from_settings(&path)?;
    shardx_set_token_inner(&token, store_dir)?;
    Ok(shardx_launcher_secret()
        .map(|(_, p)| p)
        .unwrap_or(SHARDX_API_PORT))
}

/// Where the launcher exe can be: `shardx_download_launcher` drops the
/// portable exe in ~/Downloads; an installer lands under
/// %LOCALAPPDATA%\Programs\<shardx*>\. Shallow scan, name-match "shardx",
/// newest wins.
fn shardx_find_launcher_exe() -> Option<PathBuf> {
    let mut dirs_to_scan = Vec::new();
    if let Some(d) = dirs::download_dir() {
        dirs_to_scan.push(d);
    }
    if let Some(d) = dirs::home_dir().map(|h| h.join("Downloads")) {
        dirs_to_scan.push(d);
    }
    if let Some(d) = dirs::data_local_dir().map(|d| d.join("Programs")) {
        dirs_to_scan.push(d);
    }
    let mut best: Option<(PathBuf, std::time::SystemTime)> = None;
    let consider = |p: PathBuf, m: std::time::SystemTime, best: &mut Option<(PathBuf, std::time::SystemTime)>| {
        if best.as_ref().map(|(_, bm)| m > *bm).unwrap_or(true) {
            *best = Some((p, m));
        }
    };
    for dir in dirs_to_scan {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for ent in rd.flatten() {
            let p = ent.path();
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_lowercase();
            if p.is_dir() && name.contains("shardx") {
                if let Ok(sub) = std::fs::read_dir(&p) {
                    for e in sub.flatten() {
                        let q = e.path();
                        let qn = q
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("")
                            .to_lowercase();
                        if qn.ends_with(".exe") {
                            let m = e
                                .metadata()
                                .and_then(|m| m.modified())
                                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                            consider(q, m, &mut best);
                        }
                    }
                }
                continue;
            }
            if !name.ends_with(".exe") || !name.contains("shardx") {
                continue;
            }
            let m = ent
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            consider(p, m, &mut best);
        }
    }
    best.map(|(p, _)| p)
}

/// One consent → the whole chain: launcher up (auto-start the downloaded exe
/// when it isn't), token minted + keychained from the launcher's own
/// settings.json, MCP bridge vendored. Steps report independently so a
/// partial run stays retryable — `ok` only when all three gates are green.
/// `try_launch` exists so tests never spawn a real exe. Blocking (network +
/// npm), same posture as shardx_setup.
fn shardx_bootstrap_inner(hive_root: &std::path::Path, try_launch: bool) -> Value {
    shardx_bootstrap_impl(
        hive_root,
        try_launch,
        shardx_launcher_secret()
            .map(|(_, p)| p)
            .unwrap_or(SHARDX_API_PORT),
        shardx_launcher_settings_path(),
        shardx_seed_engine,
    )
}

/// The machine-touching bits (which port to probe, which settings.json to
/// mint from, how the engine gets seeded) are parameters so tests point at
/// a dead port + missing file + a no-op seed — a real running launcher and
/// a 150MB CDN pull must not leak into a "gate reporting" test.
fn shardx_bootstrap_impl(
    hive_root: &std::path::Path,
    try_launch: bool,
    port: u16,
    settings_path: Option<PathBuf>,
    seed_engine: fn() -> Result<Value, String>,
) -> Value {
    let mut steps = serde_json::json!({});

    let mut launcher_up = std::net::TcpStream::connect(("127.0.0.1", port)).is_ok();
    if !launcher_up && try_launch {
        if let Some(exe) = shardx_find_launcher_exe() {
            steps["launchSpawned"] = serde_json::json!(std::process::Command::new(&exe).spawn().is_ok());
            // First run persists api_secret early and binds the API after the
            // engine check — poll briefly; a cold start simply reports down.
            for _ in 0..30 {
                std::thread::sleep(std::time::Duration::from_millis(500));
                if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    launcher_up = true;
                    break;
                }
            }
        }
    }
    steps["launcher"] = serde_json::json!({ "ok": launcher_up });

    // Secrets live in harness home (config.json's dir) — hive_root.parent().
    let secrets_dir = hive_root.parent().unwrap_or(hive_root).to_path_buf();
    match shardx_auto_token(&secrets_dir, settings_path.as_deref()) {
        Ok(api_port) => steps["token"] = serde_json::json!({ "ok": true, "apiPort": api_port }),
        Err(e) => steps["token"] = serde_json::json!({ "ok": false, "error": e }),
    }

    let dest = hive_root.join("mcp-servers").join("shardx");
    if dest.join("index.js").exists() && dest.join("node_modules").is_dir() {
        steps["bridge"] = serde_json::json!({ "ok": true, "already": true });
    } else {
        match vendor_shardx_mcp(&dest) {
            Ok(()) => steps["bridge"] = serde_json::json!({ "ok": true }),
            Err(e) => steps["bridge"] = serde_json::json!({ "ok": false, "error": e }),
        }
    }

    // Empty fingerprint library blocks ALL profile creation — the launcher
    // seeds it from its CDN on first run and fails silently when the CDN is
    // unreachable (ISP DNS hijack returns a block page, not an error).
    // Reproduce install_fingerprints() here so setup actually works.
    if launcher_up && steps["token"]["ok"].as_bool() == Some(true) {
        steps["fingerprints"] = match shardx_seed_fingerprints(&secrets_dir) {
            Ok(v) => v,
            Err(e) => serde_json::json!({ "ok": false, "error": e }),
        };
    }

    // Engine pre-seed — the launcher's own first-run engine download
    // (~150MB from the same poisoned CDN host) dies on the DNS hijack.
    // Landing it here (manifest stamped so runtime_install sees "up to
    // date") means the launcher skips that step entirely. Files only —
    // independent of launcher_up.
    steps["engine"] = match seed_engine() {
        Ok(v) => v,
        Err(e) => serde_json::json!({ "ok": false, "error": e }),
    };

    let ok = ["launcher", "token", "bridge", "fingerprints", "engine"]
        .iter()
        // A skipped seed (launcher down / no token) isn't a bootstrap failure.
        .filter(|k| steps[*k].is_object())
        .all(|k| steps[k]["ok"].as_bool().unwrap_or(false));
    serde_json::json!({ "ok": ok, "steps": steps })
}

/// "Install ShardX? yes" → bootstrap. The renderer's consent dialog is the
/// only prompt; everything else — launcher start, token, MCP vendor — is
/// automatic. A SUCCESSFUL setup also flips the shardx consent: clicking Set
/// up IS the operator opting this keyed MCP on — agents should never need a
/// second trip into Settings just to arm it.
#[tauri::command(async)]
pub fn shardx_bootstrap(app: AppHandle) -> Value {
    let st = state(&app);
    let root = st.hive.root.clone();
    let out = shardx_bootstrap_inner(&root, true);
    if out["ok"].as_bool() == Some(true) {
        let mut cfg = st.config.lock().unwrap();
        let entry = cfg.mcp_defaults.entry("shardx".into()).or_default();
        if !entry.enabled {
            entry.enabled = true;
            let _ = st.config_store.save(&cfg);
        }
    }
    out
}

// ── Per-agent ShardX profiles ───────────────────────────────────────────────

/// The setup gates a shardx-armed spawn actually needs: vendored MCP bridge +
/// a stored automation token. The launcher is NOT a gate — the agent can
/// start it itself (SHARDX.md teaches the exe paths) and bootstrap does too.
/// Extracted so the needsShardx flag is testable without a spawn.
fn shardx_setup_ready(hive_root: &Path, secrets_dir: &Path) -> bool {
    hive_root
        .join("mcp-servers")
        .join("shardx")
        .join("index.js")
        .exists()
        && crate::misc::secret_get_persist("SHARDX_TOKEN", secrets_dir).is_some()
}

/// Deterministic per-agent profile name. Agent ids are slugged already;
/// sanitize anyway so a weird id can't collide two profiles into one.
fn shardx_profile_name(agent_id: &str) -> String {
    let slug: String = agent_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("office-{slug}")
}

/// Which hive a profile belongs to — first 8 hex of sha256(hive root). The
/// launcher's profiles are GLOBAL across workspaces; without this tag a
/// cleanup pass on workspace A could nuke auto-profiles belonging to B.
fn hive_tag(root: &std::path::Path) -> String {
    use sha2::Digest;
    let h = sha2::Sha256::digest(root.to_string_lossy().as_bytes());
    h.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

/// GET-or-CREATE a launcher profile by name. `profile_name` is the identity
/// contract: auto-spawns pass `office-<agentId>` (one agent, one identity);
/// a user-bound name binds the agent to an ACCOUNT's existing profile so
/// sibling agents on the same shop share one login instead of multiplying
/// fingerprints. None on any failure: the spawn proceeds, the shardx tools
/// just can't reach the launcher yet.
fn shardx_ensure_profile(
    agent_id: &str,
    profile_name: &str,
    hive_root: &std::path::Path,
) -> Option<String> {
    let port = shardx_launcher_secret()
        .map(|(_, p)| p)
        .unwrap_or(SHARDX_API_PORT);
    if std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        return None;
    }
    let token = crate::misc::secret_get_persist(
        SHARDX_TOKEN_KEY,
        hive_root.parent().unwrap_or(hive_root),
    )?;
    let api = format!("http://127.0.0.1:{port}");
    let auth = format!("Bearer {token}");
    let http = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(8)))
        .build()
        .new_agent();
    let mut resp = http
        .get(&format!("{api}/profiles"))
        .header("Authorization", &auth)
        .call()
        .ok()?;
    let list: Value = resp.body_mut().read_json().ok()?;
    if let Some(hit) = list
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["name"].as_str() == Some(profile_name))
    {
        return hit["id"].as_str().map(String::from);
    }
    let mut resp = http
        .get(&format!("{api}/fingerprint/new"))
        .header("Authorization", &auth)
        .call()
        .ok()?;
    let fp: Value = resp.body_mut().read_json().ok()?;
    let mut resp = http
        .post(&format!("{api}/profiles"))
        .header("Authorization", &auth)
        .send_json(serde_json::json!({
            "name": profile_name,
            "notes": format!(
                "office agent {agent_id} | hive:{} | auto-created, do not reuse",
                hive_tag(hive_root)
            ),
            "fingerprint": fp["fingerprint"].clone(),
        }))
        .ok()?;
    let created: Value = resp.body_mut().read_json().ok()?;
    created["id"]
        .as_str()
        .or_else(|| created["_meta"]["id"].as_str())
        .map(String::from)
}

/// Which `office-*` profiles are safe to delete: tagged to THIS hive (a
/// sibling workspace's profiles are invisible-but-real — never touch them)
/// AND bound to no registry entry. `bound` carries both derived auto names
/// (`office-<id>`) and any user-bound profile names from registry extras.
fn shardx_cleanup_plan(profiles: &[Value], bound: &std::collections::HashSet<String>, tag: &str) -> Vec<String> {
    profiles
        .iter()
        .filter(|p| {
            let name = p["name"].as_str().unwrap_or("");
            let notes = p["notes"].as_str().unwrap_or("");
            name.starts_with("office-")
                && notes.contains(&format!("hive:{tag}"))
                && !bound.contains(name)
        })
        .filter_map(|p| p["id"].as_str().map(String::from))
        .collect()
}

/// Shared launcher-API plumbing: resolves the port, mint-token keychain read,
/// and an authorized ureq agent. None when the launcher or token is missing —
/// callers map that to a short "not running"/"no token" reason.
fn shardx_api(store_dir: &std::path::Path) -> Option<(String, String, ureq::Agent)> {
    let port = shardx_launcher_secret()
        .map(|(_, p)| p)
        .unwrap_or(SHARDX_API_PORT);
    std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    let token = crate::misc::secret_get_persist(SHARDX_TOKEN_KEY, store_dir)?;
    let http = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(8)))
        .build()
        .new_agent();
    Some((
        format!("http://127.0.0.1:{port}"),
        format!("Bearer {token}"),
        http,
    ))
}

/// Profile inventory for the renderer: every launcher profile annotated with
/// running state and which hive agent owns it (registry `shardxProfile` match
/// or the `office-<agentId>` naming convention). Read-only — this is the list
/// a user sees instead of opening the launcher GUI.
#[tauri::command(async)]
pub fn shardx_profiles(app: AppHandle) -> Value {
    let st = state(&app);
    let Some((api, auth, http)) = shardx_api(st.config_store.dir()) else {
        return serde_json::json!({ "ok": false, "error": "launcher not running or no token" });
    };
    let Ok(mut resp) = http
        .get(&format!("{api}/profiles"))
        .header("Authorization", &auth)
        .call()
    else {
        return serde_json::json!({ "ok": false, "error": "profiles list failed" });
    };
    let Ok(list) = resp.body_mut().read_json::<Value>() else {
        return serde_json::json!({ "ok": false, "error": "unexpected profiles payload" });
    };
    let running: std::collections::HashSet<String> = http
        .get(&format!("{api}/running"))
        .header("Authorization", &auth)
        .call()
        .ok()
        .and_then(|mut r| r.body_mut().read_json::<Value>().ok())
        .map(|v| {
            v.as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p["profile_id"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    // Profile name → owning agent label, from the registry: explicit bindings
    // plus the auto `office-<id>` convention.
    let owners = shardx_owner_map(&st.hive.registry());
    let profiles: Vec<Value> = list
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            let name = p["name"].as_str().unwrap_or("");
            serde_json::json!({
                "id": p["id"],
                "name": name,
                "folder": p["folder"],
                "proxy_id": p["proxy_id"],
                "running": running.contains(p["id"].as_str().unwrap_or("")),
                "owner": owners.get(name),
                "auto": name.starts_with("office-")
                    && p["notes"].as_str().unwrap_or("").contains("hive:"),
            })
        })
        .collect();
    serde_json::json!({ "ok": true, "profiles": profiles })
}

/// Stored proxies for the renderer's per-profile dropdown. Credentials are
/// never returned by the launcher, so all we echo is id/name/kind/endpoint.
#[tauri::command(async)]
pub fn shardx_proxies(app: AppHandle) -> Value {
    let st = state(&app);
    let Some((api, auth, http)) = shardx_api(st.config_store.dir()) else {
        return serde_json::json!({ "ok": false, "error": "launcher not running or no token" });
    };
    match http
        .get(&format!("{api}/proxies"))
        .header("Authorization", &auth)
        .call()
    {
        Ok(mut r) => match r.body_mut().read_json::<Value>() {
            Ok(list) => serde_json::json!({ "ok": true, "proxies": list }),
            Err(_) => serde_json::json!({ "ok": false, "error": "unexpected proxies payload" }),
        },
        Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
    }
}

/// Bind a stored proxy to a profile ("" unbinds) — PATCH /profiles/{id}.
/// The launcher's launcher-side full-test + QUIC/WebRTC gating run on the
/// bind, so agents inherit a leak-checked route without doing anything.
#[tauri::command(async)]
pub fn shardx_set_profile_proxy(
    app: AppHandle,
    profile_id: String,
    proxy_id: String,
) -> Value {
    let st = state(&app);
    let Some((api, auth, http)) = shardx_api(st.config_store.dir()) else {
        return serde_json::json!({ "ok": false, "error": "launcher not running or no token" });
    };
    match shardx_patch_profile_proxy(&http, &api, &auth, &profile_id, &proxy_id) {
        Ok(()) => serde_json::json!({ "ok": true }),
        Err(e) => serde_json::json!({ "ok": false, "error": e }),
    }
}

/// PATCH /profiles/{id} with the proxy binding — `""` unbinds. Extracted so
/// the wire shape (field name, method, path) is testable against a stub.
fn shardx_patch_profile_proxy(
    http: &ureq::Agent,
    api: &str,
    auth: &str,
    profile_id: &str,
    proxy_id: &str,
) -> Result<(), String> {
    http.patch(&format!("{api}/profiles/{profile_id}"))
        .header("Authorization", auth)
        .send_json(serde_json::json!({ "proxy_id": proxy_id }))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// ── Automation projects ─────────────────────────────────────────────────────

/// Every saved automation project — the block graphs the launcher's
/// Automation tab edits. Run/stop here costs zero tokens: replay is the
/// launcher's engine, not an LLM.
#[tauri::command(async)]
pub fn shardx_automations(app: AppHandle) -> Value {
    let st = state(&app);
    let Some((api, auth, http)) = shardx_api(st.config_store.dir()) else {
        return serde_json::json!({ "ok": false, "error": "launcher not running or no token" });
    };
    match http
        .get(&format!("{api}/automation/projects"))
        .header("Authorization", &auth)
        .call()
    {
        Ok(mut r) => match r.body_mut().read_json::<Value>() {
            Ok(list) => serde_json::json!({ "ok": true, "projects": list }),
            Err(_) => serde_json::json!({ "ok": false, "error": "unexpected projects payload" }),
        },
        Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
    }
}

/// POST /automation/projects/{id}/run — 400 means "already running" or "no
/// enabled steps", which the launcher spells out in its error body.
#[tauri::command(async)]
pub fn shardx_run_automation(app: AppHandle, project_id: String) -> Value {
    let st = state(&app);
    match shardx_automation_call(&st, &project_id, "run") {
        Ok(v) => v,
        Err(e) => serde_json::json!({ "ok": false, "error": e }),
    }
}

#[tauri::command(async)]
pub fn shardx_stop_automation(app: AppHandle, project_id: String) -> Value {
    let st = state(&app);
    match shardx_automation_call(&st, &project_id, "stop") {
        Ok(v) => v,
        Err(e) => serde_json::json!({ "ok": false, "error": e }),
    }
}

/// POST {api}/automation/projects/{id}/{action} with the launcher's error
/// body lifted into `error` — a 400 on run carries the reason (already
/// running, no enabled steps) and that's what the card should show.
fn shardx_automation_call(st: &AppState, project_id: &str, action: &str) -> Result<Value, String> {
    let Some((api, auth, http)) = shardx_api(st.config_store.dir()) else {
        return Err("launcher not running or no token".into());
    };
    match shardx_automation_call_at(&http, &api, &auth, project_id, action) {
        Ok(()) => Ok(serde_json::json!({ "ok": true })),
        Err(e) => Err(e),
    }
}

/// The wire call itself — edges injected so a localhost stub can verify the
/// verb, path, and auth header without a real launcher.
fn shardx_automation_call_at(
    http: &ureq::Agent,
    api: &str,
    auth: &str,
    project_id: &str,
    action: &str,
) -> Result<(), String> {
    http.post(&format!("{api}/automation/projects/{project_id}/{action}"))
        .header("Authorization", auth)
        .send_empty()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Runs in flight right now — the card's "running" badges and the scheduler
/// run-log row share this list.
#[tauri::command(async)]
pub fn shardx_automation_runs(app: AppHandle) -> Value {
    let st = state(&app);
    let Some((api, auth, http)) = shardx_api(st.config_store.dir()) else {
        return serde_json::json!({ "ok": false, "error": "launcher not running or no token" });
    };
    match http
        .get(&format!("{api}/automation/runs"))
        .header("Authorization", &auth)
        .call()
    {
        Ok(mut r) => match r.body_mut().read_json::<Value>() {
            Ok(list) => serde_json::json!({ "ok": true, "runs": list }),
            Err(_) => serde_json::json!({ "ok": false, "error": "unexpected runs payload" }),
        },
        Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
    }
}

/// Scheduler entry point for `kind: "shardx"` missions — `body` carries the
/// project id. Returns a one-line result for the hive log instead of mailing
/// a prompt: the run is the mission, there's no agent to read it.
pub fn shardx_fire_automation(st: &AppState, project_id: &str) -> String {
    match shardx_automation_call(st, project_id, "run") {
        Ok(_) => format!("automation run started (project {project_id})"),
        Err(e) => format!("automation run failed (project {project_id}): {e}"),
    }
}

/// Profile name → owning agent label: explicit `extra.shardxProfile` bindings
/// plus the `office-<agentId>` auto-name convention. Display-only — the
/// renderer shows "→ <agent>" next to each profile.
fn shardx_owner_map(
    registry: &[crate::hive::RegistryEntry],
) -> std::collections::HashMap<String, String> {
    registry
        .iter()
        .flat_map(|e| {
            let label = if e.name.is_empty() { e.id.clone() } else { e.name.clone() };
            [
                Some((shardx_profile_name(&e.id), label.clone())),
                e.extra
                    .get("shardxProfile")
                    .and_then(|v| v["name"].as_str())
                    .map(|n| (n.to_string(), label)),
            ]
            .into_iter()
            .flatten()
        })
        .collect()
}

/// Remove auto-created `office-*` profiles that no longer belong to any agent
/// in THIS hive's registry (deleted agents, retired workspaces). User-named
/// profiles and anything another hive created are left alone.
#[tauri::command(async)]
pub fn shardx_cleanup(app: AppHandle) -> Value {
    let st = state(&app);
    let port = shardx_launcher_secret()
        .map(|(_, p)| p)
        .unwrap_or(SHARDX_API_PORT);
    if std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        return serde_json::json!({ "ok": false, "error": "launcher not running" });
    }
    let Some(token) = crate::misc::secret_get_persist(SHARDX_TOKEN_KEY, st.config_store.dir()) else {
        return serde_json::json!({ "ok": false, "error": "no token" });
    };
    let api = format!("http://127.0.0.1:{port}");
    let auth = format!("Bearer {token}");
    let http = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(8)))
        .build()
        .new_agent();
    let Ok(mut resp) = http
        .get(&format!("{api}/profiles"))
        .header("Authorization", &auth)
        .call()
    else {
        return serde_json::json!({ "ok": false, "error": "profiles list failed" });
    };
    let list: Value = resp.body_mut().read_json().unwrap_or(Value::Null);
    let Some(profiles) = list.as_array() else {
        return serde_json::json!({ "ok": false, "error": "unexpected profiles payload" });
    };

    // Bound names: every registry agent's auto name + any explicit binding.
    let bound: std::collections::HashSet<String> = st
        .hive
        .registry()
        .iter()
        .flat_map(|e| {
            [
                Some(shardx_profile_name(&e.id)),
                e.extra
                    .get("shardxProfile")
                    .and_then(|v| v["name"].as_str())
                    .map(String::from),
            ]
            .into_iter()
            .flatten()
        })
        .collect();
    let doomed = shardx_cleanup_plan(profiles, &bound, &hive_tag(&st.hive.root));

    let mut removed = 0usize;
    let mut failed = 0usize;
    for id in &doomed {
        match http
            .delete(&format!("{api}/profiles/{id}"))
            .header("Authorization", &auth)
            .call()
        {
            Ok(_) => removed += 1,
            Err(_) => failed += 1,
        }
    }
    serde_json::json!({
        "ok": failed == 0,
        "removed": removed,
        "kept": profiles.len() - removed - failed,
        "failed": failed,
    })
}

/// Pure seed resolution for the hermes lifecycle bridge: the configured base
/// URL decides the provider (api.deepseek.com → first-class "deepseek", any
/// other URL → the generic "custom" OpenAI-compatible route), the key lookup
/// supplies the credential, and a provider-less config falls back to ANY
/// stored backend key so the CLI never boots into its first-run wizard
/// (agent appears hired but dead). `lookup` is the credential source — the OS
/// keychain in prod, canned maps in tests.
fn hermes_seed_plan(
    cfg: &crate::config::Config,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> crate::hive::HermesSeed {
    let base_url = cfg.provider_base_urls.get("hermes").cloned();
    let provider = base_url.as_deref().map(|u| {
        if u.contains("deepseek.com") { "deepseek" } else { "custom" }.to_string()
    });
    let mut api_key = match provider.as_deref() {
        Some("deepseek") => lookup("PROVIDER_KEY_DEEPSEEK"),
        _ => lookup("PROVIDER_KEY_DEEPSEEK").or_else(|| lookup("PROVIDER_KEY_OPENAI")),
    };
    let (provider, base_url) = if provider.is_some() {
        (provider, base_url)
    } else {
        const FALLBACK: &[(&str, &str, Option<&str>)] = &[
            ("openrouter", "PROVIDER_KEY_OPENROUTER", None),
            ("openai", "PROVIDER_KEY_OPENAI", Some("https://api.openai.com/v1")),
            ("deepseek", "PROVIDER_KEY_DEEPSEEK", Some("https://api.deepseek.com")),
            ("anthropic", "PROVIDER_KEY_ANTHROPIC", None),
        ];
        match FALLBACK.iter().find_map(|(p, k, u)| {
            lookup(k).map(|key| ((*p, Some(key)), u.map(|s| s.to_string())))
        }) {
            Some(((p, key), url)) => {
                api_key = key;
                (Some(p.to_string()), url)
            }
            None => (None, base_url),
        }
    };
    crate::hive::HermesSeed {
        provider,
        base_url,
        model: cfg.provider_default_models.get("hermes").cloned(),
        api_key,
        ..Default::default()
    }
}

/// True when the spawned hermes will reach NO credential from any source: the
/// seed carried no api_key AND no provider-key env landed via secret_env_keys
/// injection (a user CAN name a custom secret "OPENROUTER_API_KEY" verbatim,
/// which bypasses the PROVIDER_KEY_ translation — env is checked too). An
/// authless hermes falls back to its openrouter default and dies on 401s.
fn hermes_needs_key(
    seed: &crate::hive::HermesSeed,
    env: &std::collections::HashMap<String, String>,
) -> bool {
    seed.api_key.is_none()
        && ![
            "OPENROUTER_API_KEY",
            "DEEPSEEK_API_KEY",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
        ]
        .iter()
        .any(|k| env.contains_key(*k))
}

/// The workspace note that tells the agent WHICH browser identity it owns —
/// env vars of the MCP server process are invisible to the agent, so the
/// contract lives here and in $SHARDX_PROFILE on the agent's own env. The
/// operating rules are distilled from the vendored bridge README
/// (<harness>/mcp-servers/shardx/README.md) — the full tool reference the
/// agent can read on demand; this note only carries what must NEVER be
/// learned the hard way.
fn shardx_workspace_note(
    profile_name: &str,
    profile_id: Option<&str>,
    hive_root: &std::path::Path,
) -> String {
    let readme = hive_root
        .join("mcp-servers")
        .join("shardx")
        .join("README.md");
    let id_line = match profile_id {
        Some(pid) => format!("Profile `{profile_name}` exists — profile_id `{pid}`."),
        None => format!(
            "Profile `{profile_name}` is yours but not created yet — create it once with \
             create_profile {{name:\"{profile_name}\"}} and use the returned id."
        ),
    };
    format!(
        "# ShardX browser\n\n\
         {id_line}\n\n\
         - Pass this profile (id or name `{profile_name}`) to every shardx profile/browser call.\n\
         - NEVER use another profile: profiles map 1:1 to accounts and sharing a\n  \
         fingerprint/cookie jar is how marketplaces issue linked-account bans.\n\n\
         ## Operating rules\n\n\
         - browser_* tools AUTO-START the profile — do not manage start/stop yourself.\n\
         - Prefer `human_*` over `browser_*` on login/gated/checkout surfaces:\n  \
         browser_click is instant and looks it; human_* produces real pointer\n  \
         trajectories + log-normal keystrokes inside the browser process.\n\
         - human_* calls take REAL time — budget waits like a person, no click storms.\n\
         - Phone profiles: human_click taps automatically, human_move refuses\n  \
         (no cursor on glass); use touch_* for swipes/drags.\n\
         - Use browser_aria_snapshot to SEE a page before guessing selectors.\n\n\
         ## Reading & capture\n\n\
         - browser_aria_snapshot is the DEFAULT page reader — the compact\n  \
         accessibility tree costs far fewer tokens than browser_content/HTML.\n\
         - browser_pdf saves the page as PDF — invoices, receipts, reports.\n\
         - browser_capture_start/stop logs every finished network request —\n  \
         use it to learn what a page calls before guessing an endpoint.\n\
         - browser_block_resources skips images/fonts/media — scraping gets\n  \
         faster and cheaper; re-enable before visual verification.\n\
         - browser_mock / browser_intercept stub or rewrite requests in\n  \
         flight — testing and offline replays, not live deception of users.\n\
         - browser_wait_for_download catches file downloads; browser_dialog\n  \
         answers JS alert/confirm/prompt.\n\n\
         ## Guardrails\n\n\
         - NEVER export cookies/session data (export_cookies) — sessions live in\n  \
         the profile and stay there.\n\
         - NEVER type passwords, OTP, or captcha answers — hand them to the human.\n\
         - Destructive/financial actions (publish, purchase, refund, campaign\n  \
         changes) require the human's explicit confirmation FIRST.\n\n\
         ## Launcher down? Start it yourself\n\n\
         If tools fail to connect, the ShardX launcher probably isn't running.\n  \
         You can open it yourself — the exe is a portable app:\n\
         - Check `~/Downloads/*shardx*.exe` first, then\n  \
         `%LOCALAPPDATA%/Programs/*shardx*/` for an installed copy.\n\
         - Start it detached (Windows): `cmd /c start \"\" \"<path-to-exe>\"` —\n  \
         it must keep RUNNING while you use profiles; do not block on it.\n\
         - Wait ~3-5s, then verify `profiles_list` answers.\n\
         - Only escalate to the human if no exe exists in either spot — that\n  \
         means the launcher was never installed, which is a settings-card job\n  \
         (Settings → MCP → ShardX → Set up), not something you can fix.\n\
         - The automation API lives on 127.0.0.1:40325 by default; the\n  \
         launcher's settings.json may define a different api_port.\n\n\
         ## Full reference\n\n\
         Read `{}` for the complete 120-tool\n\
         documentation: automation projects (no-code workflows that run without\n\
         you), proxies, extensions, cookies import, network mocking/capture.\n",
        readme.display()
    )
}

/// Install one bundled skill into ~/.claude/skills — same {ok,error} envelope.
#[tauri::command(async)]
pub fn skills_install_bundled(dir_name: String) -> Value {
    match crate::skills::install_bundled_skill(&dir_name) {
        Ok(p) => serde_json::json!({ "ok": true, "path": p }),
        Err(e) => serde_json::json!({ "ok": false, "error": e }),
    }
}

#[tauri::command(async)]
pub fn skills_uninstall(path: String, cwds: Vec<String>) -> Value {
    match crate::skills::uninstall_skill(&path, &cwds) {
        Ok(()) => serde_json::json!({ "ok": true }),
        Err(e) => serde_json::json!({ "ok": false, "error": e }),
    }
}

// ─── trigger ledger (inbound webhook history + verdicts) ────────────────────

#[tauri::command(async)]
pub fn triggers_list(
    app: AppHandle,
) -> Result<Vec<crate::trigger_history::TriggerHistoryEntry>, String> {
    Ok(state(&app).trigger_history.list())
}

#[tauri::command(async)]
pub fn triggers_clear(app: AppHandle, source: Option<String>) -> Result<bool, String> {
    state(&app).trigger_history.clear(source.as_deref());
    Ok(true)
}

// ─── breaker (the beat's data seam + level read) ────────────────────────────

#[tauri::command(async)]
pub fn breaker_level(app: AppHandle, agent_id: String) -> Result<String, String> {
    let lvl = state(&app).breaker.lock().unwrap().level_for(&agent_id);
    Ok(format!("{lvl:?}").to_lowercase())
}

fn err_str(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("oa-cmd-{tag}-{}", uuid::Uuid::new_v4()))
    }

    /// The verbatim AgentSpawnOptions the ported renderer posts via
    /// `window.cth.spawnPty` → invoke('spawn_agent', {args: <this>}).
    fn req(v: serde_json::Value) -> PtySpawnRequest {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn worktree_base_reads_branch_key_and_handles_detached() {
        let b = serde_json::json!({ "branch": "dev", "detached": false });
        assert_eq!(worktree_base_from_branch(&b).as_deref(), Some("dev"));
        let d = serde_json::json!({ "branch": "abc1234", "detached": true });
        assert_eq!(worktree_base_from_branch(&d).as_deref(), Some("HEAD"));
        let u = serde_json::json!({ "branch": "(no commits yet)", "detached": false });
        assert_eq!(worktree_base_from_branch(&u), None);
    }

    #[test]
    fn worktree_seg_is_unique_and_cannot_climb() {
        assert_eq!(worktree_seg("agent-1"), "agent-1");
        // Sanitised ids carry a hash so `a b` and `a-b` never collide.
        let (a, b) = (worktree_seg("a b"), worktree_seg("a-b"));
        assert_eq!(b, "a-b");
        assert_ne!(a, b);
        assert!(a.starts_with("a-b-"));
        // `.`/`..` never reach Path::join as-is.
        for bad in ["..", ".", "", "a/b", "..\\x"] {
            let s = worktree_seg(bad);
            assert!(!s.contains('/') && !s.contains('\\'), "{bad}: {s}");
            assert!(!s.chars().all(|c| c == '.'), "{bad}: {s}");
        }
    }

    #[test]
    fn pty_spawn_request_deserializes_upstream_payload() {
        // The god-boot payload verbatim (useHive): pty id + hive agent meta.
        let r = req(serde_json::json!({
            "id": "pty-god",
            "cwd": "D:/repo",
            "command": "claude",
            "provider": "claude",
            "args": ["--dangerously-skip-permissions"],
            "cols": 100,
            "rows": 30,
            "resume": true,
            "hive": {
                "id": "god", "name": "Michael", "provider": "claude",
                "cwd": "D:/repo", "isGod": true, "role": "orchestrator (god)"
            }
        }));
        assert_eq!(r.id, "pty-god");
        assert_eq!(r.command, "claude");
        assert_eq!(r.args, vec!["--dangerously-skip-permissions"]);
        let h = r.hive.as_ref().unwrap();
        assert_eq!(h.id, "god");
        assert_eq!(h.name.as_deref(), Some("Michael"));
        assert!(h.is_god);
        assert!(r.resume);
    }

    #[test]
    fn translate_splits_pty_and_agent_namespaces() {
        let r = req(serde_json::json!({
            "id": "pty-a1", "cwd": "C:/w", "command": "claude",
            "hive": { "id": "a1", "name": "Ada", "role": "dev", "isGod": false,
                      "capabilities": ["rust", "sql"] }
        }));
        let (args, plan) = translate_spawn_request(&r).unwrap();
        // The plan carries BOTH ids — pty id for the terminal map, agent id
        // for the registry/AGENT_ID/hooks namespace.
        assert_eq!(plan.pty_id.as_deref(), Some("pty-a1"));
        assert_eq!(plan.agent_id.as_deref(), Some("a1"));
        assert_eq!(args.provider, "claude");
        assert_eq!(args.name, "Ada"); // hive.name — never a required top-level field
        assert_eq!(args.capabilities, vec!["rust", "sql"]);
        // Renderer spawns never auto-type — the seed rides back as seedPrompt.
        assert!(!plan.auto_type_seed);
    }

    #[test]
    fn translate_legacy_spawn_collapses_namespaces() {
        // A hive-less spawn (internal/debug path) keeps the old single-id
        // behaviour: agent id falls back to the pty id.
        let r = req(serde_json::json!({
            "id": "pty-x", "cwd": "C:/w", "command": "cmd"
        }));
        let (args, plan) = translate_spawn_request(&r).unwrap();
        assert_eq!(plan.agent_id.as_deref(), Some("pty-x"));
        assert_eq!(args.provider, "custom"); // unknown binary → custom preset
    }

    #[test]
    fn translate_rejects_invalid_options() {
        for v in [
            serde_json::json!({"id": "", "cwd": "x", "command": "cmd"}),
            serde_json::json!({"id": "pty-a", "cwd": "x", "command": " "}),
        ] {
            let r = req(v);
            assert!(translate_spawn_request(&r).is_err());
        }
    }

    #[test]
    fn translate_resume_precedence_explicit_wins() {
        // index.ts:2871 — a typed resumeSessionId beats `resume: true`; bare
        // `resume` becomes the registry lookup (LastSession).
        let explicit = req(serde_json::json!({
            "id": "pty-a", "cwd": "C:/w", "command": "claude",
            "resume": true, "resumeSessionId": "sess-42"
        }));
        assert!(matches!(
            translate_spawn_request(&explicit).unwrap().1.resume,
            Some(ResumeRequest::Explicit(ref s)) if s == "sess-42"
        ));
        let last = req(serde_json::json!({
            "id": "pty-a", "cwd": "C:/w", "command": "claude", "resume": true
        }));
        assert!(matches!(
            translate_spawn_request(&last).unwrap().1.resume,
            Some(ResumeRequest::LastSession)
        ));
        let none = req(serde_json::json!({
            "id": "pty-a", "cwd": "C:/w", "command": "claude"
        }));
        assert!(translate_spawn_request(&none).unwrap().1.resume.is_none());
    }

    #[test]
    fn infer_provider_matches_upstream_table() {
        for (bin, want) in [
            ("claude", "claude"),
            ("C:/tools/claude.cmd", "claude"),
            ("codex", "codex"),
            ("grok", "grok"),
            ("kimi", "kimi"),
            ("gemini", "gemini"),
            ("agy", "antigravity"),
            ("opencode", "opencode"),
            ("cursor-agent", "cursor"),
            ("my-own-tool", "custom"),
        ] {
            assert_eq!(infer_agent_provider(bin, None), want, "{bin}");
        }
        // An explicit KNOWN provider overrides binary sniffing; an unknown
        // explicit value falls through to the binary (upstream inferAgentProvider).
        assert_eq!(infer_agent_provider("cmd", Some("claude")), "claude");
        assert_eq!(infer_agent_provider("cmd", Some("bogus")), "custom");
    }

    #[test]
    fn spawn_result_serializes_upstream_shape() {
        // The renderer reads res.ok / res.seedPrompt / res.resumed /
        // res.resumeNotFound — camelCase, optionals absent when unset.
        let ok = SpawnOutcome {
            pty_id: "pty-a".into(),
            cwd: "C:/w".into(),
            seed_prompt: Some("do it".into()),
            resumed: false,
            resume_not_found: true,
            worktree_path: Some("C:/w/.worktrees/a".into()),
            needs_key: None,
            needs_shardx: false,
        }
        .into_result();
        let v = serde_json::to_value(&ok).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["id"], "pty-a");
        assert_eq!(v["cwd"], "C:/w");
        assert_eq!(v["seedPrompt"], "do it");
        assert_eq!(v["resumed"], false);
        assert_eq!(v["resumeNotFound"], true);
        assert_eq!(v["worktreePath"], "C:/w/.worktrees/a");
        assert!(v.get("error").is_none());
        // needsKey rides the same camelCase wire surface when set.
        assert!(v.get("needsKey").is_none());
        let authless = SpawnOutcome {
            pty_id: "pty-b".into(),
            cwd: "C:/w".into(),
            seed_prompt: None,
            resumed: false,
            resume_not_found: false,
            worktree_path: None,
            needs_key: Some("hermes".into()),
            needs_shardx: false,
        }
        .into_result();
        let v = serde_json::to_value(&authless).unwrap();
        assert_eq!(v["needsKey"], "hermes");
        // needsShardx rides the same wire surface — absent when false.
        assert!(v.get("needsShardx").is_none());
        let unmet = SpawnOutcome {
            pty_id: "pty-c".into(),
            cwd: "C:/w".into(),
            seed_prompt: None,
            resumed: false,
            resume_not_found: false,
            worktree_path: None,
            needs_key: None,
            needs_shardx: true,
        }
        .into_result();
        let v = serde_json::to_value(&unmet).unwrap();
        assert_eq!(v["needsShardx"], true);

        let bad = PtySpawnResult::err("nope");
        let v = serde_json::to_value(&bad).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "nope");
        // Failure payloads still carry resumed:false (upstream spreads
        // resumed only on success — but the flag itself is always present).
        assert_eq!(v["resumed"], false);
    }

    #[test]
    fn pty_agent_maps_roundtrip() {
        let st = AppState::new(tmp_home("map")).unwrap();
        st.map_pty("pty-god", "god");
        assert_eq!(st.pty_for_agent("god").as_deref(), Some("pty-god"));
        assert_eq!(st.agent_for_pty("pty-god").as_deref(), Some("god"));
        // resolve_pty_id accepts the agent id even with no live-pty key.
        assert_eq!(st.resolve_pty_id("god").as_deref(), Some("pty-god"));
        assert_eq!(st.unmap_pty("pty-god").as_deref(), Some("god"));
        assert!(st.pty_for_agent("god").is_none());
        assert!(st.agent_for_pty("pty-god").is_none());
    }

    #[test]
    fn spawn_registers_both_namespaces_and_kill_accepts_agent_id() {
        let app = tauri::test::mock_app();
        let handle = app.app_handle().clone();
        let home = tmp_home("spawn");
        let cwd = home.display().to_string();
        handle.manage(AppState::new(home.clone()).unwrap());
        // A bare `cmd` stays alive — no natural-exit race.
        let res = spawn_agent_core(
            &handle,
            req(serde_json::json!({
                "id": "pty-ta", "cwd": cwd, "command": "cmd",
                "hive": { "id": "ta", "name": "Tab", "role": "dev" }
            })),
        );
        assert!(res.ok, "spawn failed: {:?}", res.error);
        assert_eq!(res.id.as_deref(), Some("pty-ta"));
        let st = state(&handle);
        // PTY namespace: keyed under the renderer's pty id, NOT the agent id.
        assert!(st.ptys.lock().unwrap().contains_key("pty-ta"));
        assert!(!st.ptys.lock().unwrap().contains_key("ta"));
        // Agent namespace: registry row + the pty↔agent mapping.
        assert!(st.hive.registry().iter().any(|a| a.id == "ta"));
        assert_eq!(st.pty_for_agent("ta").as_deref(), Some("pty-ta"));
        assert_eq!(st.agent_for_pty("pty-ta").as_deref(), Some("ta"));

        // Duplicate pty id → upstream's exact refusal.
        let dup = spawn_agent_core(
            &handle,
            req(serde_json::json!({
                "id": "pty-ta", "cwd": cwd, "command": "cmd",
                "hive": { "id": "ta2", "name": "Tab2" }
            })),
        );
        assert!(!dup.ok);
        assert!(dup.error.unwrap().contains("pty already exists"));

        // Kill by AGENT id (voice/breaker path) resolves the mapped pty.
        let k = kill_agent_core(&handle, "ta");
        assert!(k.ok);
        assert!(st.ptys.lock().unwrap().is_empty());
        assert!(st.pty_for_agent("ta").is_none());
        assert!(st.agent_for_pty("pty-ta").is_none());
        assert!(st
            .hive
            .registry()
            .iter()
            .find(|a| a.id == "ta")
            .is_some_and(|a| a.status == "gone"));

        // Unknown id → upstream's `no pty: <id>` (colon included — Restart &
        // Continue matches that prefix).
        let miss = kill_agent_core(&handle, "pty-missing");
        assert!(!miss.ok);
        assert_eq!(miss.error.as_deref(), Some("no pty: pty-missing"));

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn require_resume_refuses_when_session_unresolvable() {
        let app = tauri::test::mock_app();
        let handle = app.app_handle().clone();
        let home = tmp_home("resume");
        let cwd = home.display().to_string();
        handle.manage(AppState::new(home.clone()).unwrap());
        // Restart & Continue on a session that can't be seeded must REFUSE —
        // never silently start a fresh one (upstream requireResume).
        let res = spawn_agent_core(
            &handle,
            req(serde_json::json!({
                "id": "pty-rr", "cwd": cwd, "command": "claude",
                "provider": "claude",
                "resumeSessionId": "00000000-0000-0000-0000-000000000000",
                "requireResume": true,
                "hive": { "id": "rr", "name": "Res" }
            })),
        );
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("could not be resumed"));
        // Nothing spawned — the pty map stays empty.
        assert!(state(&handle).ptys.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Killing every agent drains EVERY namespace — ptys map, both directions
    /// of the pty↔agent index, and each registry row lands on "gone". This is
    /// the "close the app → all agents die" contract: nothing may outlive its
    /// kill.
    #[test]
    fn kill_all_agents_drains_every_namespace() {
        let app = tauri::test::mock_app();
        let handle = app.app_handle().clone();
        let home = tmp_home("killall");
        let cwd = home.display().to_string();
        handle.manage(AppState::new(home.clone()).unwrap());
        let st = state(&handle);

        for id in ["g", "w1", "w2"] {
            let res = spawn_agent_core(
                &handle,
                req(serde_json::json!({
                    "id": format!("pty-{id}"), "cwd": cwd, "command": "cmd",
                    "hive": { "id": id, "name": id, "isGod": id == "g" }
                })),
            );
            assert!(res.ok, "spawn {id} failed: {:?}", res.error);
        }
        assert_eq!(st.ptys.lock().unwrap().len(), 3);

        for id in ["g", "w1", "w2"] {
            let k = kill_agent_core(&handle, id);
            assert!(k.ok, "kill {id} failed: {:?}", k.error);
        }

        assert!(st.ptys.lock().unwrap().is_empty(), "ptys map must drain");
        for id in ["g", "w1", "w2"] {
            assert!(st.pty_for_agent(id).is_none(), "{id} still has a pty");
            assert!(st.agent_for_pty(&format!("pty-{id}")).is_none());
            assert_eq!(
                st.hive.registry().iter().find(|a| a.id == id).unwrap().status,
                "gone",
                "{id} registry row must read gone"
            );
        }

        let _ = std::fs::remove_dir_all(&home);
    }

    /// Offline skills reach the agent: spawn copies the whole bundled pack into
    /// <agent_dir>/.claude/skills so a cwd=agent_dir session (god) discovers
    /// them exactly like a user's own ~/.claude/skills.
    #[test]
    fn spawn_stages_bundled_skills_into_agent_dir() {
        let app = tauri::test::mock_app();
        let handle = app.app_handle().clone();
        let home = tmp_home("bundled-spawn");
        let cwd = home.display().to_string();
        handle.manage(AppState::new(home.clone()).unwrap());
        let st = state(&handle);

        let res = spawn_agent_core(
            &handle,
            req(serde_json::json!({
                "id": "pty-b1", "cwd": cwd, "command": "cmd",
                "hive": { "id": "b1", "name": "Bundled", "role": "dev" }
            })),
        );
        assert!(res.ok, "spawn failed: {:?}", res.error);

        let skills_dir = st
            .hive
            .agent_dir("b1")
            .join(".claude")
            .join("skills");
        assert!(skills_dir.is_dir(), "bundled skills must land in the agent dir");
        assert!(
            std::fs::read_dir(&skills_dir).unwrap().count() >= 10,
            "expected the bundled pack, got an empty/near-empty dir"
        );
        assert!(skills_dir.join("meta-ads").join("SKILL.md").exists());

        let _ = kill_agent_core(&handle, "b1");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A spawn that fails AFTER registration (cwd missing) must not leak a
    /// live-looking registry row, a pty slot, a pty↔agent mapping or a
    /// worktree record.
    #[test]
    fn failed_spawn_cleans_up_after_itself() {
        let app = tauri::test::mock_app();
        let handle = app.app_handle().clone();
        let home = tmp_home("spawn-fail");
        handle.manage(AppState::new(home.clone()).unwrap());
        let st = state(&handle);

        let res = spawn_agent_core(
            &handle,
            req(serde_json::json!({
                "id": "pty-f1", "cwd": home.join("does-not-exist").display().to_string(),
                "command": "cmd",
                "hive": { "id": "f1", "name": "Ghost", "role": "dev" }
            })),
        );
        assert!(!res.ok, "a missing cwd must fail the spawn");
        assert!(st.ptys.lock().unwrap().is_empty());
        assert!(st.pty_to_agent.lock().unwrap().is_empty());
        assert!(st.worktree_paths.lock().unwrap().is_empty());
        let row = st.hive.registry().into_iter().find(|a| a.id == "f1");
        assert_ne!(
            row.map(|a| a.status).as_deref(),
            Some("idle"),
            "failed spawn must not leave an idle ghost"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// shardx_status reports the three gates the UI surfaces: launcher TCP
    /// probe, vendored MCP tree, keychain token — all false in a temp harness.
    #[test]
    fn shardx_status_reports_empty_chain() {
        let app = tauri::test::mock_app();
        let handle = app.app_handle().clone();
        let home = tmp_home("shardx-status");
        handle.manage(AppState::new(home.clone()).unwrap());
        let v = shardx_status_inner(&handle);
        assert_eq!(v["mcpInstalled"], false);
        assert_eq!(v["api"], SHARDX_API_DEFAULT);
        assert!(v["mcpDir"].as_str().unwrap().contains("shardx"));
        // launcherRunning/tokenSet depend on the host — assert they exist.
        assert!(v["launcherRunning"].is_boolean());
        assert!(v["tokenSet"].is_boolean());
        let _ = std::fs::remove_dir_all(&home);
    }

    /// shardx_set_token rejects empties; a stored token flips status.tokenSet.
    /// Touches the real keychain — cleans up after itself.
    #[test]
    fn shardx_token_roundtrip_through_keychain() {
        let app = tauri::test::mock_app();
        let handle = app.app_handle().clone();
        let home = tmp_home("shardx-tok");
        assert!(shardx_set_token_inner("  ", &home).is_err());
        // Store → status sees it → delete. With the DPAPI fallback the write
        // must succeed even on hosts where CredMan drops credentials — that
        // is exactly this VM's behavior, so this is now a hard gate.
        shardx_set_token_inner("tok-test-xyz", &home).unwrap();
        handle.manage(AppState::new(home.clone()).unwrap());
        let st = shardx_status_inner(&handle);
        assert_eq!(st["tokenSet"], true, "stored token must read back");
        crate::misc::secret_delete_persist(SHARDX_TOKEN_KEY, &home);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The vendored MCP dir lands under <hive>/mcp-servers/shardx — the same
    /// path the catalog's `<harness>` placeholder resolves to at spawn.
    #[test]
    fn shardx_mcp_dir_matches_catalog_placeholder() {
        let app = tauri::test::mock_app();
        let handle = app.app_handle().clone();
        let home = tmp_home("shardx-path");
        handle.manage(AppState::new(home.clone()).unwrap());
        let st = state(&handle);
        let expected = st.hive.root.join("mcp-servers").join("shardx");
        // Simulate a vendored copy; status must detect it.
        std::fs::create_dir_all(expected.join("node_modules")).unwrap();
        std::fs::write(expected.join("index.js"), "// stub").unwrap();
        let v = shardx_status_inner(&handle);
        assert_eq!(v["mcpInstalled"], true);
        // And the spawn merge resolves <harness> to exactly this parent.
        let m = crate::mcp::build_default_mcp_servers(
            "C:/w",
            &st.hive.root,
            &std::collections::HashMap::new(),
            &["shardx".to_string()],
            &[],
            &|_| None,
        );
        assert_eq!(
            std::path::Path::new(&m["agent-office-shardx"].args[0]),
            expected.join("index.js")
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The auto-token path must mint exactly the JWT shape the launcher's
    /// auth middleware verifies: three base64url segments, sub "shardx-api",
    /// exp>iat, and signature = HS256(secret, "header.payload").
    #[test]
    fn shardx_mint_token_is_a_valid_hs256_jwt() {
        use base64::Engine;
        let tok = shardx_mint_token("s3cr3t", 3600).unwrap();
        let parts: Vec<&str> = tok.split('.').collect();
        assert_eq!(parts.len(), 3, "a JWT is header.payload.signature");
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[1])
            .unwrap();
        let v: Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(v["sub"], "shardx-api");
        assert!(v["exp"].as_u64().unwrap() > v["iat"].as_u64().unwrap());
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let mut mac = Hmac::<Sha256>::new_from_slice(b"s3cr3t").unwrap();
        mac.update(format!("{}.{}", parts[0], parts[1]).as_bytes());
        let expect = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(mac.finalize().into_bytes());
        assert_eq!(parts[2], expect, "signature must verify against the secret");
    }

    /// settings.json → token: the launcher's persisted api_secret mints a
    /// Bearer token without any user copy-paste. Empty secret = not yet
    /// initialized = clean error.
    #[test]
    fn shardx_token_from_settings_file() {
        let dir = std::env::temp_dir().join(format!("sx-set-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("settings.json");
        std::fs::write(&f, r#"{"api_secret":"abc123","api_port":40325}"#).unwrap();
        let tok = shardx_token_from_settings(&f).unwrap();
        assert_eq!(tok.split('.').count(), 3);
        std::fs::write(&f, r#"{"api_secret":""}"#).unwrap();
        assert!(shardx_token_from_settings(&f).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Profile names are per-agent and deterministic — the slug can't collide
    /// two agents into one browser identity.
    #[test]
    fn shardx_profile_name_slugifies_per_agent() {
        assert_eq!(shardx_profile_name("toko bot"), "office-toko-bot");
        assert_eq!(shardx_profile_name("a/b.c"), "office-a-b-c");
        assert_ne!(shardx_profile_name("a"), shardx_profile_name("b"));
    }

    /// The workspace note names the agent's profile, carries the never-share
    /// rule, the human_*-over-browser_* strategy, the guardrails, and a real
    /// path to the vendored README — with and without a resolved profile id.
    #[test]
    fn shardx_workspace_note_contract() {
        let root = std::path::Path::new("C:/hive");
        let n = shardx_workspace_note("office-toko-a", Some("pid-1"), root);
        assert!(n.contains("office-toko-a"));
        assert!(n.contains("pid-1"));
        assert!(n.contains("NEVER"));
        // The non-negotiables an agent can't afford to learn by banning an
        // account: human emulation preference, cookie/OTP guardrails, and a
        // resolvable pointer to the vendored 120-tool reference.
        assert!(n.contains("human_"));
        assert!(n.contains("export_cookies"));
        assert!(n.contains("OTP"));
        assert!(n.contains("mcp-servers"));
        // And it can recover a dead launcher itself — exe locations + detached
        // start are IN the note, "ask the human" is the last resort.
        assert!(n.contains("Downloads"));
        assert!(n.contains("LOCALAPPDATA"));
        assert!(n.contains("40325"));
        // The reading/capture toolbox the bridge exposes — the note is what
        // teaches agents aria-first page reads and the capture/pdf helpers.
        assert!(n.contains("browser_aria_snapshot"));
        assert!(n.contains("browser_pdf"));
        assert!(n.contains("browser_capture_start"));
        assert!(n.contains("browser_block_resources"));
        assert!(n.contains("browser_mock"));
        let pending = shardx_workspace_note("office-toko-a", None, root);
        assert!(pending.contains("create_profile"));
    }

    /// needsShardx gates: vendored bridge file + stored token — the launcher
    /// itself is deliberately NOT a gate (the agent self-starts it).
    #[test]
    fn shardx_setup_ready_requires_bridge_and_token() {
        let home = tmp_home("shardx-ready");
        let hive = home.join("hive");
        // Neither piece → not ready.
        assert!(!shardx_setup_ready(&hive, &home));
        // Bridge only → still not ready (no token).
        std::fs::create_dir_all(hive.join("mcp-servers").join("shardx")).unwrap();
        std::fs::write(
            hive.join("mcp-servers").join("shardx").join("index.js"),
            "// bridge",
        )
        .unwrap();
        assert!(!shardx_setup_ready(&hive, &home));
        // Token lands (keychain or DPAPI fallback) → ready.
        crate::misc::secret_set_persist("SHARDX_TOKEN", "tok", &home).unwrap();
        assert!(shardx_setup_ready(&hive, &home));
        std::fs::remove_dir_all(&home).ok();
    }

    /// Persist-tier write: keychain OR the DPAPI file — a secret must read
    /// back through secret_get_persist regardless of which store landed it.
    /// On this VM the keychain drops entries between Entry objects, so the
    /// file fallback is the store actually exercised here.
    #[test]
    fn secret_persist_roundtrips_despite_flaky_keychain() {
        let home = tmp_home("secrets-persist");
        let key = "DEVIN_TEST_PERSIST";
        let store = crate::misc::secret_set_persist(key, "v1", &home).unwrap();
        assert!(matches!(store, "keychain" | "dpapi-file"));
        assert_eq!(
            crate::misc::secret_get_persist(key, &home).as_deref(),
            Some("v1"),
            "value must read back from whichever store landed it"
        );
        crate::misc::secret_delete_persist(key, &home);
        assert_eq!(crate::misc::secret_get_persist(key, &home), None);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The exact failure the user hit: a fresh harness with no provider base
    /// URL and zero stored keys resolves an EMPTY seed — hermes boots its
    /// openrouter default authless and dies on 401 (observed live).
    #[test]
    fn hermes_seed_empty_everything_is_authless() {
        let cfg = crate::config::Config::default();
        let seed = hermes_seed_plan(&cfg, &|_| None);
        assert!(seed.provider.is_none());
        assert!(seed.api_key.is_none());
        assert!(seed.base_url.is_none());
        assert!(hermes_needs_key(&seed, &std::collections::HashMap::new()));
    }

    /// The configured-and-happy path: deepseek base URL + key → first-class
    /// deepseek seed with credential, no needsKey flag.
    #[test]
    fn hermes_seed_configured_deepseek_resolves() {
        let mut cfg = crate::config::Config::default();
        cfg.provider_base_urls
            .insert("hermes".into(), "https://api.deepseek.com".into());
        cfg.provider_default_models
            .insert("hermes".into(), "deepseek-chat".into());
        let seed = hermes_seed_plan(&cfg, &|k| {
            (k == "PROVIDER_KEY_DEEPSEEK").then(|| "sk-real".to_string())
        });
        assert_eq!(seed.provider.as_deref(), Some("deepseek"));
        assert_eq!(seed.api_key.as_deref(), Some("sk-real"));
        assert_eq!(seed.model.as_deref(), Some("deepseek-chat"));
        assert!(!hermes_needs_key(&seed, &std::collections::HashMap::new()));
    }

    /// URL configured but the keychain lost its entry (the VM CredMan drop)
    /// still resolves a provider — but authless. needsKey must flag it: the
    /// spawned hermes 401s at api.deepseek.com instead of openrouter, same
    /// dead agent either way.
    #[test]
    fn hermes_seed_url_without_key_is_authless() {
        let mut cfg = crate::config::Config::default();
        cfg.provider_base_urls
            .insert("hermes".into(), "https://api.deepseek.com".into());
        let seed = hermes_seed_plan(&cfg, &|_| None);
        assert_eq!(seed.provider.as_deref(), Some("deepseek"));
        assert!(seed.api_key.is_none());
        assert!(hermes_needs_key(&seed, &std::collections::HashMap::new()));
    }

    /// Fallback ladder: no configured URL + only an openrouter key → the seed
    /// picks openrouter with its credential; same for a lone deepseek key
    /// which also brings its well-known endpoint.
    #[test]
    fn hermes_seed_fallback_picks_any_stored_key() {
        let cfg = crate::config::Config::default();
        let openrouter_only = |k: &str| {
            (k == "PROVIDER_KEY_OPENROUTER").then(|| "sk-or".to_string())
        };
        let seed = hermes_seed_plan(&cfg, &openrouter_only);
        assert_eq!(seed.provider.as_deref(), Some("openrouter"));
        assert_eq!(seed.api_key.as_deref(), Some("sk-or"));

        let deepseek_only = |k: &str| {
            (k == "PROVIDER_KEY_DEEPSEEK").then(|| "sk-ds".to_string())
        };
        let seed = hermes_seed_plan(&cfg, &deepseek_only);
        assert_eq!(seed.provider.as_deref(), Some("deepseek"));
        assert_eq!(seed.base_url.as_deref(), Some("https://api.deepseek.com"));
        assert!(!hermes_needs_key(&seed, &std::collections::HashMap::new()));
    }

    /// A verbatim-named custom secret ("OPENROUTER_API_KEY", no PROVIDER_KEY_
    /// prefix) injects as an env var straight into the CLI — auth works even
    /// though the seed has no api_key, so needsKey must NOT flag it.
    #[test]
    fn hermes_needs_key_respects_env_injected_credential() {
        let cfg = crate::config::Config::default();
        let seed = hermes_seed_plan(&cfg, &|_| None);
        let env = std::collections::HashMap::from([
            ("OPENROUTER_API_KEY".to_string(), "sk-or".to_string()),
        ]);
        assert!(!hermes_needs_key(&seed, &env));
    }

    /// The owner map labels both naming paths: `office-<id>` by convention and
    /// a user-bound account profile via extra.shardxProfile.
    #[test]
    fn shardx_owner_map_labels_auto_and_bound() {
        let auto = crate::hive::RegistryEntry {
            id: "toko".into(),
            name: "Toko".into(),
            ..Default::default()
        };
        let mut bound = crate::hive::RegistryEntry {
            id: "cs".into(),
            name: "CS".into(),
            ..Default::default()
        };
        bound
            .extra
            .insert("shardxProfile".into(), serde_json::json!({"name": "shopee-a"}));
        let m = shardx_owner_map(&[auto, bound]);
        assert_eq!(m.get("office-toko").map(String::as_str), Some("Toko"));
        assert_eq!(m.get("shopee-a").map(String::as_str), Some("CS"));
    }

    /// Cleanup deletes ONLY this hive's own auto-profiles that nothing binds:
    /// bound names survive, other hives' office-* survive (the tag check),
    /// user-named profiles are never candidates regardless.
    #[test]
    fn shardx_cleanup_plan_only_drops_own_orphans() {
        let tag = hive_tag(std::path::Path::new("C:/hive/a"));
        let other = hive_tag(std::path::Path::new("C:/hive/b"));
        assert_ne!(tag, other, "different roots must tag differently");
        let profiles = vec![
            serde_json::json!({"id":"p1","name":"office-alive","notes":format!("office agent alive | hive:{tag}")}),
            serde_json::json!({"id":"p2","name":"office-gone","notes":format!("office agent gone | hive:{tag}")}),
            serde_json::json!({"id":"p3","name":"office-foreign","notes":format!("office agent x | hive:{other}")}),
            serde_json::json!({"id":"p4","name":"shopee-toko-a","notes":"user's account"}),
            serde_json::json!({"id":"p5","name":"office-notag","notes":"legacy"}),
        ];
        let bound = std::collections::HashSet::from([
            "office-alive".to_string(),
            "shopee-toko-a".to_string(),
        ]);
        let doomed = shardx_cleanup_plan(&profiles, &bound, &tag);
        assert_eq!(doomed, vec!["p2".to_string()], "only own-hive orphans die");
    }

    /// spawn args carry the optional profile binding through the wire struct.
    #[test]
    fn shardx_profile_field_translates() {
        let mut r = req(serde_json::json!({
            "id": "pty-x", "cwd": "C:/w", "command": "claude",
            "hive": { "id": "x" },
            "shardxProfile": "shopee-toko-a"
        }));
        let (args, _) = translate_spawn_request(&r).unwrap();
        assert_eq!(args.shardx_profile.as_deref(), Some("shopee-toko-a"));
        // Blank/whitespace collapses to None → auto office-<id>.
        r.shardx_profile = Some("   ".into());
        let (args, _) = translate_spawn_request(&r).unwrap();
        assert!(args.shardx_profile.is_none());
    }

    /// Bootstrap reports each gate independently: bridge pre-seeded → ok,
    /// launcher down + try_launch=false → not ok, token without a launcher
    /// settings file → not ok. Whole result is the AND of the three.
    #[test]
    fn shardx_bootstrap_reports_each_gate() {
        let home = tmp_home("shardx-boot");
        let dest = home.join("mcp-servers").join("shardx");
        std::fs::create_dir_all(dest.join("node_modules")).unwrap();
        std::fs::write(dest.join("index.js"), "// stub").unwrap();
        // Dead port + no settings file = deterministic "nothing live" —
        // independent of whether a real launcher is running on this box.
        let v = shardx_bootstrap_impl(&home, false, 1, None, || {
            Ok(serde_json::json!({ "ok": true, "stubbed": true }))
        });
        assert_eq!(v["steps"]["bridge"]["ok"], true);
        assert_eq!(v["steps"]["launcher"]["ok"], false);
        assert_eq!(v["steps"]["token"]["ok"], false);
        // Launcher down → fingerprint seeding never ran (it's a live-API step).
        assert!(v["steps"]["fingerprints"].is_null());
        assert_eq!(v["ok"], false);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// ── Local stub launcher API ─────────────────────────────────────────
    /// Serves canned bodies per path on 127.0.0.1:<ephemeral> and records
    /// (method, path, authorization, body) of every request — enough to drive
    /// the real ureq helpers end-to-end without a launcher or the CDN.
    struct FakeShardX {
        base: String,
        hits: std::sync::Arc<std::sync::Mutex<Vec<(String, String, String, String)>>>,
    }

    fn http_request_complete(buf: &[u8]) -> bool {
        let Some(pos) = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
        else {
            return false;
        };
        let head = String::from_utf8_lossy(&buf[..pos]);
        let len: usize = head
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.trim().eq_ignore_ascii_case("content-length")
                    .then(|| v.trim().parse().ok())
                    .flatten()
            })
            .unwrap_or(0);
        buf.len() >= pos + 4 + len
    }

    fn fake_shardx_api(routes: Vec<(&'static str, Vec<u8>)>) -> FakeShardX {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let base_url = format!("http://127.0.0.1:{port}");
        let base_for_thread = base_url.clone();
        let hits = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let h = hits.clone();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { break };
                s.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok();
                let mut buf = Vec::new();
                let mut tmp = [0u8; 8192];
                loop {
                    match s.read(&mut tmp) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            if http_request_complete(&buf) {
                                break;
                            }
                        }
                    }
                }
                let text = String::from_utf8_lossy(&buf).to_string();
                let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
                let mut req = head.lines();
                let mut parts = req.next().unwrap_or("").split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let path = parts.next().unwrap_or("").to_string();
                let auth = head
                    .lines()
                    .find(|l| l.to_lowercase().starts_with("authorization"))
                    .unwrap_or("")
                    .to_string();
                h.lock()
                    .unwrap()
                    .push((method, path.clone(), auth, body.to_string()));
                // {BASE} in a served body self-references the stub's own
                // loopback URL — lets fixtures embed absolute URLs (release
                // asset browser_download_url) without knowing the port.
                let payload = routes
                    .iter()
                    .find(|(p, _)| p == &path)
                    .map(|(_, b)| {
                        // Only text bodies get the {BASE} substitution —
                        // binary fixtures (zips, exes) must pass through
                        // byte-exact.
                        if b.windows(6).any(|w| w == b"{BASE}") {
                            String::from_utf8_lossy(b)
                                .replace("{BASE}", &base_for_thread)
                                .into_bytes()
                        } else {
                            b.clone()
                        }
                    })
                    .unwrap_or_else(|| b"{}".to_vec());
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                let _ = s.write_all(resp.as_bytes());
                let _ = s.write_all(&payload);
            }
        });
        FakeShardX {
            base: format!("http://127.0.0.1:{port}"),
            hits,
        }
    }

    /// The fixture bundle: shardx-fingerprints/{fp-a,fp-b}.json + a README
    /// the copier must skip — built once by hand with Compress-Archive.
    const FP_FIXTURE: &[u8] = include_bytes!("fixtures/shardx-fp-test.zip");

    /// Status counts read real array lengths; an unrouted path returns a
    /// 404-shaped body → None, not a panic.
    #[test]
    fn shardx_counts_read_list_lengths() {
        let srv = fake_shardx_api(vec![
            ("/fingerprints", br#"[{"a":1},{"a":2},{"a":3}]"#.to_vec()),
            ("/profiles", b"[{}]".to_vec()),
            ("/running", b"[]".to_vec()),
        ]);
        let http = shardx_http(5);
        assert_eq!(
            shardx_count_path(&http, &srv.base, "Bearer t", "/fingerprints"),
            Some(3)
        );
        assert_eq!(
            shardx_count_path(&http, &srv.base, "Bearer t", "/profiles"),
            Some(1)
        );
        assert_eq!(
            shardx_count_path(&http, &srv.base, "Bearer t", "/running"),
            Some(0)
        );
        // Missing route → stub 200s {} (not an array) → None.
        assert_eq!(
            shardx_count_path(&http, &srv.base, "Bearer t", "/nope"),
            None
        );
        // The Bearer header actually went out on the wire.
        let hits = srv.hits.lock().unwrap();
        assert!(hits.iter().all(|(_, _, auth, _)| auth.contains("Bearer t")));
    }

    /// A library that already has entries is reported, never re-downloaded.
    #[test]
    fn shardx_seed_reports_already_without_downloading() {
        let srv = fake_shardx_api(vec![
            ("/fingerprints", b"[{},{}]".to_vec()),
            ("/ShardX-Fingerprints.zip", FP_FIXTURE.to_vec()),
        ]);
        let http = shardx_http(5);
        let lib = tmp_home("shardx-fp-already");
        let v = shardx_seed_fingerprints_at(&http, &srv.base, "Bearer t", "unused://url", &lib)
            .unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["already"], 2);
        // One request: the emptiness probe. The zip route was never touched.
        let hits = srv.hits.lock().unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, "/fingerprints");
        let _ = std::fs::remove_dir_all(&lib);
    }

    /// Empty library → download → extract → jsons land in the library dir,
    /// the wrapper dir and non-json files handled correctly.
    #[test]
    fn shardx_seed_populates_empty_library() {
        let srv = fake_shardx_api(vec![
            ("/fingerprints", b"[]".to_vec()),
            ("/fp.zip", FP_FIXTURE.to_vec()),
        ]);
        let http = shardx_http(5);
        let lib = tmp_home("shardx-fp-seed");
        let url = format!("{}/fp.zip", srv.base);
        let v = shardx_seed_fingerprints_at(&http, &srv.base, "Bearer t", &url, &lib).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["seeded"], 2);
        assert!(lib.join("fp-a.json").exists());
        assert!(lib.join("fp-b.json").exists());
        // The README.txt in the fixture was correctly ignored.
        assert!(!lib.join("README.txt").exists());
        let _ = std::fs::remove_dir_all(&lib);
    }

    /// The full launcher-download arc against a stub release API: pick the
    /// OS-correct asset, stream a >10 MiB body to disk (the exact case that
    /// used to die on ureq's 10 MiB cap), and land the file under the
    /// destination dir — fresh-download → usable.
    #[test]
    fn shardx_download_launcher_picks_os_asset_and_lands() {
        let want = if cfg!(windows) {
            "portable-win-x64.exe"
        } else if cfg!(target_os = "macos") {
            "aarch64.dmg"
        } else {
            "amd64.AppImage"
        };
        let big = vec![0x4Du8; 11 * 1024 * 1024 + 13];
        let srv = fake_shardx_api(vec![
            (
                "/releases/latest",
                serde_json::json!({
                    "assets": [
                        { "name": "ShardX-x86_64-unknown.AppImage", "browser_download_url": "{BASE}/wrong-1" },
                        { "name": format!("ShardX-{want}"), "browser_download_url": "{BASE}/dl/asset" },
                        { "name": "ShardX-aarch64.dmg", "browser_download_url": "{BASE}/wrong-2" }
                    ]
                })
                .to_string()
                .into_bytes(),
            ),
            ("/dl/asset", big.clone()),
        ]);
        let home = tmp_home("shardx-dl");
        let release_url = format!("{}/releases/latest", srv.base);
        let p = shardx_download_launcher_at(&release_url, &home).unwrap();
        let landed = PathBuf::from(&p);
        assert!(landed.starts_with(&home), "lander landed outside dest: {p}");
        assert!(landed.exists(), "launcher never landed: {p}");
        assert_eq!(
            std::fs::metadata(&landed).unwrap().len() as usize,
            big.len(),
            "streamed body truncated"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The release API's per-asset `digest` gates the download: a matching
    /// sha256 lands the file; a mismatch deletes it and errors.
    #[test]
    fn shardx_download_launcher_verifies_release_digest() {
        use sha2::{Digest, Sha256};
        let want = if cfg!(windows) {
            "portable-win-x64.exe"
        } else if cfg!(target_os = "macos") {
            "aarch64.dmg"
        } else {
            "amd64.AppImage"
        };
        let body = b"launcher-bytes".to_vec();
        let good: String = Sha256::digest(&body).iter().map(|b| format!("{b:02x}")).collect();
        let rel = |digest: String| {
            serde_json::json!({ "assets": [{
                "name": format!("ShardX-{want}"),
                "browser_download_url": "{BASE}/dl/asset",
                "digest": digest
            }]})
            .to_string()
            .into_bytes()
        };
        let srv = fake_shardx_api(vec![
            ("/ok", rel(format!("sha256:{good}"))),
            ("/bad", rel(format!("sha256:{}", "0".repeat(64)))),
            ("/dl/asset", body.clone()),
        ]);
        let home = tmp_home("shardx-dl-digest");
        let p = shardx_download_launcher_at(&format!("{}/ok", srv.base), &home).unwrap();
        assert!(PathBuf::from(&p).exists());
        let _ = std::fs::remove_file(&p);
        let err = shardx_download_launcher_at(&format!("{}/bad", srv.base), &home).unwrap_err();
        assert!(err.contains("sha256 mismatch"), "{err}");
        assert!(!PathBuf::from(&p).exists(), "mismatched file must be deleted");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn verify_sha256_digest_helper() {
        let dir = tmp_home("sha-helper");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.bin");
        std::fs::write(&f, b"abc").unwrap();
        // sha256("abc")
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(verify_sha256_digest(&f, &format!("sha256:{abc}")).is_ok());
        assert!(verify_sha256_digest(&f, &format!("sha256:{}", abc.to_uppercase())).is_ok());
        // Unknown algorithm / no prefix → not enforced, file kept.
        assert!(verify_sha256_digest(&f, "sha512:deadbeef").is_ok());
        assert!(f.exists());
        assert!(verify_sha256_digest(&f, "sha256:00").is_err());
        assert!(!f.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// LIVE — the real GitHub release → real launcher asset → disk. ~100MB
    /// download; run manually:
    ///   cargo test shardx_download_launcher_live -- --ignored --nocapture
    #[test]
    #[ignore = "live: real GitHub release download (~100MB)"]
    fn shardx_download_launcher_live() {
        let home = tmp_home("shardx-dl-live");
        let p = shardx_download_launcher_at(
            "https://api.github.com/repos/ProxyShard/ShardBrowser/releases/latest",
            &home,
        )
        .unwrap();
        let landed = PathBuf::from(&p);
        let meta = std::fs::metadata(&landed).unwrap();
        eprintln!("[live] landed {p} ({} bytes)", meta.len());
        assert!(meta.len() > 10 * 1024 * 1024, "suspiciously small asset");
        // A real portable exe starts with MZ; an HTML error page doesn't.
        if cfg!(windows) {
            let mut magic = [0u8; 2];
            use std::io::Read as _;
            std::fs::File::open(&landed)
                .unwrap()
                .read_exact(&mut magic)
                .unwrap();
            assert_eq!(&magic, b"MZ", "downloaded file is not a PE exe");
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    /// ureq's buffered readers cap at 10 MiB by default — the launcher exe
    /// and fingerprint bundle both exceed it ("the response body is larger
    /// than request limit: 10485760"). download_to_file streams instead, so
    /// a >10 MiB body must land on disk intact.
    #[test]
    fn download_to_file_streams_past_ureq_10mb_cap() {
        let big = vec![0xABu8; 11 * 1024 * 1024 + 7];
        let srv = fake_shardx_api(vec![("/big.bin", big.clone())]);
        let http = shardx_http(30);
        let mut resp = http
            .get(&format!("{}/big.bin", srv.base))
            .call()
            .unwrap();
        let tmp = std::env::temp_dir().join(format!("dl-{}", uuid::Uuid::new_v4()));
        let n = download_to_file(&mut resp, &tmp).unwrap();
        assert_eq!(n as usize, big.len());
        assert_eq!(std::fs::metadata(&tmp).unwrap().len() as usize, big.len());
        assert_eq!(&std::fs::read(&tmp).unwrap()[..4], &[0xAB; 4]);
        // Landed via <dest>.part + rename — no scratch file left behind.
        assert!(!part_path(&tmp).exists());
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn part_path_appends_suffix_and_tar_resolves() {
        assert_eq!(part_path(Path::new("a/b.zip")), PathBuf::from("a/b.zip.part"));
        let t = system_tar();
        #[cfg(windows)]
        assert!(
            t.ends_with("System32/tar.exe") || t == PathBuf::from("tar"),
            "{t:?}"
        );
        #[cfg(not(windows))]
        assert_eq!(t, PathBuf::from("tar"));
    }

    /// A DNS-hijack block page arrives with HTTP 200 but HTML bytes — the
    /// PK magic check must reject it instead of writing garbage into the
    /// library. This is the exact failure the launcher's own seed hit.
    #[test]
    fn shardx_seed_rejects_html_block_page() {
        let srv = fake_shardx_api(vec![
            ("/fingerprints", b"[]".to_vec()),
            ("/fp.zip", b"<html><title>Internet Positif</title></html>".to_vec()),
        ]);
        let http = shardx_http(5);
        let lib = tmp_home("shardx-fp-block");
        let url = format!("{}/fp.zip", srv.base);
        let e = shardx_seed_fingerprints_at(&http, &srv.base, "Bearer t", &url, &lib)
            .unwrap_err();
        assert!(e.contains("not a zip"), "got: {e}");
        // Nothing landed in the library.
        assert!(!lib.join("fp-a.json").exists());
        let _ = std::fs::remove_dir_all(&lib);
    }

    /// The proxy bind goes out as PATCH /profiles/{id} with {"proxy_id":..},
    /// and "" unbinds — the wire contract the launcher documents.
    #[test]
    fn shardx_proxy_bind_wire_shape() {
        let srv = fake_shardx_api(vec![("/profiles/p1", b"{}".to_vec())]);
        let http = shardx_http(5);
        shardx_patch_profile_proxy(&http, &srv.base, "Bearer t", "p1", "px-9").unwrap();
        shardx_patch_profile_proxy(&http, &srv.base, "Bearer t", "p1", "").unwrap();
        let hits = srv.hits.lock().unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, "PATCH");
        assert_eq!(hits[0].1, "/profiles/p1");
        assert_eq!(
            serde_json::from_str::<Value>(&hits[0].3).unwrap()["proxy_id"],
            "px-9"
        );
        assert_eq!(serde_json::from_str::<Value>(&hits[1].3).unwrap()["proxy_id"], "");
    }

    /// kind:'shardx' missions POST /automation/projects/{id}/run — empty
    /// body, Bearer auth. The scheduler relies on this exact shape: a wrong
    /// verb or path turns a scheduled run into a silent 404.
    #[test]
    fn shardx_automation_call_wire_shape() {
        let srv = fake_shardx_api(vec![
            ("/automation/projects/proj-9/run", b"{}".to_vec()),
            ("/automation/projects/proj-9/stop", b"{}".to_vec()),
        ]);
        let http = shardx_http(5);
        shardx_automation_call_at(&http, &srv.base, "Bearer t", "proj-9", "run").unwrap();
        shardx_automation_call_at(&http, &srv.base, "Bearer t", "proj-9", "stop").unwrap();
        let hits = srv.hits.lock().unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, "POST");
        assert_eq!(hits[0].1, "/automation/projects/proj-9/run");
        assert_eq!(hits[1].1, "/automation/projects/proj-9/stop");
        assert!(hits.iter().all(|(_, _, auth, _)| auth.contains("Bearer t")));
    }

    /// An idle orchestrator must get the closing-time nudge — workerwake
    /// deliberately never wakes god (explain() → "god"), and the mail+steer
    /// only land at the next hook boundary, so without this poke an idle god
    /// sits on the shutdown brief until the 6-minute timeout. The fix types
    /// the wake text into god's PTY when the registry marks it idle — cmd
    /// echoes input back, so the tail buffer catches the proof.
    #[test]
    fn closing_time_nudges_idle_god() {
        let app = tauri::test::mock_app();
        let handle = app.app_handle().clone();
        let home = tmp_home("closing-god");
        handle.manage(AppState::new(home.clone()).unwrap());
        // cmd stays alive and accepts writes — enough to prove the nudge
        // path reaches the wire (conpty echo isn't reliable enough to assert
        // on).
        let res = spawn_agent_core(
            &handle,
            req(serde_json::json!({
                "id": "pty-god", "cwd": home.display().to_string(),
                "command": "cmd",
                "hive": { "id": "god", "name": "God", "role": "orchestrator", "isGod": true }
            })),
        );
        assert!(res.ok, "spawn failed: {:?}", res.error);
        let st = state(&handle);
        st.hive.set_status("god", "idle").unwrap();
        // The full entry path: protocol starts (0 workers) without deadlocking
        // on the control→pty lock order.
        assert_eq!(closing_time_start_core(&handle).unwrap(), 0);
        // Idle god + live PTY → the write is attempted and succeeds.
        assert!(nudge_idle_god(&st));
        // Busy god → never type into a working terminal. The PTY-quiet leg
        // keys off last_output_at (seeded at spawn, so a loaded machine that
        // stalls this test for 12s+ would flip it idle) — pin the clock
        // explicitly instead of racing real time.
        let clock = {
            let ptys = st.ptys.lock().unwrap();
            ptys.get("pty-god").expect("god pty").last_output_at.clone()
        };
        let set_clock = |ago_ms: u64| {
            clock.store(
                crate::closing_time::now_ms_pub().saturating_sub(ago_ms).max(1),
                std::sync::atomic::Ordering::Relaxed,
            );
        };
        st.hive.set_status("god", "busy").unwrap();
        // (Only ever moved FORWARD here: the first nudge's echo also bumps
        // the clock from the reader thread, so a "far in the past" value
        // could be raced away — the negative assertion can't be.)
        set_clock(0);
        assert!(!nudge_idle_god(&st));
        // Idle again but PTY gone → nothing to write to.
        st.hive.set_status("god", "idle").unwrap();
        let pty = { st.ptys.lock().unwrap().remove("pty-god") };
        st.unmap_pty("pty-god");
        assert!(!nudge_idle_god(&st));
        if let Some(mut h) = pty {
            let _ = h.kill();
        }
    }

    /// The scheduler's shardx dispatch on a state with no usable token —
    /// the outcome string must NAME the project and say it failed, never
    /// panic or pretend a run started. Works whether or not a real launcher
    /// happens to be listening (fresh harness dir carries no token).
    #[test]
    fn shardx_fire_reports_failure_without_token() {
        let st = AppState::new(tmp_home("shardx-fire")).unwrap();
        let out = shardx_fire_automation(&st, "proj-nope");
        assert!(out.contains("proj-nope"), "got: {out}");
        assert!(out.contains("failed"), "got: {out}");
    }

    /// The real download chain — the same tarball + npm install the consent
    /// button runs. Network + npm required; run manually:
    ///   cargo test shardx_bootstrap_live_download -- --ignored
    #[test]
    #[ignore = "live network: real GitHub tarball + npm install"]
    fn shardx_bootstrap_live_download() {
        let home = tmp_home("shardx-live");
        let v = shardx_bootstrap_inner(&home, false);
        assert_eq!(
            v["steps"]["bridge"]["ok"], true,
            "bridge vendor failed: {v}"
        );
        assert!(home
            .join("mcp-servers")
            .join("shardx")
            .join("index.js")
            .exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn pty_op_results_match_upstream_envelope() {
        // submitToPty reads `wrote.ok`; a dead pty resolves {ok:false,
        // error:'no pty: <id>'} rather than rejecting.
        let app = tauri::test::mock_app();
        let handle = app.app_handle().clone();
        let home = tmp_home("ptyop");
        handle.manage(AppState::new(home.clone()).unwrap());
        let w = pty_write_core(&handle, "pty-nada", "x");
        assert!(!w.ok);
        assert_eq!(w.error.as_deref(), Some("no pty: pty-nada"));
        let r = pty_resize_core(&handle, "pty-nada", 80, 24);
        assert!(!r.ok);
        assert_eq!(r.error.as_deref(), Some("no pty: pty-nada"));
        let _ = std::fs::remove_dir_all(&home);
    }

    /// LIVE TEST — the whole missing-CLI ladder against the real world:
    /// `hermes` uninstalled → spawn_agent_core runs the installer PTY →
    /// exit-0 relaunch → `hermes chat` booting in the same slot. Downloads +
    /// installs the real Nous Research CLI (minutes); run manually:
    ///   cargo test --lib hermes_install_ladder_live -- --ignored --nocapture
    #[test]
    #[ignore = "live: downloads and installs the real hermes CLI"]
    fn hermes_install_ladder_live() {
        // The test only means something with the binary actually absent —
        // but PATH-based checks must not re-hit after install, so also probe
        // the venv fallback (a fresh resolve AFTER install is the success
        // signal, so the guard runs on PATH alone).
        let path_env = std::env::var("PATH").unwrap_or_default();
        assert!(
            crate::pty::resolve_program("hermes", &path_env).is_none(),
            "uninstall hermes first — this test exercises the missing-CLI ladder"
        );

        let app = tauri::test::mock_app();
        let handle = app.app_handle().clone();
        let home = tmp_home("hermes-live");
        handle.manage(AppState::new(home.clone()).unwrap());
        let res = spawn_agent_core(
            &handle,
            req(serde_json::json!({
                "id": "pty-hermes", "provider": "hermes",
                "command": "hermes chat --provider deepseek -m deepseek-v4-pro",
                "cwd": home.display().to_string(),
                "hive": { "id": "hermes-live", "name": "Hermes", "role": "worker" }
            })),
        );
        assert!(res.ok, "spawn failed: {:?}", res.error);
        let st = state(&handle);
        // The install rail spawned a PTY in the slot AND armed the relaunch —
        // the two halves of "installs on first run".
        assert!(st.ptys.lock().unwrap().contains_key("pty-hermes"));
        assert!(
            PENDING_INSTALLS.lock().unwrap().contains_key("pty-hermes"),
            "install PTY must be armed for relaunch"
        );
        eprintln!("[live] install PTY spawned + relaunch armed — waiting for the installer");
        // PowerShell + prompt_toolkit under conpty emit CSI 6 n and BLOCK on
        // the reply — the real app's xterm answers it; the test has no
        // terminal. Answer only when the tail ENDS with a fresh query (the
        // child is parked on that read) — replying on a count basis keeps
        // writing into a pipe nobody drains and can stall the test.
        let answer_dsr = |st: &tauri::State<'_, AppState>| {
            let mut ptys = st.ptys.lock().unwrap();
            if let Some(h) = ptys.get_mut("pty-hermes") {
                if h.tail().trim_end().ends_with("\u{1b}[6n") {
                    let _ = h.write("\u{1b}[24;80R");
                }
            }
        };
        // The installer downloads Python+deps and builds the TUI — allow 40
        // minutes (measured ~24 on this machine). Success = relaunch
        // consumed the pending entry AND `hermes` resolves now.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(40 * 60);
        loop {
            answer_dsr(&st);
            let armed = PENDING_INSTALLS.lock().unwrap().contains_key("pty-hermes");
            let resolved = crate::pty::resolve_program("hermes", &path_env).is_some();
            if !armed && resolved {
                break;
            }
            if std::time::Instant::now() > deadline {
                let tail = st
                    .ptys
                    .lock()
                    .unwrap()
                    .get("pty-hermes")
                    .map(|h| h.tail())
                    .unwrap_or_default();
                panic!("install never finished (armed={armed} resolved={resolved}); tail: {tail:?}");
            }
            std::thread::sleep(std::time::Duration::from_secs(10));
        }
        // Relaunch landed: the slot now hosts the real REPL, not the
        // installer — keep answering DSR (prompt_toolkit queries the cursor
        // position on boot and blocks on the reply).
        for _ in 0..120 {
            answer_dsr(&st);
            let done = st
                .ptys
                .lock()
                .unwrap()
                .get("pty-hermes")
                .map(|h| h.command.contains("hermes"))
                .unwrap_or(false);
            if done {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        let cmd = st
            .ptys
            .lock()
            .unwrap()
            .get("pty-hermes")
            .map(|h| h.command.clone())
            .unwrap_or_default();
        assert!(cmd.contains("hermes"), "relaunch should run hermes chat: {cmd:?}");
        eprintln!("[live] hermes installed + relaunched: {cmd}");

        // Keep answering DSR for a short window so the REPL can finish its
        // boot probes, then let the TUI be — the API round-trip below is the
        // assertion; the interactive REPL's health shows in the tail.
        let repl_window = std::time::Instant::now() + std::time::Duration::from_secs(90);
        while std::time::Instant::now() < repl_window {
            answer_dsr(&st);
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
        {
            let tail = st
                .ptys
                .lock()
                .unwrap()
                .get("pty-hermes")
                .map(|h| h.tail())
                .unwrap_or_default();
            eprintln!("[live] repl tail: {}", &tail[tail.len().saturating_sub(400)..]);
        }

        // API round-trip against DeepSeek V4 Pro — `hermes chat -q --oneshot`
        // prints the reply and exits; no TUI readline in the way. The key
        // comes from the harness .env the install just laid down (the same
        // .env the per-agent bridge copies — DEEPSEEK_API_KEY is set there).
        let pty = { st.ptys.lock().unwrap().remove("pty-hermes") };
        if let Some(mut h) = pty {
            let _ = h.kill();
        }
        let out = std::process::Command::new("hermes")
            .args([
                "chat",
                "-q",
                "Answer with the single lowercase word for a tropical yellow fruit with spiky skin.",
                "--oneshot",
                "--provider",
                "deepseek",
                "-m",
                "deepseek-v4-pro",
            ])
            .current_dir(&home)
            .output()
            .expect("run hermes oneshot");
        let body = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        eprintln!("[live] oneshot exit={:?} out: {}", out.status.code(), body);
        assert!(out.status.success(), "hermes oneshot failed: {body}");
        assert!(
            body.to_lowercase().contains("pineapple"),
            "no DeepSeek reply token in output: {body}"
        );
    }

    /// LIVE TEST — openclaw's ladder, same rails as hermes': missing binary
    /// → install PTY (`install.ps1 -NoOnboard` so the onboard wizard can't
    /// park) → exit-0 relaunch → `openclaw tui --local` in the same slot.
    /// Downloads a private Node + the npm package — minutes; run manually:
    ///   cargo test --lib openclaw_install_ladder_live -- --ignored --nocapture
    #[test]
    #[ignore = "live: downloads and installs the real openclaw CLI"]
    fn openclaw_install_ladder_live() {
        // user_shell_path() on Windows re-reads the registry PATH — the same
        // live view spawn_inner resolves with, so a shim the installer JUST
        // registered (its private portable-node prefix) is visible here.
        let path_env = crate::pty::user_shell_path();
        assert!(
            crate::pty::resolve_program("openclaw", &path_env).is_none(),
            "uninstall openclaw first — this test exercises the missing-CLI ladder"
        );

        let app = tauri::test::mock_app();
        let handle = app.app_handle().clone();
        let home = tmp_home("openclaw-live");
        handle.manage(AppState::new(home.clone()).unwrap());
        let res = spawn_agent_core(
            &handle,
            req(serde_json::json!({
                "id": "pty-openclaw", "provider": "openclaw",
                "command": "openclaw tui --local",
                "cwd": home.display().to_string(),
                "hive": { "id": "openclaw-live", "name": "OpenClaw", "role": "worker" }
            })),
        );
        assert!(res.ok, "spawn failed: {:?}", res.error);
        let st = state(&handle);
        assert!(st.ptys.lock().unwrap().contains_key("pty-openclaw"));
        assert!(
            PENDING_INSTALLS.lock().unwrap().contains_key("pty-openclaw"),
            "install PTY must be armed for relaunch"
        );
        eprintln!("[live] openclaw install PTY spawned + relaunch armed");
        let answer_dsr = |st: &tauri::State<'_, AppState>| {
            let mut ptys = st.ptys.lock().unwrap();
            if let Some(h) = ptys.get_mut("pty-openclaw") {
                if h.tail().trim_end().ends_with("\u{1b}[6n") {
                    let _ = h.write("\u{1b}[24;80R");
                }
            }
        };
        // Node bootstrap + npm i -g — allow 20 minutes.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20 * 60);
        loop {
            answer_dsr(&st);
            let armed = PENDING_INSTALLS.lock().unwrap().contains_key("pty-openclaw");
            let resolved =
                crate::pty::resolve_program("openclaw", &crate::pty::user_shell_path()).is_some();
            if !armed && resolved {
                break;
            }
            if std::time::Instant::now() > deadline {
                let tail = st
                    .ptys
                    .lock()
                    .unwrap()
                    .get("pty-openclaw")
                    .map(|h| h.tail())
                    .unwrap_or_default();
                panic!("openclaw install never finished (armed={armed} resolved={resolved}); tail: {tail:?}");
            }
            std::thread::sleep(std::time::Duration::from_secs(10));
        }
        // Same-slot relaunch carries the real TUI command.
        for _ in 0..60 {
            answer_dsr(&st);
            let done = st
                .ptys
                .lock()
                .unwrap()
                .get("pty-openclaw")
                .map(|h| h.command.contains("openclaw"))
                .unwrap_or(false);
            if done {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        let cmd = st
            .ptys
            .lock()
            .unwrap()
            .get("pty-openclaw")
            .map(|h| h.command.clone())
            .unwrap_or_default();
        assert!(cmd.contains("openclaw"), "relaunch should run openclaw: {cmd:?}");
        eprintln!("[live] openclaw installed + relaunched: {cmd}");
        let pty = { st.ptys.lock().unwrap().remove("pty-openclaw") };
        if let Some(mut h) = pty {
            let _ = h.kill();
        }
        // Binary sanity: version probe outside the PTY — resolve through the
        // same live registry path the spawner uses; our own process PATH may
        // still predate the installer's PATH write.
        let exe = match crate::pty::resolve_program("openclaw", &crate::pty::user_shell_path())
        {
            Some(crate::pty::Resolved::Script(p) | crate::pty::Resolved::Exe(p)) => p,
            _ => panic!("openclaw still not resolvable post-install"),
        };
        let out = std::process::Command::new("cmd")
            .args(["/d", "/s", "/c"])
            .arg(format!(
                "{} --version",
                crate::pty::cmd_quote(&exe.display().to_string())
            ))
            .output()
            .expect("run openclaw --version");
        eprintln!(
            "[live] openclaw --version: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(out.status.success(), "openclaw --version failed");
    }

    /// A PTY whose child exits on its own must trip the waiter path (the
    /// `pty-exit` → map removal → teardown rail every kill/exit depends on,
    /// and the install-relaunch rail builds on). `ver` exits immediately.
    #[test]
    fn probe_pty_natural_exit_detection() {
        let app = tauri::test::mock_app();
        let handle = app.app_handle().clone();
        let home = tmp_home("probe-exit");
        handle.manage(AppState::new(home.clone()).unwrap());
        let st = state(&handle);
        let env: std::collections::HashMap<String, String> = Default::default();
        let h = crate::pty::spawn(
            "pty-probe",
            "ver",
            &home.display().to_string(),
            &env,
            80,
            24,
            handle.clone(),
        )
        .unwrap();
        st.ptys.lock().unwrap().insert("pty-probe".into(), h);
        let mut answered = 0usize;
        for i in 0..50 {
            // ConPTY emits CSI 6 n at session start and the child BLOCKS on
            // the reply — the real app's xterm answers it; here we must.
            {
                let mut ptys = st.ptys.lock().unwrap();
                if let Some(h) = ptys.get_mut("pty-probe") {
                    let cur = h.tail().matches("\u{1b}[6n").count();
                    for _ in answered..cur {
                        let _ = h.write("\u{1b}[24;80R");
                    }
                    answered = cur;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
            if !st.ptys.lock().unwrap().contains_key("pty-probe") {
                eprintln!("[probe] exit detected after {}ms", i * 200);
                return;
            }
        }
        let tail = st
            .ptys
            .lock()
            .unwrap()
            .get("pty-probe")
            .map(|h| h.tail())
            .unwrap_or_default();
        panic!("exit never detected after 10s; tail={tail:?}");
    }

    /// The spawn-vs-waiter race: the child exits (waiter runs its teardown)
    /// BEFORE spawn_inner inserts the handle. Forced deterministically here by
    /// inserting only after the exit marker appears. Without the reap the dead
    /// handle + agent mapping would stay forever ("already running").
    #[test]
    fn handle_inserted_after_child_already_exited_is_reaped() {
        let app = tauri::test::mock_app();
        let handle = app.app_handle().clone();
        let home = tmp_home("late-insert");
        handle.manage(AppState::new(home.clone()).unwrap());
        let st = state(&handle);
        let env: std::collections::HashMap<String, String> = Default::default();
        let mut h = crate::pty::spawn(
            "pty-late",
            "ver",
            &home.display().to_string(),
            &env,
            80,
            24,
            handle.clone(),
        )
        .unwrap();
        let spawn_gen = h.spawn_gen;
        // Wait for the waiter to observe the exit with nothing registered
        // (answering ConPTY's CSI 6n so `ver` can run).
        let mut answered = 0usize;
        let mut exited = false;
        for _ in 0..50 {
            let cur = h.tail().matches("\u{1b}[6n").count();
            for _ in answered..cur {
                let _ = h.write("\u{1b}[24;80R");
            }
            answered = cur;
            if crate::pty::EARLY_EXITS
                .lock()
                .unwrap()
                .get("pty-late")
                .is_some_and(|(g, _)| *g == spawn_gen)
            {
                exited = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        assert!(exited, "waiter never reported the exit");
        // Now do what spawn_inner does — too late.
        st.ptys.lock().unwrap().insert("pty-late".into(), h);
        st.map_pty("pty-late", "late-agent");
        crate::pty::reap_if_already_exited(&handle, "pty-late", spawn_gen);
        assert!(!st.ptys.lock().unwrap().contains_key("pty-late"), "dead handle left behind");
        assert!(st.pty_for_agent("late-agent").is_none(), "stale agent mapping left behind");
        assert!(crate::pty::EARLY_EXITS.lock().unwrap().get("pty-late").is_none());
    }

    /// A FRESH hire whose spawn fails after registration leaves no registry
    /// ghost; a failed respawn of an EXISTING agent keeps its row (gone).
    #[test]
    fn failed_fresh_spawn_removes_its_registry_row_but_respawn_keeps_it() {
        let app = tauri::test::mock_app();
        let handle = app.app_handle().clone();
        let home = tmp_home("fresh-fail");
        handle.manage(AppState::new(home.clone()).unwrap());
        let st = state(&handle);
        let missing = home.join("no-such-dir").display().to_string();
        let res = spawn_agent_core(
            &handle,
            req(serde_json::json!({
                "id": "pty-ghost", "cwd": missing, "command": "cmd",
                "hive": { "id": "ghost", "name": "Ghost", "role": "worker" }
            })),
        );
        assert!(!res.ok, "spawn into a missing cwd must fail");
        assert!(
            !st.hive.registry().iter().any(|a| a.id == "ghost"),
            "fresh hire left a registry ghost"
        );
        // Existing agent: failed respawn keeps the row, status gone.
        st.hive
            .register_agent(&crate::hive::RegistryEntry {
                id: "keeper".into(),
                name: "Keeper".into(),
                ..Default::default()
            })
            .unwrap();
        let res = spawn_agent_core(
            &handle,
            req(serde_json::json!({
                "id": "pty-keeper", "cwd": missing, "command": "cmd",
                "hive": { "id": "keeper", "name": "Keeper", "role": "worker" }
            })),
        );
        assert!(!res.ok);
        let row = st.hive.registry().into_iter().find(|a| a.id == "keeper").expect("row kept");
        assert_eq!(row.status, "gone");
    }

    #[test]
    fn hive_remove_agent_drops_row_and_god_pointer() {
        let st = AppState::new(tmp_home("rm-agent")).unwrap();
        st.hive
            .register_agent(&crate::hive::RegistryEntry {
                id: "g".into(),
                name: "G".into(),
                is_god: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(st.hive.god_id().as_deref(), Some("g"));
        assert!(st.hive.remove_agent("g").unwrap());
        assert!(!st.hive.remove_agent("g").unwrap());
        assert!(st.hive.registry().is_empty());
        assert!(st.hive.god_id().is_none());
    }

    /// A live handle (no early-exit marker) is never touched by the reap.
    #[test]
    fn reap_leaves_live_handles_alone() {
        let app = tauri::test::mock_app();
        let handle = app.app_handle().clone();
        let home = tmp_home("reap-live");
        handle.manage(AppState::new(home.clone()).unwrap());
        let st = state(&handle);
        let env: std::collections::HashMap<String, String> = Default::default();
        let h = crate::pty::spawn(
            "pty-alive", "cmd", &home.display().to_string(), &env, 80, 24, handle.clone(),
        )
        .unwrap();
        let g = h.spawn_gen;
        st.ptys.lock().unwrap().insert("pty-alive".into(), h);
        st.map_pty("pty-alive", "alive-agent");
        crate::pty::reap_if_already_exited(&handle, "pty-alive", g);
        assert!(st.ptys.lock().unwrap().contains_key("pty-alive"));
        assert_eq!(st.pty_for_agent("alive-agent").as_deref(), Some("pty-alive"));
        let h = st.ptys.lock().unwrap().remove("pty-alive");
        if let Some(mut h) = h {
            let _ = h.kill();
        }
    }

    /// The install PTY's exit-0 → relaunch rail (upstream
    /// pendingInstallRelaunch): an armed entry whose installer finishes
    /// cleanly must re-run the SAME spawn in the same slot. `exit /b 0`
    /// stands in for the installer, `more` (a stdin echo filter that stays
    /// alive) for the agent CLI — deterministic, offline, seconds.
    #[test]
    fn install_exit_zero_relaunches_same_slot() {
        let app = tauri::test::mock_app();
        let handle = app.app_handle().clone();
        let home = tmp_home("install-relaunch");
        handle.manage(AppState::new(home.clone()).unwrap());
        let st = state(&handle);

        // Arm exactly the way spawn_inner does once the installer PTY is up.
        let (mut args, mut plan) = translate_spawn_request(&req(serde_json::json!({
            "id": "pty-x", "cwd": home.display().to_string(),
            "command": "more", "provider": "custom",
            "hive": { "id": "agent-x", "name": "X", "role": "worker" }
        })))
        .unwrap();
        args.no_auto_install = true;
        plan.pty_id = Some("pty-x".into());
        plan.auto_type_seed = true;
        PENDING_INSTALLS
            .lock()
            .unwrap()
            .insert("pty-x".into(), PendingInstall { args, plan });

        // A PTY that exits 0 immediately — "the installer just finished".
        // (`exit` under `cmd /c` quits cmd with code 0.)
        let env: std::collections::HashMap<String, String> = Default::default();
        let h = crate::pty::spawn(
            "pty-x",
            "exit 0",
            &home.display().to_string(),
            &env,
            80,
            24,
            handle.clone(),
        )
        .unwrap();
        st.ptys.lock().unwrap().insert("pty-x".into(), h);

        // The waiter thread consumes the arm and re-spawns the agent's real
        // command into the same slot. ConPTY's startup CSI 6 n blocks the
        // child until a terminal answers — in the app that's xterm; here the
        // test replies with a fixed position. (The relaunched `more` PTY asks
        // too, so keep answering.)
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut answered = 0usize;
        loop {
            {
                let mut ptys = st.ptys.lock().unwrap();
                if let Some(h) = ptys.get_mut("pty-x") {
                    let cur = h.tail().matches("\u{1b}[6n").count();
                    if cur < answered {
                        answered = 0; // relaunch swapped in a fresh handle
                    }
                    for _ in answered..cur {
                        let _ = h.write("\u{1b}[24;80R");
                    }
                    answered = cur;
                }
            }
            let armed = PENDING_INSTALLS.lock().unwrap().contains_key("pty-x");
            let (present, relaunched) = {
                let ptys = st.ptys.lock().unwrap();
                let cur = ptys.get("pty-x").map(|h| h.command.contains("more")).unwrap_or(false);
                (ptys.contains_key("pty-x"), cur)
            };
            if !armed && relaunched {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "relaunch never happened (armed={armed} pty_present={present} relaunched={relaunched})"
            );
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let pty = { st.ptys.lock().unwrap().remove("pty-x") };
        if let Some(mut h) = pty {
            let _ = h.kill();
        }
    }
}
