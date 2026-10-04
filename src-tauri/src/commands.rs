//! Tauri commands — the IPC surface replacing Electron's ipcMain.handle.
//! Frontend calls these via `invoke()`; async work emits events back.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Result;
use serde::{Deserialize, Serialize};
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
    pub ptys: Mutex<HashMap<String, PtyHandle>>,
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
    let name = if cfg!(windows) { "hive-hook.exe" } else { "hive-hook" };
    let src = std::env::current_exe()
        .ok()?
        .parent()?
        .join(name);
    if !src.is_file() {
        return None;
    }
    let dest = harness_home.join("bin").join(name);
    if let Some(p) = dest.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    std::fs::copy(&src, &dest).ok().map(|_| dest)
}

pub fn state(app: &AppHandle) -> State<'_, AppState> {
    app.state::<AppState>()
}

// ─── agent lifecycle ────────────────────────────────────────────────────────

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
    #[serde(default)]
    pub no_auto_install: bool,
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

#[derive(Serialize)]
pub struct Spawned {
    pub id: String,
}

/// Upstream `pendingInstallRelaunch` — install PTYs armed to re-run their
/// agent spawn on a clean exit (the retry carries `no_auto_install` so the
/// ladder can never loop).
pub(crate) static PENDING_INSTALLS: std::sync::LazyLock<
    Mutex<HashMap<String, SpawnArgs>>,
> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

#[tauri::command]
pub fn spawn_agent(app: AppHandle, args: SpawnArgs) -> Result<Spawned, String> {
    spawn_inner(app, args, None, None)
}

/// Respawn a registry agent — same id, same hive folder/memory, optionally
/// resuming its recorded session (claude --resume <session_id>). This is the
/// restart/restore-team path: the registry survives, only the PTY died.
#[tauri::command]
pub fn respawn_agent(app: AppHandle, agent_id: String) -> Result<Spawned, String> {
    let st = state(&app);
    let entry = st
        .hive
        .registry()
        .into_iter()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| format!("no agent {agent_id}"))?;
    if st.ptys.lock().unwrap().contains_key(&agent_id) {
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
        no_auto_install: false,
        cols: 120,
        rows: 30,
    };
    // Claude resumes by session id; other providers start fresh (their resume
    // flags differ — PORTING.md notes the preset fields for later).
    let resume = (entry.provider == "claude")
        .then(|| entry.session_id.clone())
        .flatten();
    spawn_inner(app, args, Some(agent_id), resume)
}

pub(crate) fn spawn_inner(
    app: AppHandle,
    args: SpawnArgs,
    forced_id: Option<String>,
    resume_session: Option<String>,
) -> Result<Spawned, String> {
    let st = state(&app);
    let id = forced_id.clone().unwrap_or_else(|| {
        format!(
            "{}-{}",
            args.name.to_lowercase().replace(' ', "-"),
            &uuid::Uuid::new_v4().simple().to_string()[..4]
        )
    });

    // Git isolation (index.ts:2730): when requested and the cwd is a real
    // repo, give the agent its own `agent/<id>` worktree under
    // <harness>/worktrees/ — it can't clobber other agents' (or the user's)
    // working tree. Best-effort: a failure falls back to the shared cwd.
    let mut effective_cwd = args.cwd.clone();
    if args.isolate && crate::git_ops::is_repo(std::path::Path::new(&args.cwd)) {
        let seg: String = id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' })
            .collect();
        let wt_root = st.hive.root.parent().unwrap_or(&st.hive.root).join("worktrees");
        let wt_path = wt_root.join(&seg);
        // Path-safety: the resolved worktree path must stay under wt_root.
        if wt_path.starts_with(&wt_root) {
            let base = crate::git_ops::branch(std::path::Path::new(&args.cwd))
                .and_then(|b| b["current"].as_str().map(str::to_string))
                .unwrap_or_else(|| "main".into());
            let branch = crate::git_ops::agent_branch_for(&wt_path);
            match crate::git_ops::add_worktree(
                std::path::Path::new(&args.cwd),
                &wt_path,
                Some(&branch),
                Some(&base),
            ) {
                Ok(_) => {
                    effective_cwd = wt_path.display().to_string();
                    st.worktree_paths.lock().unwrap().insert(
                        id.clone(),
                        (wt_path.clone(), PathBuf::from(&args.cwd)),
                    );
                    if let Err(e) = crate::git_ops::link_worktree_deps(
                        std::path::Path::new(&args.cwd),
                        &wt_path,
                    ) {
                        eprintln!("[worktree] dependency link failed: {e}");
                    }
                }
                Err(e) => eprintln!("[worktree] addWorktree failed: {e}"),
            }
        }
    }

    let entry = RegistryEntry {
        id: id.clone(),
        name: args.name.clone(),
        provider: args.provider.clone(),
        role: args.role.clone().unwrap_or_default(),
        cwd: effective_cwd.clone(),
        is_god: args.is_god,
        status: "idle".into(),
        session_id: None,
        command: args.command.clone(),
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
    let agent_dir = st.hive.agent_dir(&id);
    let is_claude = args.provider == "claude";
    let mut agent_env = HashMap::from([
        ("AGENT_ID".to_string(), id.clone()),
        ("AGENT_NAME".to_string(), args.name.clone()),
        ("HIVE_ROOT".to_string(), st.hive.root.display().to_string()),
        ("AGENT_DIR".to_string(), agent_dir.display().to_string()),
    ]);
    if auto {
        agent_env.insert("HIVE_AUTO_APPROVE".into(), "1".into());
    }
    if let Some(sock) = &st.sock_addr {
        agent_env.insert("HIVE_SOCK".into(), sock.clone());
    }
    // Integration broker token (integrationBroker.ts grant(workerId,
    // enabledIds)) — MD_BROKER_URL/MD_BROKER_TOKEN env for worker spawn;
    // no token is granted when no integrations are enabled.
    {
        let home = st.hive.root.parent().unwrap_or(&st.hive.root).to_path_buf();
        let ids = crate::integrations::enabled_ids(&home);
        if !ids.is_empty() {
            // grant mints (or rotates) the worker's capability token.
            let token = crate::integrations::BROKER.grant(&id, ids);
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
    for key in &st.config.lock().unwrap().secret_env_keys {
        if let Ok(entry) = keyring::Entry::new("agent-office", key) {
            if let Ok(v) = entry.get_password() {
                agent_env.insert(key.clone(), v);
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
        let bundled = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| p.join("resources").join("skills")));
        if let Some(src) = bundled {
            if src.is_dir() {
                let _ = crate::hive::copy_dir(&src, &agent_dir.join(".claude").join("skills"));
            }
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
        st.hive.expose_codex_data(&agent_dir, &id);
    }
    // Hermes lifecycle bridge — per-agent HERMES_HOME + config.yaml `hooks:`
    // block on the same shim (--hermes mode). Runtime dir is isolated too so
    // multiplex state never collides with the user's own hermes.
    if args.provider == "hermes" {
        if let Some(bin) = &st.hook_bin {
            if let Ok(home) = st.hive.install_hermes_hooks(&agent_dir, bin) {
                agent_env.insert("HERMES_HOME".into(), home.display().to_string());
                agent_env.insert(
                    "HERMES_RUNTIME_DIR".into(),
                    home.join("runtime").display().to_string(),
                );
                agent_env.insert("HERMES_ACCEPT_HOOKS".into(), "1".into());
            }
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
    // user's REAL base URL (read from the parent env before we rewrite it).
    if args.provider == "qwen" || args.provider == "crush" {
        if let Some(sock) = &st.sock_addr {
            let upstream = std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".into());
            let bridge = std::sync::Arc::new(crate::proxy::ProxyBridge::new(
                id.clone(),
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
                        .insert(id.clone(), bridge);
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
                    let msg = format!("proxy bridge failed to bind — agent spawns without hive events: {e}");
                    let _ = st.hive.log(&serde_json::json!({
                        "kind": "proxy-degraded", "agentId": id,
                        "name": args.name, "provider": args.provider,
                    }));
                    let _ = app.emit(
                        "hive:degraded",
                        serde_json::json!({
                            "agentId": id, "name": args.name,
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

    let parent: HashMap<String, String> = std::env::vars().collect();
    // withHiveRuntimeFallback: the hive's bundled-runtime dir (if any) is
    // APPENDED to the child PATH so provider subprocesses that shell out to
    // `node` work even on machines without a system install.
    let mut path = parent.get("PATH").cloned().unwrap_or_default();
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
    let env = build_pty_env(&parent, &path, Some(&agent_env), cfg!(windows));

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
    let injected = st.hive.injected_prompt(&entry, &prompt_opts);
    let mut command_line = args.command.clone();
    let mut seed = args.seed_prompt.clone();

    if is_claude {
        if let (Some(_sock), Some(bin)) = (&st.sock_addr, &st.hook_bin) {
            let shim_cmd = format!("\"{}\"", bin.display());
            let settings = st.hive.hook_settings(&shim_cmd);
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
    let first_tok = command_line
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();

    // Missing-CLI auto-install (spawnAgentCore → pendingInstallRelaunch): the
    // provider binary can't be resolved — run the bundled installer in the
    // same terminal slot, then re-run THIS spawn on exit-0 (with
    // no_auto_install so the ladder never loops). A provider with no installer
    // spec, or a machine where no rung exists, falls through to the normal
    // spawn and dies on its own missing-binary error — same as upstream.
    if !args.no_auto_install && pty::resolve_program(&first_tok, &path).is_none() {
        let npm_available = pty::resolve_program("npm", &path).is_some();
        let node_probe = crate::misc::node_install_command().and_then(|(probe, _)| {
            pty::resolve_program(probe, &path).is_some().then_some(probe)
        });
        if let Some((script, relaunch)) =
            crate::misc::install_script(&args.provider, &first_tok, npm_available, node_probe)
        {
            if relaunch {
                let mut retry = args.clone();
                retry.no_auto_install = true;
                PENDING_INSTALLS.lock().unwrap().insert(id.clone(), retry);
            }
            let handle = pty::spawn(&id, &script, &cwd, &env, args.cols, args.rows, app.clone())
                .map_err(err_str)?;
            st.ptys.lock().unwrap().insert(id.clone(), handle);
            st.hive.set_status(&id, "idle").map_err(err_str)?;
            return Ok(Spawned { id });
        }
    }

    let handle = match pty::resolve_program(&first_tok, &path) {
        Some(pty::Resolved::Exe(prog)) => {
            let mut av: Vec<String> = command_line
                .split_whitespace()
                .skip(1)
                .map(String::from)
                .collect();
            if is_claude {
                av.push("--append-system-prompt".into());
                av.push(injected.clone());
            }
            if let Some(sid) = &resume_session {
                av.push("--resume".into());
                av.push(sid.clone());
            }
            pty::spawn_argv(&id, &prog, &av, &cwd, &env, args.cols, args.rows, app.clone())
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
                av.extend(command_line.split_whitespace().skip(1).map(String::from));
                if is_claude {
                    av.push("--append-system-prompt".into());
                    av.push(injected.clone());
                }
                if let Some(sid) = &resume_session {
                    av.push("--resume".into());
                    av.push(sid.clone());
                }
                pty::spawn_argv(&id, &exe, &av, &cwd, &env, args.cols, args.rows, app.clone())
            } else {
            // Fallback when the shim can't be decoded: route through cmd.exe
            // as RAW tokens — portable-pty CRT-quotes each arg, which is the
            // same line shape upstream's buildCmdCommandLine emits verbatim.
            // (cmd_sanitize still applies: cmd cannot carry a literal newline
            // at all — upstream truncates at it; collapsing is strictly less
            // destructive.)
            let mut parts = vec![prog.display().to_string()];
            parts.extend(command_line.split_whitespace().skip(1).map(String::from));
            if is_claude {
                parts.push("--append-system-prompt".into());
                parts.push(pty::cmd_sanitize(&injected));
            }
            if let Some(sid) = &resume_session {
                parts.push("--resume".into());
                parts.push(sid.clone());
            }
            pty::spawn_cmd(&id, &parts, &cwd, &env, args.cols, args.rows, app.clone())
            }
        }
        None => {
            let mut parts: Vec<String> =
                command_line.split_whitespace().map(String::from).collect();
            if is_claude {
                parts.push("--append-system-prompt".into());
                parts.push(pty::cmd_sanitize(&injected));
            }
            if let Some(sid) = &resume_session {
                parts.push("--resume".into());
                parts.push(sid.clone());
            }
            pty::spawn_cmd(&id, &parts, &cwd, &env, args.cols, args.rows, app.clone())
        }
    }
    .map_err(err_str)?;
    st.wake.lock().unwrap().note_spawn(
        &id,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    );
    st.ptys.lock().unwrap().insert(id.clone(), handle);
    st.hive.set_status(&id, "idle").map_err(err_str)?;

    // 'type-into-tui' seed delivery: the injected hive prompt (or an explicit
    // seed) typed into the session once the TUI has booted — upstream's
    // seedPrompt path for crush/cursor, used here for every non-claude agent.
    if let Some(text) = seed {
        let app2 = app.clone();
        let id2 = id.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            let Some(st) = app2.try_state::<AppState>() else { return };
            let mut ptys = st.ptys.lock().unwrap();
            if let Some(h) = ptys.get_mut(&id2) {
                let _ = h.write(&format!("{}\r", text.split_whitespace().collect::<Vec<_>>().join(" ")));
            }
        });
    }

    let _ = app.emit(
        "agent-status",
        serde_json::json!({ "id": id, "status": "idle" }),
    );
    Ok(Spawned { id })
}

#[tauri::command]
pub fn pty_write(app: AppHandle, id: String, data: String) -> Result<(), String> {
    let st = state(&app);
    let mut ptys = st.ptys.lock().unwrap();
    match ptys.get_mut(&id) {
        Some(h) => h.write(&data).map_err(err_str),
        None => Err(format!("no pty {id}")),
    }
}

#[tauri::command]
pub fn pty_resize(app: AppHandle, id: String, cols: u16, rows: u16) -> Result<(), String> {
    let st = state(&app);
    let ptys = st.ptys.lock().unwrap();
    match ptys.get(&id) {
        Some(h) => h.resize(cols, rows).map_err(err_str),
        None => Err(format!("no pty {id}")),
    }
}

#[tauri::command]
pub fn kill_agent(app: AppHandle, id: String) -> Result<(), String> {
    let st = state(&app);
    if let Some(mut h) = st.ptys.lock().unwrap().remove(&id) {
        let _ = h.kill();
    }
    // Dead agents can't leak a tripped breaker level or a pending steer.
    st.breaker.lock().unwrap().forget(&id);
    st.wake.lock().unwrap().forget(&id, Some(&id));
    st.telemetry.forget(&id);
    // A dead worker's broker capability token dies with it
    // (integrationBroker.ts revoke-on-teardown).
    crate::integrations::BROKER.revoke(&id);
    // Kill this agent's proxy-bridge sidecar (qwen/crush) — a dead PTY never
    // leaves an orphan loopback listener (teardownPty stopProxyBridge).
    if let Some(bridge) = st.proxy_bridges.lock().unwrap().remove(&id) {
        bridge.stop();
    }
    // teardownPty step 2: worker/live tracking + the isolated-worktree gate
    // (workers: preserve unintegrated work + defer scratch GC; regular:
    // upstream's immediate force-remove). Empties live_workers for this id.
    let was_worker = st.live_workers.lock().unwrap().contains_key(&id);
    crate::workers::finalize_teardown(&st, &id);
    if was_worker {
        // Mirrors teardownPty's hive:agentArchived broadcast — the dead
        // worker's floor card comes down even on a manual stop.
        let _ = app.emit("hive:agentArchived", serde_json::json!({ "id": id }));
    }
    st.hive.set_status(&id, "gone").map_err(err_str)?;
    let _ = app.emit(
        "agent-status",
        serde_json::json!({ "id": id, "status": "stopped" }),
    );
    Ok(())
}

// ─── hive surface ───────────────────────────────────────────────────────────

/// hive:registry — upstream returns the registry DOCUMENT `{godId, agents:{
/// <id>: entry}}` so the renderer can key by id and reach godId.
#[tauri::command]
pub fn hive_registry(app: AppHandle) -> Result<serde_json::Value, String> {
    let (god_id, agents) = state(&app).hive.read_registry_doc();
    let map: serde_json::Map<String, serde_json::Value> = agents
        .into_iter()
        .filter_map(|a| {
            serde_json::to_value(&a)
                .ok()
                .map(|v| (a.id.clone(), v))
        })
        .collect();
    Ok(serde_json::json!({ "godId": god_id, "agents": map }))
}

/// hive:send — upstream's exact contract: `{ok, message}` where the partial
/// is normalized and routed SYNCHRONOUSLY (index.ts:3503 — no outbox hop, so
/// virtual senders like 'human'/'system' never materialize an agents/<from>
/// dir).
#[tauri::command]
pub fn hive_send(
    app: AppHandle,
    partial: crate::hive::PartialMessage,
    from: Option<String>,
) -> Result<serde_json::Value, String> {
    let sender = from.unwrap_or_else(|| "system".into());
    let msg = state(&app).hive.send_partial(&partial, &sender).map_err(err_str)?;
    Ok(serde_json::json!({ "ok": true, "message": msg }))
}

#[tauri::command]
pub fn hive_inbox(app: AppHandle, agent_id: String) -> Result<Vec<Message>, String> {
    state(&app).hive.inbox(&agent_id).map_err(err_str)
}

#[tauri::command]
pub fn hive_mark_done(app: AppHandle, agent_id: String, msg_id: String) -> Result<(), String> {
    state(&app)
        .hive
        .mark_done(&agent_id, &msg_id)
        .map_err(err_str)
}

#[tauri::command]
pub fn hive_board(app: AppHandle) -> Result<String, String> {
    state(&app).hive.read_board().map_err(err_str)
}

#[tauri::command]
pub fn hive_tasks(app: AppHandle) -> Result<serde_json::Value, String> {
    state(&app).hive.read_tasks().map_err(err_str)
}

// ─── memory ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn read_memory(app: AppHandle, agent_id: String) -> Result<String, String> {
    state(&app).hive.read_memory(&agent_id).map_err(err_str)
}

#[tauri::command]
pub fn append_memory(app: AppHandle, agent_id: String, entry: String) -> Result<(), String> {
    state(&app)
        .hive
        .append_memory(&agent_id, &entry)
        .map_err(err_str)
}

// ─── config ─────────────────────────────────────────────────────────────────

#[tauri::command]
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

#[tauri::command]
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

#[tauri::command]
pub fn control_pause(app: AppHandle, agent_id: String, on: bool) -> Result<ControlSnapshot, String> {
    let st = state(&app);
    let mut c = st.control.lock().unwrap();
    c.pause(&agent_id, on);
    Ok(c.snapshot(&agent_id))
}

#[tauri::command]
pub fn control_resume(app: AppHandle, agent_id: String) -> Result<ControlSnapshot, String> {
    let st = state(&app);
    let mut c = st.control.lock().unwrap();
    c.resume(&agent_id);
    Ok(c.snapshot(&agent_id))
}

#[tauri::command]
pub fn control_halt(app: AppHandle, agent_id: String) -> Result<ControlSnapshot, String> {
    let st = state(&app);
    let mut c = st.control.lock().unwrap();
    c.halt(&agent_id);
    Ok(c.snapshot(&agent_id))
}

#[tauri::command]
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

#[tauri::command]
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

#[tauri::command]
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

#[tauri::command]
pub fn control_snapshot(app: AppHandle, agent_id: String) -> Result<ControlSnapshot, String> {
    Ok(state(&app).control.lock().unwrap().snapshot(&agent_id))
}

// ─── missions (scheduler) ───────────────────────────────────────────────────

#[tauri::command]
pub fn missions_list(app: AppHandle) -> Result<Vec<crate::missions::ScheduledMission>, String> {
    Ok(state(&app).config.lock().unwrap().missions.clone())
}

#[tauri::command]
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
    // The scheduler polls config — a save takes effect on the next tick for
    // free (no re-arm plumbing; that's the poll design's payoff).
    let _ = app.emit("missions:updated", serde_json::json!({}));
    Ok(())
}

// ─── secrets (OS keychain → agent env) ──────────────────────────────────────

const KEYRING_SERVICE: &str = "agent-office";

/// Read a credential — the caller's only path to a value (slack secrets and
/// spawn-time env injection both go through here).
fn secret_get(app: &AppHandle, key: &str) -> Option<String> {
    let _ = app;
    keyring::Entry::new(KEYRING_SERVICE, key)
        .ok()
        .and_then(|e| e.get_password().ok())
}

/// Store a secret in the OS credential store and register its env key for
/// injection. The value never touches config.json or log.jsonl.
#[tauri::command]
pub fn secret_set(app: AppHandle, key: String, value: String) -> Result<(), String> {
    let key = key.trim().to_string();
    if key.is_empty() || value.is_empty() {
        return Err("key and value required".into());
    }
    keyring::Entry::new(KEYRING_SERVICE, &key)
        .and_then(|e| e.set_password(&value))
        .map_err(|e| format!("keychain: {e}"))?;
    let st = state(&app);
    let mut cfg = st.config.lock().unwrap();
    if !cfg.secret_env_keys.contains(&key) {
        cfg.secret_env_keys.push(key);
        st.config_store.save(&cfg).map_err(err_str)?;
    }
    Ok(())
}

/// Remove a secret — from the keychain AND the injection list.
#[tauri::command]
pub fn secret_delete(app: AppHandle, key: String) -> Result<(), String> {
    if let Ok(e) = keyring::Entry::new(KEYRING_SERVICE, &key) {
        let _ = e.delete_credential();
    }
    let st = state(&app);
    let mut cfg = st.config.lock().unwrap();
    cfg.secret_env_keys.retain(|k| k != &key);
    st.config_store.save(&cfg).map_err(err_str)
}

/// The injection-list key NAMES — values are never exposed to the frontend.
#[tauri::command]
pub fn secrets_list(app: AppHandle) -> Result<Vec<String>, String> {
    Ok(state(&app).config.lock().unwrap().secret_env_keys.clone())
}

// ─── webhooks (inbound HTTP → god's inbox) ──────────────────────────────────

#[tauri::command]
pub fn webhooks_list(app: AppHandle) -> Result<Vec<crate::webhook::WebhookEndpoint>, String> {
    Ok(state(&app).config.lock().unwrap().webhooks.clone())
}

/// Persist the endpoint list and hot-swap it on the running server — a
/// removed id stops resolving on the very next request (upstream
/// setEndpoints semantics; the port never rebinds).
#[tauri::command]
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
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Some(port); // already bound; set_endpoints already applied
    }
    let wh = st.webhook.clone();
    let hive = st.hive.clone();
    let app2 = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = wh.run(port, hive, app2) {
            eprintln!("[webhook] server stopped: {e:#}");
            STARTED.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    });
    Some(port)
}

/// Approve a held inbound — upstream triggerHistory:decide with
/// decision='approved'. Takes the HISTORY ENTRY id (the pending row the UI
/// lists), releases the message down the identical card+god path, and flips
/// the ledger. Idempotent: already-decided entries are a no-op.
#[tauri::command]
pub fn webhook_approve(app: AppHandle, token: String) -> Result<serde_json::Value, String> {
    let st = state(&app);
    st.webhook
        .decide(&app, &st.hive, &token, "approved")
        .ok_or_else(|| "unknown or non-pending entry".to_string())
}

#[tauri::command]
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
#[tauri::command]
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
#[tauri::command]
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
#[tauri::command]
pub fn hire_drain_pending(app: AppHandle) -> Result<Option<String>, String> {
    let st = state(&app);
    let home = st.hive.root.parent().unwrap_or(&st.hive.root).to_path_buf();
    let p = pending_hire_path(&home);
    if !p.exists() {
        return Ok(None);
    }
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&p).map_err(|e| e.to_string())?).unwrap_or_default();
    let _ = std::fs::remove_file(&p);
    Ok(v.get("src").and_then(|s| s.as_str()).map(String::from))
}

// ─── config:update (generic patch — upstream's settings write path) ─────────

/// config:update — merge a partial JSON patch into config.json. Only keys
/// that exist on Config survive (serde drops the rest, same as upstream's
/// typed settings write); one level of object merge for nested knobs.
#[tauri::command]
/// config:update — writeConfig(patch): shallow merge at the top level (a
/// nested object REPLACES its subtree wholesale, matching upstream's spread),
/// onto the RAW file so unknown keys survive, then re-parse the typed Config.
/// Returns the same merged view get_config produces.
pub fn config_update(app: AppHandle, patch: serde_json::Value) -> Result<serde_json::Value, String> {
    let st = state(&app);
    let mut raw = st.config_store.raw_value();
    if let (Some(dst), Some(src)) = (raw.as_object_mut(), patch.as_object()) {
        for (k, val) in src {
            dst.insert(k.clone(), val.clone());
        }
    }
    let mut cfg = st.config.lock().unwrap();
    *cfg = serde_json::from_value(raw).map_err(|e| e.to_string())?;
    st.config_store.save(&cfg).map_err(err_str)?;
    drop(cfg);
    get_config(app)
}

/// webhook:setConfig — secret + port + enable toggle; the port applies on
/// next boot. `secret` rotates the legacy endpoint's shared secret.
#[tauri::command]
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
        let next_secret = secret.clone().or_else(|| prior.as_ref().map(|p| p.secret.clone()));
        if let Some(sec) = next_secret.filter(|s| !s.is_empty()) {
            let row = crate::webhook::WebhookEndpoint {
                id: "legacy".into(),
                name: prior
                    .as_ref()
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "Default webhook".into()),
                secret: sec,
                schema: prior
                    .as_ref()
                    .map(|p| p.schema.clone())
                    .unwrap_or_default(),
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
        Ok(serde_json::json!({ "ok": true, "manifest": m, "warnings": warns, "path": path.display().to_string() }))
    } else {
        Ok(serde_json::json!({ "ok": false, "errors": errors }))
    }
}

// ─── app lifecycle (close gating + info) ────────────────────────────────────

/// app:confirmClose — kick the closing-time protocol (same rail as
/// closing_time_start; frontend shows "Confirm close" → this).
#[tauri::command]
pub fn app_confirm_close(app: AppHandle) -> Result<usize, String> {
    closing_time_start(app)
}

/// app:cancelClose — abort the in-flight closing-time protocol.
#[tauri::command]
pub fn app_cancel_close(app: AppHandle) -> Result<bool, String> {
    closing_time_cancel(app)
}

/// app:info — version + home path for the About surface.
#[tauri::command]
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
#[tauri::command]
pub fn skills_reveal(app: AppHandle, id: String) -> Result<bool, String> {
    let st = state(&app);
    let cwds: Vec<String> = st.hive.registry().iter().map(|a| a.cwd.clone()).collect();
    let bundled = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.join("resources").join("skills")));
    let dir = crate::skills::list_local_skills(&cwds, bundled.as_deref())
        .into_iter()
        .find(|s| s.id == id)
        .map(|s| s.path)
        .ok_or_else(|| "unknown skill".to_string())?;
    #[cfg(windows)]
    std::process::Command::new("explorer").arg(&dir).spawn().ok();
    #[cfg(not(windows))]
    std::process::Command::new("open").arg(&dir).spawn().ok();
    Ok(true)
}

// ─── memory reflector (condense oversized memory.md via headless claude) ────

/// Manually condense one agent's memory.md — skips the trigger (on-demand);
/// the full backup→summarize→rebuild→verify→swap pipeline still runs.
#[tauri::command]
pub fn reflect_now(app: AppHandle, agent_id: Option<String>) -> Result<Vec<serde_json::Value>, String> {
    let st = state(&app);
    let settings = crate::reflect::ReflectSettings { enabled: true, ..Default::default() };
    let hive_root = st.hive.root.clone();
    let path = std::env::var("PATH").unwrap_or_default();
    let claude = match pty::resolve_program("claude", &path) {
        Some(pty::Resolved::Exe(p)) | Some(pty::Resolved::Script(p)) => p.display().to_string(),
        None => "claude".to_string(),
    };
    if let Some(id) = agent_id {
        return Ok(vec![crate::reflect::condense_one(&hive_root, &id, &claude, &settings, true)]);
    }
    Ok(crate::reflect::reflect_scan(&hive_root, &claude, &settings))
}

// ─── git pane (read surface; safe_resolve confinement on the caller path) ──

/// One git snapshot for the pane — is_repo? branch? counts? ahead/behind?
/// The `cwd` argument is the agent's workspace directory.
#[tauri::command]
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
#[tauri::command]
pub fn fs_list(app: AppHandle, root: String, rel: String) -> Result<Vec<serde_json::Value>, String> {
    let _ = app;
    crate::fs_ops::list_dir(std::path::Path::new(&root), &rel)
        .ok_or_else(|| "path escapes the workspace".to_string())
}

#[tauri::command]
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

#[tauri::command]
pub fn kv_get(app: AppHandle, key: String) -> Result<serde_json::Value, String> {
    Ok(state(&app)
        .persist
        .lock()
        .unwrap()
        .get_kv(&key)
        .unwrap_or(serde_json::Value::Null))
}

#[tauri::command]
pub fn kv_set(app: AppHandle, key: String, value: serde_json::Value) -> Result<bool, String> {
    state(&app).persist.lock().unwrap().set_kv(&key, &value);
    Ok(true)
}

/// Record one user-submitted prompt (db.ts addHistory).
#[tauri::command]
pub fn command_history_add(app: AppHandle, agent_id: String, cwd: Option<String>, text: String) -> Result<bool, String> {
    state(&app).persist.lock().unwrap().add_history(&agent_id, cwd.as_deref(), &text);
    Ok(true)
}

#[tauri::command]
pub fn command_history_list(app: AppHandle, agent_id: Option<String>, limit: Option<i64>) -> Result<Vec<serde_json::Value>, String> {
    Ok(state(&app).persist.lock().unwrap().list_history(agent_id.as_deref(), limit.unwrap_or(100)))
}

#[tauri::command]
pub fn command_history_search(app: AppHandle, query: String, limit: Option<i64>) -> Result<Vec<serde_json::Value>, String> {
    Ok(state(&app).persist.lock().unwrap().search_history(&query, limit.unwrap_or(50)))
}

// ─── hire manifests (munderdifflin://hire deep link + file import) ──────────

/// Validate + return a hire manifest (prefill only — never auto-spawns).
#[tauri::command]
pub fn hire_validate(raw: serde_json::Value) -> serde_json::Value {
    let (m, errors, consent) = crate::hire::validate_hire_manifest(&raw);
    serde_json::json!({ "ok": m.is_some(), "manifest": m, "errors": errors, "consentRequired": consent })
}

/// Import from an https URL — SSRF-gated (resolved addresses blocklisted).
#[tauri::command]
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
#[tauri::command]
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
#[tauri::command]
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
    match st.slack_reply.start(
        0,
        move || secret_get(&app4, "SLACK_BOT_TOKEN"),
        {
            let st2 = std::sync::Arc::clone(&st.directly_replied);
            move |thread_ts| {
                st2.lock().unwrap().insert(thread_ts);
            }
        },
    ) {
        Ok(reply_port) => {
            let cfg_path = crate::misc::harness_home(&app).join("slack-reply.json");
            let _ = std::fs::write(
                &cfg_path,
                serde_json::json!({ "port": reply_port, "token": st.slack_reply.token })
                    .to_string(),
            );
        }
        Err(e) => eprintln!("[slack] reply endpoint failed to start: {e}"),
    }
    // Public tunnel (upstream slack.ts openTunnel — Slack's servers need a
    // reachable Request URL). Non-fatal: the local handler is the security
    // boundary and stays up even if the tunnel can't be established.
    match crate::misc::start_tunnel(port) {
        Ok((url, child)) => *st.slack_tunnel.lock().unwrap() = Some((url, child)),
        Err(e) => eprintln!("[slack] tunnel unavailable: {e}"),
    }
    Ok(true)
}

#[tauri::command]
pub fn slack_stop(app: AppHandle) -> Result<bool, String> {
    let st = state(&app);
    st.slack.stop();
    st.slack_reply.stop();
    if let Some((_, mut child)) = st.slack_tunnel.lock().unwrap().take() {
        let _ = child.kill();
    }
    // The discovery file advertises a dead socket once the rail is down —
    // drop it so a stale helper doesn't talk to an unrelated listener.
    let _ = std::fs::remove_file(crate::misc::harness_home(&app).join("slack-reply.json"));
    Ok(true)
}

#[tauri::command]
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
#[tauri::command]
pub fn slack_reply(app: AppHandle, channel: String, thread_ts: String, text: String) -> Result<bool, String> {
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
static SLACK_DONE_BASELINE: std::sync::OnceLock<
    Mutex<Option<std::collections::HashSet<String>>>,
> = std::sync::OnceLock::new();

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
    let Ok(doc) = st.hive.read_tasks() else { return };
    let Some(tasks) = doc["tasks"].as_array() else { return };

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
        let _ = std::fs::write(&notified_path, serde_json::to_string(&v).unwrap_or_default());
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
                eprintln!("[slack] done-summary for task {id} — giving up (terminal error: {e})");
            }
            Err(e) => {
                eprintln!("[slack] done-summary for task {id} failed — will retry: {e}");
            }
        }
    }
}

// ─── closing time (graceful shutdown protocol) ──────────────────────────────

/// Press "closing time": brief god + steer every live agent, then the router
/// observer tracks worker ACKs and the verified COMPLETE.
#[tauri::command]
pub fn closing_time_start(app: AppHandle) -> Result<usize, String> {
    let st = state(&app);
    let live: std::collections::HashSet<String> = st.ptys.lock().unwrap().keys().cloned().collect();
    let mut control = st.control.lock().unwrap();
    let n = st.closing.start(&st.hive, &mut control, &live)?;
    emit_closing(&app, "started");
    Ok(n)
}

#[tauri::command]
pub fn closing_time_cancel(app: AppHandle) -> Result<bool, String> {
    let st = state(&app);
    let mut control = st.control.lock().unwrap();
    st.closing.cancel(&st.hive, &mut control);
    emit_closing(&app, "cancelled");
    Ok(true)
}

#[tauri::command]
pub fn closing_time_status(app: AppHandle) -> Result<serde_json::Value, String> {
    let st = state(&app);
    let (acked, total) = st.closing.progress();
    Ok(serde_json::json!({
        "active": st.closing.is_active(), "acked": acked, "total": total
    }))
}

/// Emit the protocol state to the floor UI (upstream 'app:closingTime').
pub(crate) fn emit_closing(app: &AppHandle, phase: &str) {
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

#[tauri::command]
pub fn roster_read(app: AppHandle) -> Result<Option<serde_json::Value>, String> {
    Ok(state(&app).roster.read())
}

#[tauri::command]
pub fn roster_write(app: AppHandle, snap: serde_json::Value) -> Result<bool, String> {
    // A declined empty-first-write is not an error for the caller — the file
    // was protected, which is the outcome the renderer wants either way.
    match state(&app).roster.write(&snap) {
        Ok(()) => Ok(true),
        Err(e) if e.starts_with("skipped:") => Ok(false),
        Err(e) => Err(e),
    }
}

#[tauri::command]
pub fn roster_archive(app: AppHandle) -> Result<bool, String> {
    state(&app).roster.archive();
    Ok(true)
}

/// Lifetime spend folded from cost-ledger.jsonl — the reset-safe number
/// (telemetry's cumulative-since-boot counter understates post-restart).
#[tauri::command]
pub fn cost_lifetime(app: AppHandle) -> Result<serde_json::Value, String> {
    let st = state(&app);
    let mut t = crate::cost_lifetime::CostLedgerTotals::default();
    t.refresh_fully(&st.hive.cost_ledger_path());
    Ok(serde_json::json!({
        "agents": t.all(),
        "floor_total": t.floor_total(),
    }))
}

// ─── task ledger writes (patchTaskInLedger semantics) ───────────────────────

#[tauri::command]
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
#[tauri::command]
pub fn task_answer(app: AppHandle, id: String, answer: String) -> Result<serde_json::Value, String> {
    state(&app).hive.task_answer(&id, &answer).map_err(err_str)
}

// ─── skills (local discovery + catalog + hardened install) ─────────────────

#[tauri::command]
pub fn skills_local(cwds: Vec<String>) -> Result<Vec<crate::skills::LocalSkill>, String> {
    // Bundled skills staged next to the running exe (app resources/skills/).
    let bundled = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.join("resources").join("skills")));
    Ok(crate::skills::list_local_skills(&cwds, bundled.as_deref()))
}

#[tauri::command]
pub fn skills_catalog(app: AppHandle, force: Option<bool>) -> Result<crate::skills::CatalogResult, String> {
    let cache = state(&app).hive.root.join("skills-catalog.json");
    Ok(crate::skills::load_catalog(&cache, force.unwrap_or(false)))
}

#[tauri::command]
pub fn skills_install(url: String, name: String) -> Result<String, String> {
    crate::skills::install_skill(&url, &name)
}

#[tauri::command]
pub fn skills_uninstall(path: String, cwds: Vec<String>) -> Result<bool, String> {
    crate::skills::uninstall_skill(&path, &cwds)?;
    Ok(true)
}

// ─── trigger ledger (inbound webhook history + verdicts) ────────────────────

#[tauri::command]
pub fn triggers_list(app: AppHandle) -> Result<Vec<crate::trigger_history::TriggerHistoryEntry>, String> {
    Ok(state(&app).trigger_history.list())
}

#[tauri::command]
pub fn triggers_clear(app: AppHandle, source: Option<String>) -> Result<bool, String> {
    state(&app).trigger_history.clear(source.as_deref());
    Ok(true)
}

// ─── breaker (the beat's data seam + level read) ────────────────────────────

#[tauri::command]
pub fn breaker_level(app: AppHandle, agent_id: String) -> Result<String, String> {
    let lvl = state(&app).breaker.lock().unwrap().level_for(&agent_id);
    Ok(format!("{lvl:?}").to_lowercase())
}

fn err_str(e: anyhow::Error) -> String {
    format!("{e:#}")
}
