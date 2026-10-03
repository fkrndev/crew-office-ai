//! The hive — port of `src/main/hive.ts` core layer.
//!
//! On-disk layout and semantics follow HIVE.md exactly:
//!
//! ```text
//! hive/  PROTOCOL.md registry.json board.md tasks.json log.jsonl
//!   agents/<id>/ identity.md memory.md inbox/ inbox/.done/ outbox/ cursor.json
//! ```
//!
//! Locked decisions carried over:
//!  - git as the coordination/audit layer, SINGLE COMMITTER: only this process
//!    commits (via git2); agents write plain files, never call git.
//!  - single-writer-per-file; cross-agent delivery happens by the router
//!    moving message files outbox → inbox via atomic rename.
//!  - append-only log.jsonl; consumers track their own cursor.
//!
//! Deviation from the Electron version: the router is a poll loop (called on a
//! tokio interval) instead of an fs watcher — same contract, fewer deps.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// FIPA-lite message schema (HIVE.md §4) — field-for-field port.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    #[serde(default)]
    pub conversation: String,
    #[serde(default)]
    pub in_reply_to: Option<String>,
    pub from: String,
    pub to: String,
    #[serde(default = "default_act")]
    pub act: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub hops: u32,
    #[serde(default)]
    pub requires_reply: bool,
    #[serde(default)]
    pub needs_human: bool,
    #[serde(default)]
    pub created_at: String,
}

fn default_act() -> String {
    "inform".into()
}

impl Message {
    pub fn new(from: &str, to: &str, act: &str, subject: &str, body: &str) -> Self {
        let ts = Utc::now();
        Self {
            // time-sortable unique id, same shape as the Electron version.
            id: format!(
                "{}-{}",
                ts.format("%Y-%m-%dT%H-%M-%S-%3fZ"),
                &Uuid::new_v4().simple().to_string()[..4]
            ),
            conversation: String::new(),
            in_reply_to: None,
            from: from.into(),
            to: to.into(),
            act: act.into(),
            subject: subject.into(),
            body: body.into(),
            hops: 0,
            requires_reply: matches!(act, "request" | "query" | "propose"),
            needs_human: false,
            created_at: ts.to_rfc3339(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegistryEntry {
    pub id: String,
    pub name: String,
    pub provider: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub is_god: bool,
    #[serde(default)]
    pub status: String,
    /// The CLI's own session id (from hook payloads) — for --resume + cost
    /// dedup, same as upstream's recordSession.
    #[serde(default)]
    pub session_id: Option<String>,
    /// The spawn command line — kept so respawn/restart can rebuild the PTY
    /// without asking the UI for the original invocation again.
    #[serde(default)]
    pub command: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Delivery {
    pub from: String,
    pub to: String,
    pub msg_id: String,
    pub bounced: bool,
}

const PROTOCOL_MD: &str = r#"# Hive Protocol

You are an agent inside the office hive. Coordination happens through FILES —
the harness routes them; you never call git.

Your folder (see env): `$AGENT_DIR`
- `identity.md` — who you are (written by the harness; do not edit)
- `memory.md`   — YOUR long-term memory. Read it when you start; append what
                  you learn. It survives restarts.
- `inbox/`      — messages delivered to you, one `<ts>-<id>.json` each
- `inbox/.done/`— move a message here once handled (audit trail)
- `outbox/`     — drop a message file here to SEND it. The router delivers it.
- `cursor.json` — `{ "lastProcessed": "<msgid>" }` to track what you handled

## Sending a message

Write one JSON file per message into your `outbox/` — via a temp file then
rename so the router never sees a half-written file:

```jsonc
{
  "id":            "2026-05-30T14-03-11-123Z-a1b2",
  "conversation":  "conv-7f3",
  "in_reply_to":   "<prev msgid> | null",
  "from":          "<your agent id>",
  "to":            "<agent id> | god | broadcast",
  "act":           "request | inform | propose | query | agree | refuse | done",
  "subject":       "short summary",
  "body":          "free text / markdown",
  "hops":          0,
  "requires_reply": true,
  "needs_human":   false,
  "created_at":    "ISO-8601"
}
```

Rules:
- Only `request` / `query` / `propose` obligate a reply. `inform` / `done` are
  terminal — do not reply to them.
- Every reply increments `hops`. Past the hop cap the orchestrator escalates
  instead of letting two agents loop forever.
- Address `god` for anything cross-agent you cannot resolve. Address `human`
  only for spend, destructive ops, or scope changes — it routes through god.
- Set `needs_human: true` to escalate.

Env vars available to you: AGENT_ID, AGENT_NAME, HIVE_ROOT, AGENT_DIR.
"#;

const BOARD_MD: &str = "# Shared board\n\nCo-authored plans live here. Written by the god agent (single scribe).\n";
const TASKS_JSON: &str = "{\n  \"tasks\": []\n}\n";

/// Newest mtime (epoch-ms) of a path — a file's own mtime, or the newest
/// direct child of a directory (one level: enough for inbox/outbox dirs).
fn newest_mtime(path: &PathBuf) -> u64 {
    fn epoch_ms(meta: &fs::Metadata) -> u64 {
        meta.modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
    let Ok(meta) = fs::metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return epoch_ms(&meta);
    }
    fs::read_dir(path)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .map(|m| epoch_ms(&m))
        .max()
        .unwrap_or(0)
}

pub struct Hive {
    pub root: PathBuf,
}

impl Hive {
    /// Create (if missing) and return the hive rooted at `root`.
    pub fn ensure(root: &Path) -> Result<Self> {
        fs::create_dir_all(root.join("agents"))?;
        Self::seed(&root.join("PROTOCOL.md"), PROTOCOL_MD)?;
        Self::seed(&root.join("board.md"), BOARD_MD)?;
        Self::seed(&root.join("tasks.json"), TASKS_JSON)?;
        Self::seed(&root.join("registry.json"), "{\n  \"agents\": []\n}\n")?;
        Self::seed(&root.join("log.jsonl"), "")?;
        let hive = Self {
            root: root.to_path_buf(),
        };
        hive.commit("hive: init");
        Ok(hive)
    }

    fn seed(path: &Path, content: &str) -> Result<()> {
        if !path.exists() {
            if let Some(p) = path.parent() {
                fs::create_dir_all(p)?;
            }
            fs::write(path, content)?;
        }
        Ok(())
    }

    pub fn agent_dir(&self, id: &str) -> PathBuf {
        self.root.join("agents").join(id)
    }

    pub fn registry(&self) -> Vec<RegistryEntry> {
        let path = self.root.join("registry.json");
        let text = fs::read_to_string(&path).unwrap_or_default();
        serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("agents").cloned())
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }

    fn write_registry(&self, agents: &[RegistryEntry]) -> Result<()> {
        let v = serde_json::json!({ "agents": agents });
        self.atomic_write(
            &self.root.join("registry.json"),
            serde_json::to_string_pretty(&v)?.as_bytes(),
        )
    }

    pub fn god_id(&self) -> Option<String> {
        self.registry()
            .into_iter()
            .find(|a| a.is_god)
            .map(|a| a.id)
    }

    pub fn is_god(&self, id: &str) -> bool {
        self.god_id().is_some_and(|g| g == id)
    }

    /// Store the CLI's own session id (Claude `session_id` from every hook
    /// payload) on the registry entry — cheap write-only-on-change, used for
    /// --resume and cost dedup. Port of hive.recordSession.
    pub fn record_session(&self, id: &str, session_id: &str) -> Result<()> {
        let mut agents = self.registry();
        if let Some(a) = agents.iter_mut().find(|a| a.id == id) {
            if a.session_id.as_deref() != Some(session_id) {
                a.session_id = Some(session_id.into());
                self.write_registry(&agents)?;
            }
        }
        Ok(())
    }

    /// Newest coordination-file mtime for one agent (index.ts
    /// lastCoordinationAt): inbox + .done, outbox, memory.md — FILES only, so
    /// "no progress" means "not coordinating" even while tokens print.
    /// Returns epoch-ms, 0 when nothing exists.
    pub fn last_coordination_at(&self, agent_id: &str) -> u64 {
        let dir = self.agent_dir(agent_id);
        [
            dir.join("inbox"),
            dir.join("inbox").join(".done"),
            dir.join("outbox"),
            dir.join("memory.md"),
        ]
        .iter()
        .map(newest_mtime)
        .max()
        .unwrap_or(0)
    }

    /// Agent's working-dir "work" clock — git markers (.git/index, logs/HEAD,
    /// FETCH_HEAD, packed-refs) like index.ts lastWorkAt. None when the caller
    /// can't tell (no .git), so the breaker's work arm skips it.
    pub fn last_work_at(cwd: &str) -> Option<u64> {
        let g = Path::new(cwd).join(".git");
        if !g.exists() {
            return None;
        }
        let t = [
            g.join("index"),
            g.join("logs").join("HEAD"),
            g.join("FETCH_HEAD"),
            g.join("packed-refs"),
            g.join("refs").join("remotes"),
        ]
        .iter()
        .map(newest_mtime)
        .max()
        .unwrap_or(0);
        Some(t)
    }

    /// Cumulative usage for one agent, folded from cost.jsonl each beat —
    /// the same pull seam upstream's UsageProvider feeds the breaker.
    pub fn usage_sample(&self, agent_id: &str) -> Option<crate::breaker::UsageSample> {
        let text = fs::read_to_string(self.root.join("cost.jsonl")).ok()?;
        let mut s = crate::breaker::UsageSample::default();
        let mut seen = false;
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if v.get("agent").and_then(|a| a.as_str()) != Some(agent_id) {
                continue;
            }
            seen = true;
            s.input += v.get("input").and_then(|x| x.as_u64()).unwrap_or(0);
            s.output += v.get("output").and_then(|x| x.as_u64()).unwrap_or(0);
            s.cache_read += v.get("cache_read").and_then(|x| x.as_u64()).unwrap_or(0);
            s.cache_creation += v.get("cache_creation").and_then(|x| x.as_u64()).unwrap_or(0);
            if let Some(ts) = v.get("ts").and_then(|x| x.as_str()) {
                if let Ok(t) = chrono::DateTime::parse_from_rfc3339(ts) {
                    s.ts = t.timestamp_millis() as u64;
                }
            }
        }
        seen.then_some(s)
    }

    /// Is the floor quiet? (index.ts isFloorQuiet) — newest of: log.jsonl
    /// mtime (every routed msg/spawn touches it), each agent's mailbox mtimes,
    /// and live-PTY last_output clocks passed in `pty_clocks`. Never
    /// registry.status. Returns false when there's nothing to judge.
    pub fn is_floor_quiet(&self, threshold_ms: u64, pty_clocks: &[u64]) -> bool {
        let mut times: Vec<u64> = vec![newest_mtime(&self.root.join("log.jsonl"))];
        if let Ok(rd) = fs::read_dir(self.root.join("agents")) {
            for e in rd.flatten() {
                let d = e.path();
                times.push(newest_mtime(&d.join("inbox")));
                times.push(newest_mtime(&d.join("outbox")));
            }
        }
        times.extend_from_slice(pty_clocks);
        let Some(&latest) = times.iter().max() else {
            return false;
        };
        if latest == 0 {
            return false;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        now.saturating_sub(latest) > threshold_ms
    }

    /// Persist a proxy-bridge usage sample to the cost ledger — a separate
    /// append-only file so log.jsonl stays the event feed. Rows keep the
    /// upstream accounting schema (input/output/cache/usd estimate).
    pub fn append_cost_sample(&self, agent_id: &str, p: &serde_json::Value) -> Result<()> {
        use std::io::Write;
        let row = serde_json::json!({
            "agent": agent_id,
            "session": p.get("session_id"),
            "ts": Utc::now().to_rfc3339(),
            "input": p.get("input").and_then(|v| v.as_u64()).unwrap_or(0),
            "output": p.get("output").and_then(|v| v.as_u64()).unwrap_or(0),
            "cache_read": p.get("cache_read").and_then(|v| v.as_u64()).unwrap_or(0),
            "cache_creation": p.get("cache_creation").and_then(|v| v.as_u64()).unwrap_or(0),
            "model": p.get("model").and_then(|v| v.as_str()).unwrap_or(""),
        });
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("cost.jsonl"))?;
        writeln!(f, "{}", serde_json::to_string(&row)?)?;
        Ok(())
    }

    /// Port of hive.hookSettings — the per-session settings.json given to
    /// Claude via `--settings`. Every hook entry invokes the native hive-hook
    /// binary (quoted for paths with spaces); the event name arrives inside
    /// the stdin payload, so one command serves all events. `statusLine`
    /// forwards the session's context-window accounting via `--status`.
    ///
    /// Omitted vs upstream: mcpServers (catalog not ported) and the
    /// sandbox/permissions block (writableDirs — add when the sandbox story
    /// is designed for this backend).
    pub fn hook_settings(&self, shim_command: &str) -> serde_json::Value {
        let entry = |matcher: Option<&str>| {
            let mut e = serde_json::json!({
                "hooks": [{ "type": "command", "command": shim_command }]
            });
            if let Some(m) = matcher {
                e["matcher"] = serde_json::json!(m);
            }
            e
        };
        let plain = entry(None);
        let tool = entry(Some("*"));
        serde_json::json!({
            // 'auto', not a pinned theme — DEC 2031 notifications let the CLI
            // re-derive the palette when the harness theme flips.
            "theme": "auto",
            "statusLine": {
                "type": "command",
                "command": format!("{shim_command} --status"),
                "padding": 0
            },
            "hooks": {
                "Stop": [plain],
                "SubagentStop": [plain],
                "PreToolUse": [tool],
                "PostToolUse": [tool],
                "UserPromptSubmit": [plain],
                "Notification": [plain],
                "SessionStart": [plain],
                "PreCompact": [plain],
                "PostCompact": [plain]
            }
        })
    }

    /// Codex lifecycle-hook bridge — the codex counterpart of the claude
    /// settings block (upstream `installCodexHooks`, hive.ts:2193).
    ///
    /// Codex's hook contract is already Claude-shaped (snake_case stdin, same
    /// response fields; `Stop` honoring {decision:'block',reason} = "continue
    /// with reason as next prompt" — exactly our inbox drain), so the SAME
    /// hive-hook shim serves it verbatim. Rather than mutate the user's
    /// global ~/.codex (which also holds their login), each worker gets a
    /// per-agent CODEX_HOME with: auth.json linked in, their config.toml
    /// copied + our [[hooks.<Event>]] tables appended, and packages junctioned.
    /// Returns the CODEX_HOME path for the worker's env.
    pub fn install_codex_hooks(&self, dir: &Path, hook_bin: &Path) -> Result<PathBuf> {
        let home = dir.join(".codex");
        fs::create_dir_all(&home)?;
        let user_home = dirs::home_dir().unwrap_or_default().join(".codex");

        // auth.json — symlink where permitted, copy otherwise (login follows
        // the user). Idempotent.
        let (auth_src, auth_dst) = (user_home.join("auth.json"), home.join("auth.json"));
        if auth_src.exists() && !auth_dst.exists() {
            Self::link_or_copy(&auth_src, &auth_dst);
        }
        // packages — the managed app-server binaries; junction where possible.
        let (pk_src, pk_dst) = (user_home.join("packages"), home.join("packages"));
        if pk_src.exists() && !pk_dst.exists() {
            #[cfg(windows)]
            let _ = std::os::windows::fs::symlink_dir(&pk_src, &pk_dst);
            #[cfg(unix)]
            let _ = std::os::unix::fs::symlink(&pk_src, &pk_dst);
        }

        // config.toml — seed the user's (model/provider/trust carry over) and
        // append our hook group. `timeout` here is SECONDS — Codex floors a 0
        // to ONE second, which timed every shim call out under concurrent
        // spawns upstream (#v0.3.7 bug); 30s clears cold-start by two orders.
        let mut config = fs::read_to_string(user_home.join("config.toml")).unwrap_or_default();
        let command = hook_bin.display().to_string();
        config.push_str("\n# --- office-agent lifecycle hooks (auto-generated; do not edit) ---\n");
        for ev in [
            "PreToolUse",
            "PostToolUse",
            "Stop",
            "SubagentStop",
            "SessionStart",
            "UserPromptSubmit",
            "PreCompact",
            "PostCompact",
        ] {
            config.push_str(&format!(
                "\n[[hooks.{ev}]]\n[[hooks.{ev}.hooks]]\ntype = \"command\"\ncommand = {}\ntimeout = 30\n",
                serde_json::to_string(&command).unwrap_or_else(|_| "\"\"".into())
            ));
        }
        fs::write(home.join("config.toml"), config)?;
        Ok(home)
    }

    /// Hermes lifecycle bridge — per-agent HERMES_HOME isolation + a `hooks:`
    /// block in the worker's config.yaml, wired to the SAME hive-hook shim in
    /// `--hermes` translator mode (agent/shell_hooks.py's stdin contract is
    /// already `{hook_event_name, tool_name, tool_input, session_id, cwd}` —
    /// Claude-shaped; the shim's argv override maps each Hermes event name
    /// onto our canonical vocabulary and translates responses back to Hermes
    /// directives).
    ///
    /// Event map (Hermes → canonical): pre_tool_call→PreToolUse (HITL deny
    /// works — it's a blocking event), post_tool_call→PostToolUse, pre_verify
    /// + agent_loop_stopped→Stop (the turn-end drain seams), on_session_start
    /// →SessionStart, subagent_stop→SubagentStop, api_request_error→ApiError
    /// (breaker error-storm feed — Hermes has no OTel).
    ///
    /// The user's own ~/.hermes/config.yaml + .env are COPIED into the
    /// isolated home (provider creds + model prefs follow the worker; the
    /// user's home is never mutated). `hooks_auto_accept` is set inside the
    /// seeded config so the shell-hook consent prompt can't stall a worker.
    /// Returns the HERMES_HOME path for the worker's env.
    pub fn install_hermes_hooks(&self, dir: &Path, hook_bin: &Path) -> Result<PathBuf> {
        let home = dir.join(".hermes");
        fs::create_dir_all(&home)?;
        let user_home = dirs::home_dir().unwrap_or_default().join(".hermes");

        for name in ["config.yaml", ".env"] {
            let (src, dst) = (user_home.join(name), home.join(name));
            if src.exists() && !dst.exists() {
                let _ = fs::copy(&src, &dst);
            }
        }

        let cfg_path = home.join("config.yaml");
        let mut cfg = fs::read_to_string(&cfg_path).unwrap_or_default();
        if !cfg.lines().any(|l| l.starts_with("hooks_auto_accept:")) {
            cfg.push_str("\n# --- office-agent hive bridge (auto-generated) ---\nhooks_auto_accept: true\n");
        }
        // A pre-existing user `hooks:` block wins — merging dict-keyed YAML
        // blocks safely needs a real YAML writer; log-shaped skip instead.
        if !cfg.lines().any(|l| l.starts_with("hooks:")) {
            // Single-quoted YAML scalars: the command already carries double
            // quotes around the shim path and Windows backslashes — inside
            // single quotes neither needs escaping.
            let cmd = |ev: &str| format!("'\"{}\" --hermes {ev}'", hook_bin.display());
            cfg.push_str(&format!(
                "hooks:\n\
                \x20 pre_tool_call:\n\
                \x20   - command: {}\n\
                \x20     timeout: 30\n\
                \x20 post_tool_call:\n\
                \x20   - command: {}\n\
                \x20     timeout: 30\n\
                \x20 pre_verify:\n\
                \x20   - command: {}\n\
                \x20     timeout: 30\n\
                \x20 agent_loop_stopped:\n\
                \x20   - command: {}\n\
                \x20     timeout: 30\n\
                \x20 on_session_start:\n\
                \x20   - command: {}\n\
                \x20     timeout: 30\n\
                \x20 subagent_stop:\n\
                \x20   - command: {}\n\
                \x20     timeout: 30\n\
                \x20 api_request_error:\n\
                \x20   - command: {}\n\
                \x20     timeout: 30\n",
                cmd("PreToolUse"),
                cmd("PostToolUse"),
                cmd("Stop"),
                cmd("Stop"),
                cmd("SessionStart"),
                cmd("SubagentStop"),
                cmd("ApiError"),
            ));
        }
        fs::write(&cfg_path, cfg)?;
        Ok(home)
    }

    /// Gemini lifecycle bridge — per-agent `system-settings.json` under
    /// `<dir>/.gemini-hive`, bound by GEMINI_CLI_SYSTEM_SETTINGS_PATH
    /// (upstream installGeminiHooks, hive.ts:2140). Gemini's payload is
    /// already snake_case; the shim's `--gemini` mode maps each event onto
    /// our canonical name via argv and translates replies back to Gemini's
    /// {decision:"deny"} / {continue:false} shapes. Auth and user settings
    /// stay in the user's own GEMINI_CLI_HOME — only the trusted bridge is
    /// isolated per worker.
    pub fn install_gemini_hooks(&self, dir: &Path, hook_bin: &Path) -> Result<PathBuf> {
        let home = dir.join(".gemini-hive");
        fs::create_dir_all(&home)?;
        let cmd = |ev: &str| format!("\"{}\" --gemini {ev}", hook_bin.display());
        let entry = |name: &str, ev: &str, matcher: bool| {
            let mut e = serde_json::json!({
                "sequential": true,
                "hooks": [{
                    "name": format!("office-agent-{name}"),
                    "type": "command",
                    "command": cmd(ev),
                    "timeout": 30000
                }]
            });
            if matcher {
                e["matcher"] = serde_json::json!(".*");
            }
            e
        };
        let settings = serde_json::json!({
            "hooksConfig": { "enabled": true, "notifications": false },
            "hooks": {
                "SessionStart": [entry("session-start", "SessionStart", false)],
                "BeforeAgent":  [entry("before-agent",  "UserPromptSubmit", false)],
                "BeforeTool":   [entry("before-tool",   "PreToolUse", true)],
                "AfterTool":    [entry("after-tool",    "PostToolUse", true)],
                "AfterAgent":   [entry("after-agent",   "Stop", false)]
            }
        });
        let path = home.join("system-settings.json");
        fs::write(&path, serde_json::to_string_pretty(&settings)?)?;
        Ok(path)
    }

    /// Grok lifecycle bridge — global hooks in `~/.grok/hooks/office-agent.json`
    /// (upstream installGrokHooks, hive.ts:2515). Grok's global hook dir is
    /// trusted, sessions stay in the user's GROK_HOME, and every entry is a
    /// no-op outside hive workers because the shim exits when AGENT_ID/HIVE_SOCK
    /// are absent — the same runtime-scoping the upstream .cjs relied on.
    /// The shim's `--grok` mode normalizes grok's camelCase payload.
    pub fn install_grok_hooks(&self, hook_bin: &Path) -> Result<()> {
        let hook_dir = dirs::home_dir()
            .unwrap_or_default()
            .join(".grok")
            .join("hooks");
        fs::create_dir_all(&hook_dir)?;
        let cmd = format!("\"{}\" --grok", hook_bin.display());
        let tool = |matcher: bool| {
            let mut e = serde_json::json!({
                "hooks": [{ "type": "command", "command": cmd }]
            });
            if matcher {
                e["matcher"] = serde_json::json!(".*");
            }
            e
        };
        let hooks = serde_json::json!({
            "hooks": {
                "PreToolUse": [tool(true)],
                "PostToolUse": [tool(true)],
                "Stop": [tool(false)],
                "SubagentStop": [tool(true)],
                "SessionStart": [tool(true)],
                "UserPromptSubmit": [tool(false)],
                "PreCompact": [tool(true)],
                "PostCompact": [tool(true)]
            }
        });
        fs::write(
            hook_dir.join("office-agent.json"),
            serde_json::to_string_pretty(&hooks)?,
        )?;
        Ok(())
    }

    /// Antigravity (`agy`) bridge — merges an `office-agent` group into BOTH
    /// global hooks.json surfaces (antigravity-cli#49: agy LOADS from
    /// ~/.gemini/antigravity-cli/hooks.json but TRIGGERS from
    /// ~/.gemini/config/hooks.json — upstream writes both, so do we).
    /// Runtime-scoped by AGENT_ID: the shim exits for non-hive agy sessions,
    /// so the global config never disturbs the user's own usage. Only OUR
    /// group key is overwritten; other groups are preserved verbatim.
    pub fn install_agy_hooks(&self, hook_bin: &Path) -> Result<()> {
        let cmd = |ev: &str| format!("\"{}\" --agy {ev}", hook_bin.display());
        let tool = |ev: &str| {
            serde_json::json!({
                "matcher": "*",
                "hooks": [{ "type": "command", "command": cmd(ev), "timeout": 0 }]
            })
        };
        let plain = |ev: &str| {
            serde_json::json!({
                "hooks": [{ "type": "command", "command": cmd(ev), "timeout": 0 }]
            })
        };
        let group = serde_json::json!({
            "PreToolUse": [tool("PreToolUse")],
            "PostToolUse": [tool("PostToolUse")],
            "PreInvocation": [plain("PreInvocation")],
            "PostInvocation": [plain("PostInvocation")],
            "Stop": [plain("Stop")]
        });
        let gem = dirs::home_dir().unwrap_or_default().join(".gemini");
        for p in [
            gem.join("config").join("hooks.json"),
            gem.join("antigravity-cli").join("hooks.json"),
        ] {
            if let Some(parent) = p.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let mut existing: serde_json::Value = fs::read_to_string(&p)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(serde_json::json!({}));
            existing["office-agent"] = group.clone();
            fs::write(&p, serde_json::to_string_pretty(&existing)?)?;
        }
        Ok(())
    }

    fn link_or_copy(src: &Path, dst: &Path) {
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(src, dst).is_ok();
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(src, dst).is_ok();
        if !linked {
            let _ = fs::copy(src, dst);
        }
    }

    /// The system-prompt prefix injected into every spawn. Port of
    /// hive.injectedPrompt — interpolates only lifetime-stable values so the
    /// prompt-cache invariant holds (no dates/UUIDs/volatile state).
    pub fn injected_prompt(&self, entry: &RegistryEntry) -> String {
        let dir = self.agent_dir(&entry.id);
        let god_line = if entry.is_god {
            " You are the ORCHESTRATOR: read every routed request, resolve routine ones yourself, keep the floor autonomous, escalate only spend/destructive/scope items to the human via messages to \"human\"."
        } else {
            ""
        };
        format!(
            "You are {name} (id `{id}`), an agent inside an office hive. \
             Your folder is {dir}; the hive root is {root}. Coordination is file-based: \
             read {dir}/identity.md and {dir}/memory.md at start, check {dir}/inbox/ for \
             messages, drop reply files into {dir}/outbox/ — the router delivers them. \
             The full contract is in {root}/PROTOCOL.md.{god} \
             Env vars available to you: AGENT_ID, AGENT_NAME, HIVE_ROOT, AGENT_DIR.",
            name = entry.name,
            id = entry.id,
            dir = dir.display(),
            root = self.root.display(),
            god = god_line
        )
    }

    /// Create the agent's hive folder + identity/memory files and record it in
    /// the registry. Mirrors hive.ts `ensureAgent`'s file side (spawn injection
    /// stays in commands.rs).
    pub fn register_agent(&self, entry: &RegistryEntry) -> Result<()> {
        let dir = self.agent_dir(&entry.id);
        for sub in ["inbox", "inbox/.done", "outbox"] {
            fs::create_dir_all(dir.join(sub))?;
        }
        Self::seed(&dir.join("cursor.json"), "{\n  \"lastProcessed\": null\n}\n")?;
        Self::seed(
            &dir.join("identity.md"),
            &format!(
                "# {}\n\n- id: `{}`\n- provider: `{}`\n- role: {}\n- cwd: `{}`\n",
                entry.name, entry.id, entry.provider, entry.role, entry.cwd
            ),
        )?;
        Self::seed(
            &dir.join("memory.md"),
            &format!("# Memory — {}\n\n(append what you learn; this file survives restarts)\n", entry.name),
        )?;

        let mut agents = self.registry();
        if !agents.iter().any(|a| a.id == entry.id) {
            agents.push(entry.clone());
            self.write_registry(&agents)?;
        }
        self.log(&serde_json::json!({
            "event": "register", "agent": entry.id, "provider": entry.provider
        }))?;
        self.commit(&format!("hive: register {}", entry.id));
        Ok(())
    }

    pub fn set_status(&self, id: &str, status: &str) -> Result<()> {
        let mut agents = self.registry();
        if let Some(a) = agents.iter_mut().find(|a| a.id == id) {
            a.status = status.into();
            self.write_registry(&agents)?;
        }
        Ok(())
    }

    /// Atomic temp-file + rename write — the contract that lets the router
    /// never see a half-written message (HIVE.md §3 design rule).
    fn atomic_write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Write a message into `sender`'s outbox (the only path agents/humans use
    /// to send — the router does the rest).
    pub fn send(&self, msg: &Message) -> Result<()> {
        let out = self.agent_dir(&msg.from).join("outbox");
        fs::create_dir_all(&out)?;
        self.atomic_write(
            &out.join(format!("{}.json", msg.id)),
            serde_json::to_string_pretty(msg)?.as_bytes(),
        )
    }

    /// A virtual sender ('breaker', 'hive') has no outbox on disk — its
    /// message is delivered to the resolved inboxes immediately, same target
    /// resolution the router applies.
    pub fn send_virtual(
        &self,
        from: &str,
        to: &str,
        act: &str,
        subject: &str,
        body: &str,
    ) -> Result<Vec<Delivery>> {
        let msg = Message::new(from, to, act, subject, body);
        let delivered = self.deliver(&msg, &serde_json::to_string_pretty(&msg)?)?;
        if !delivered.is_empty() {
            self.commit(&format!("hive: deliver {}", msg.id));
        }
        Ok(delivered)
    }

    /// Resolve targets and write the message into recipient inboxes — the
    /// inner loop the outbox router and send_virtual share.
    fn deliver(&self, msg: &Message, raw: &str) -> Result<Vec<Delivery>> {
        let known: HashSet<String> = self.registry().iter().map(|a| a.id.clone()).collect();
        let god = self.god_id();
        let mut out = Vec::new();
        for (target_id, bounced) in self.resolve_targets(msg, &known, &god, &msg.from) {
            let inbox = self.agent_dir(&target_id).join("inbox");
            let _ = fs::create_dir_all(&inbox);
            let dest = inbox.join(format!("{}.json", msg.id));
            let payload = if bounced {
                let mut m = msg.clone();
                m.subject = format!("[bounced] {}", m.subject);
                m.needs_human = true;
                serde_json::to_string_pretty(&m)?
            } else {
                raw.to_string()
            };
            self.atomic_write(&dest, payload.as_bytes())?;
            out.push(Delivery {
                from: msg.from.clone(),
                to: target_id,
                msg_id: msg.id.clone(),
                bounced,
            });
        }
        if !out.is_empty() {
            self.log(&serde_json::json!({
                "event": "deliver", "from": msg.from, "to": msg.to,
                "msg_id": msg.id, "act": msg.act
            }))?;
        }
        Ok(out)
    }

    /// One router pass: drain every outbox into recipient inboxes.
    /// Returns the deliveries made (for frontend events).
    pub fn router_tick(&self) -> Result<Vec<Delivery>> {
        let mut delivered = Vec::new();
        let agents_dir = self.root.join("agents");

        let senders: Vec<PathBuf> = match fs::read_dir(&agents_dir) {
            Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
            Err(_) => return Ok(delivered),
        };

        for sender_dir in senders {
            let outbox = sender_dir.join("outbox");
            let Ok(rd) = fs::read_dir(&outbox) else { continue };
            let mut files: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .collect();
            files.sort();

            for file in files {
                let Ok(text) = fs::read_to_string(&file) else { continue };
                let Ok(msg) = serde_json::from_str::<Message>(&text) else {
                    // malformed: quarantine into sender's inbox/.done so the
                    // router can't wedge on it (same policy as the tests).
                    let done = sender_dir.join("inbox").join(".done");
                    let _ = fs::create_dir_all(&done);
                    if let Some(name) = file.file_name() {
                        let _ = fs::rename(&file, done.join(name));
                    }
                    continue;
                };
                delivered.extend(self.deliver(&msg, &text)?);
                let _ = fs::remove_file(&file);
            }
        }

        if !delivered.is_empty() {
            self.commit("hive: route");
        }
        Ok(delivered)
    }

    /// `to` resolution (HIVE.md §5): broadcast → everyone but sender;
    /// god/human/unknown → the god agent; agent.<id> | <id> → that inbox.
    /// When there is no god and the target is unknown, bounce back to sender.
    fn resolve_targets(
        &self,
        msg: &Message,
        known: &HashSet<String>,
        god: &Option<String>,
        sender_id: &str,
    ) -> Vec<(String, bool)> {
        let to = msg.to.trim();
        if to == "broadcast" {
            return known
                .iter()
                .filter(|id| id.as_str() != sender_id)
                .map(|id| (id.clone(), false))
                .collect();
        }
        if to == "god" || to == "human" {
            return match god {
                Some(g) => vec![(g.clone(), false)],
                None => vec![(sender_id.to_string(), true)],
            };
        }
        let id = to.strip_prefix("agent.").unwrap_or(to);
        if known.contains(id) {
            vec![(id.to_string(), false)]
        } else if let Some(g) = god {
            vec![(g.clone(), true)]
        } else {
            vec![(sender_id.to_string(), true)]
        }
    }

    pub fn inbox(&self, id: &str) -> Result<Vec<Message>> {
        let inbox = self.agent_dir(id).join("inbox");
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir(&inbox) else { return Ok(out) };
        let mut files: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        files.sort();
        for f in files {
            if let Ok(t) = fs::read_to_string(&f) {
                if let Ok(m) = serde_json::from_str::<Message>(&t) {
                    out.push(m);
                }
            }
        }
        Ok(out)
    }

    /// Move a processed message into inbox/.done (audit trail, never deleted).
    pub fn mark_done(&self, agent_id: &str, msg_id: &str) -> Result<()> {
        let dir = self.agent_dir(agent_id);
        let src = dir.join("inbox").join(format!("{msg_id}.json"));
        let done = dir.join("inbox").join(".done");
        fs::create_dir_all(&done)?;
        fs::rename(&src, done.join(format!("{msg_id}.json")))?;
        self.log(&serde_json::json!({
            "event": "done", "agent": agent_id, "msg_id": msg_id
        }))?;
        Ok(())
    }

    pub fn read_board(&self) -> Result<String> {
        Ok(fs::read_to_string(self.root.join("board.md")).unwrap_or_default())
    }

    pub fn read_tasks(&self) -> Result<serde_json::Value> {
        let t = fs::read_to_string(self.root.join("tasks.json")).unwrap_or_default();
        Ok(serde_json::from_str(&t).unwrap_or(serde_json::json!({"tasks": []})))
    }

    pub fn read_memory(&self, agent_id: &str) -> Result<String> {
        Ok(fs::read_to_string(self.agent_dir(agent_id).join("memory.md"))
            .unwrap_or_default())
    }

    pub fn append_memory(&self, agent_id: &str, entry: &str) -> Result<()> {
        use std::io::Write;
        let path = self.agent_dir(agent_id).join("memory.md");
        let mut f = fs::OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(f, "\n{entry}")?;
        self.log(&serde_json::json!({
            "event": "memory_append", "agent": agent_id
        }))?;
        Ok(())
    }

    /// Append an event line to log.jsonl — pub so the webhook gateway can
    /// record inbound traffic on the same ledger every routed message uses.
    pub fn log(&self, event: &serde_json::Value) -> Result<()> {
        use std::io::Write;
        let mut v = serde_json::json!({ "ts": Utc::now().to_rfc3339() });
        if let (Some(a), Some(b)) = (v.as_object_mut(), event.as_object()) {
            a.extend(b.clone());
        }
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("log.jsonl"))?;
        writeln!(f, "{}", serde_json::to_string(&v)?)?;
        Ok(())
    }

    /// Single-committer git (HIVE.md §2.1): only this process ever commits;
    /// agents write plain files. Best-effort — a failure to init/commit must
    /// never break routing (matches hive.ts's swallow-and-continue policy).
    pub fn commit(&self, message: &str) {
        let _ = self.try_commit(message);
    }

    fn try_commit(&self, message: &str) -> Result<()> {
        let repo = match git2::Repository::open(&self.root) {
            Ok(r) => r,
            Err(_) => git2::Repository::init(&self.root).context("git init")?,
        };
        let mut index = repo.index()?;
        index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
        index.write()?;
        let tree_id = index.write_tree()?;
        let tree = repo.find_tree(tree_id)?;
        let sig = git2::Signature::now("hive", "hive@local")?;
        match repo.head() {
            Ok(head) => {
                let parent = head.peel_to_commit()?;
                if parent.tree_id() == tree_id {
                    return Ok(()); // nothing changed
                }
                repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])?;
            }
            Err(_) => {
                repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[])?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_hooks_write_config_with_all_events() {
        let dir = std::env::temp_dir().join(format!("oa-codex-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let hive = Hive::ensure(&dir.join("hive")).unwrap();
        let hook_bin = dir.join("hive-hook.exe");
        let home = hive.install_codex_hooks(&dir.join("agent"), &hook_bin).unwrap();
        let cfg = std::fs::read_to_string(home.join("config.toml")).unwrap();
        for ev in ["PreToolUse", "PostToolUse", "Stop", "SubagentStop",
                   "SessionStart", "UserPromptSubmit", "PreCompact", "PostCompact"] {
            assert!(cfg.contains(&format!("[[hooks.{ev}]]")), "missing {ev}");
        }
        assert!(cfg.contains("timeout = 30"), "codex floors timeout 0 to 1s - never emit it");
        assert!(cfg.contains("hive-hook.exe"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hermes_hooks_write_config_with_event_map() {
        let dir = std::env::temp_dir().join(format!("oa-hermes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let hive = Hive::ensure(&dir.join("hive")).unwrap();
        let hook_bin = dir.join("hive-hook.exe");
        let home = hive.install_hermes_hooks(&dir.join("agent"), &hook_bin).unwrap();
        let cfg = std::fs::read_to_string(home.join("config.yaml")).unwrap();
        assert!(cfg.contains("hooks_auto_accept: true"));
        for ev in ["pre_tool_call", "post_tool_call", "pre_verify",
                   "agent_loop_stopped", "on_session_start", "subagent_stop",
                   "api_request_error"] {
            assert!(cfg.contains(&format!(" {ev}:")), "missing {ev}");
        }
        // argv override carries the canonical event; the shim's --hermes
        // flag selects the response translator.
        assert!(cfg.contains("--hermes PreToolUse"));
        assert!(cfg.contains("--hermes Stop"));
        assert!(cfg.contains("--hermes ApiError"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gemini_hooks_write_system_settings_with_event_map() {
        let dir = std::env::temp_dir().join(format!("oa-gemini-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let hive = Hive::ensure(&dir.join("hive")).unwrap();
        let hook_bin = dir.join("hive-hook.exe");
        let path = hive.install_gemini_hooks(&dir.join("agent"), &hook_bin).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["hooksConfig"]["enabled"], serde_json::json!(true));
        for (ge, ce) in [
            ("SessionStart", "SessionStart"),
            ("BeforeAgent", "UserPromptSubmit"),
            ("BeforeTool", "PreToolUse"),
            ("AfterTool", "PostToolUse"),
            ("AfterAgent", "Stop"),
        ] {
            let cmd = v["hooks"][ge][0]["hooks"][0]["command"].as_str().unwrap_or("");
            assert!(cmd.contains(&format!("--gemini {ce}")), "{ge} → {ce}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
