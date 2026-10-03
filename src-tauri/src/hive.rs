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

    /// One router pass: drain every outbox into recipient inboxes.
    /// Returns the deliveries made (for frontend events).
    pub fn router_tick(&self) -> Result<Vec<Delivery>> {
        let known: HashSet<String> = self.registry().iter().map(|a| a.id.clone()).collect();
        let god = self.god_id();
        let mut delivered = Vec::new();
        let agents_dir = self.root.join("agents");

        let senders: Vec<PathBuf> = match fs::read_dir(&agents_dir) {
            Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
            Err(_) => return Ok(delivered),
        };

        for sender_dir in senders {
            let Some(sender_id) = sender_dir.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
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

                let targets = self.resolve_targets(&msg, &known, &god, sender_id);
                for (target_id, bounced) in targets {
                    let inbox = self.agent_dir(&target_id).join("inbox");
                    let _ = fs::create_dir_all(&inbox);
                    let dest = inbox.join(format!("{}.json", msg.id));
                    let payload = if bounced {
                        let mut m = msg.clone();
                        m.subject = format!("[bounced] {}", m.subject);
                        m.needs_human = true;
                        serde_json::to_string_pretty(&m)?
                    } else {
                        text.clone()
                    };
                    self.atomic_write(&dest, payload.as_bytes())?;
                    delivered.push(Delivery {
                        from: msg.from.clone(),
                        to: target_id,
                        msg_id: msg.id.clone(),
                        bounced,
                    });
                }
                let _ = fs::remove_file(&file);
                self.log(&serde_json::json!({
                    "event": "deliver", "from": msg.from, "to": msg.to,
                    "msg_id": msg.id, "act": msg.act
                }))?;
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

    fn log(&self, event: &serde_json::Value) -> Result<()> {
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
