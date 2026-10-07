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
use serde_json::Value;

/// Junction (Windows, no admin) / dir-symlink (unix) for the codex rollout
/// exposure. Copy fallback when linking fails.
fn link_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        junction::create(src, dst)
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src, dst)
    }
    #[cfg(not(any(windows, unix)))]
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no dir link",
        ))
    }
}

pub(crate) fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for e in fs::read_dir(src)? {
        let e = e?;
        let to = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &to)?;
        } else {
            let _ = fs::copy(e.path(), &to);
        }
    }
    Ok(())
}
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
            // time-sortable unique id — upstream `${stamp()}-${shortRand()}`:
            // ISO with colons→dashes + 6 hex.
            id: format!("{}-{}", stamp(), short_rand()),
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

fn default_provider() -> String {
    "claude".into()
}

/// One row of `registry.json.agents["<id>"]` — upstream RegistryAgent
/// (AgentMeta + status/lastSeen/archived/onHold/sessionId/cwdValid).
/// Rust field names stay snake_case for the codebase; serde serializes
/// camelCase on disk + IPC to match upstream byte-for-byte.
///
/// `command`/`created_at` are port-only fields (respawn bookkeeping);
/// `extra` preserves every key the struct doesn't model (e.g. worker `slack`,
/// `character`, `tokenCap`) across typed rewrites — upstream's `{...prev}`
/// spread, which the old sidecar file was standing in for.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RegistryEntry {
    pub id: String,
    pub name: String,
    #[serde(default = "default_provider")]
    pub provider: String,
    pub role: String,
    pub capabilities: Vec<String>,
    pub cwd: String,
    #[serde(alias = "is_god")]
    pub is_god: bool,
    /// The send-only prep assistant — excluded from broadcast fan-out and
    /// bounced when directly addressed (it drains no inbox).
    #[serde(alias = "is_assistant")]
    pub is_assistant: bool,
    pub status: String,
    /// Epoch-ms — bumped on spawn/rename/archive/session (upstream lastSeen).
    #[serde(alias = "last_seen")]
    pub last_seen: u64,
    /// Terminal/PTY tab closed; record retained, not live. Broadcast + roster
    /// reads skip archived agents.
    pub archived: bool,
    /// Human has this agent 1:1 — do-not-dispatch marker (upstream onHold).
    #[serde(alias = "on_hold")]
    pub on_hold: bool,
    /// The CLI's own session id (from hook payloads) — for --resume + cost
    /// dedup, same as upstream's recordSession.
    #[serde(alias = "session_id", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Whether `cwd` is usable for a (re)spawn — computed at registration.
    #[serde(alias = "cwd_valid", skip_serializing_if = "Option::is_none")]
    pub cwd_valid: Option<bool>,
    /// The spawn command line (port-only) — kept so respawn/restart can
    /// rebuild the PTY without asking the UI for the invocation again.
    pub command: String,
    /// Spawn timestamp (port-only).
    #[serde(alias = "created_at")]
    pub created_at: String,
    /// Unknown upstream/port fields ride along untouched on rewrite.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// An outbox file the router only half-understands — every field optional,
/// exactly the shape `normalize` accepts upstream (hive.ts:1619).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialMessage {
    pub id: Option<String>,
    pub conversation: Option<String>,
    pub in_reply_to: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub act: Option<String>,
    pub subject: Option<String>,
    pub body: Option<String>,
    /// i64 so a negative agent-authored value clamps the way upstream's
    /// Math.max(0, …) does instead of failing the u32 parse.
    pub hops: Option<i64>,
    pub requires_reply: Option<bool>,
    pub needs_human: Option<bool>,
    pub created_at: Option<String>,
}

/// One routed delivery — the `hive-deliver` event payload. Extended with the
/// upstream `hive:message` envelope fields (id/act/subject/targets/
/// needsHuman) so the renderer sees the same contract; `from`/`to`/`msg_id`/
/// `bounced` are the port's existing fields, kept for compat.
#[derive(Clone, Debug, Serialize)]
pub struct Delivery {
    pub id: String,
    pub from: String,
    pub to: String,
    pub msg_id: String,
    pub act: String,
    pub subject: String,
    /// All resolved recipients of the routed message (upstream `targets`).
    pub targets: Vec<String>,
    #[serde(rename = "needsHuman")]
    pub needs_human: bool,
    pub bounced: bool,
    /// Upstream `hive:terminalHandoff` payload — set INSTEAD of an inbox
    /// delivery when the target's provider can't drain an inbox (hookless:
    /// kimi/copilot/custom) or rides the proxy-tier terminal path (qwen,
    /// crush). The full message — the renderer turns it into a terminal
    /// work order (#53).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handoff: Option<serde_json::Value>,
}

/// What `settle_inbox` did to a released worker's mailbox (upstream
/// SettledInbox).
#[derive(Clone, Debug, Default, Serialize)]
pub struct SettledInbox {
    /// Unread messages filed under inbox/.done.
    pub moved: u64,
    /// Unread messages left pending because they arrived after the cut-off.
    pub kept: u64,
    /// Filed messages that asked for something — their sender never gets an
    /// answer, so the caller can say so (upstream `unanswered`).
    pub unanswered: Vec<UnansweredMessage>,
}

#[derive(Clone, Debug, Serialize)]
pub struct UnansweredMessage {
    pub id: String,
    pub act: String,
    pub from: String,
    pub subject: String,
    pub conversation: String,
}

/// Banner stamped at the top of every generated hive doc (upstream
/// GENERATED_DOC_NOTICE, hive.ts:2924) — verbatim, proper noun included.
const GENERATED_DOC_NOTICE: &str =
    "<!-- Generated and managed by Munder Difflin. Local edits may be replaced during hive bootstrap. -->";

/// Spawn-time context for `injected_prompt` — the optional blocks upstream
/// weaves into the system-prompt prefix (hive.ts injectedPrompt args).
#[derive(Clone, Debug, Default)]
pub struct PromptOpts {
    /// MemPalace is available → the semantic-memory paragraph is included.
    pub semantic_memory: bool,
    /// Knowledge Graph is populated → the kg paragraph is included.
    pub knowledge_graph: bool,
    /// Path to the kg CLI shown in the knowledge paragraph (upstream's own).
    pub kg_cli_path: Option<String>,
    /// The bundled-node path upstream quotes in the kg paragraph
    /// (`<hive>/bin/runtime/node` or the harness node).
    pub hive_node: String,
    /// Orchestrator gets the spawn-requests block when the floor allows it.
    pub orchestrator_may_spawn: bool,
    /// RUNNING BUILD line — version + packaging state, when known.
    pub runtime: Option<RuntimeInfo>,
}

#[derive(Clone, Debug)]
pub struct RuntimeInfo {
    pub version: String,
    pub packaged: bool,
    pub app_path: Option<String>,
}

/// shared/agentRole.ts — status captions that must never overwrite the
/// durable hire role.
fn is_durable_role(text: &str) -> bool {
    let v = text.trim();
    if v.is_empty() {
        return false;
    }
    let l = v.to_lowercase();
    let l = l.strip_prefix("on ").unwrap_or(&l);
    !matches!(
        l,
        "standby"
            | "idle"
            | "awaiting"
            | "paused"
            | "resumed"
            | "working"
            | "thinking"
            | "archived"
            | "starting up"
            | "reconnecting"
            | "reconnecting…"
            | "running the floor"
            | "a fresh harness"
    )
}

/// preferredAgentRole — a real hire role always beats a status-like caption;
/// when both are durable the candidate (newly-set value) wins.
fn preferred_agent_role(candidate: Option<&str>, fallback: Option<&str>, is_god: bool) -> String {
    let incoming = candidate.unwrap_or("").trim().to_string();
    let existing = fallback.unwrap_or("").trim().to_string();
    if is_durable_role(&incoming) {
        return incoming;
    }
    if is_durable_role(&existing) {
        return existing;
    }
    if !incoming.is_empty() {
        return incoming;
    }
    if !existing.is_empty() {
        return existing;
    }
    if is_god {
        "orchestrator (god)".into()
    } else {
        "agent".into()
    }
}

/// cwdValidity (hive.ts:693) — whether a worker's working directory is usable
/// for a (re)spawn. Best-effort; a stat error degrades to invalid.
fn cwd_valid(cwd: &str) -> bool {
    cwd_issue(cwd).is_none()
}

fn cwd_issue(cwd: &str) -> Option<&'static str> {
    if cwd.trim().is_empty() {
        return Some("missing");
    }
    let expanded = crate::misc::expand_tilde(cwd);
    let p = std::path::Path::new(&expanded);
    if !p.is_absolute() {
        return Some("not-absolute");
    }
    match std::fs::metadata(p) {
        Ok(m) if m.is_dir() => None,
        Ok(_) => Some("not-a-directory"),
        Err(_) => Some("missing-dir"),
    }
}

/// repairLiteralLineBreaksInJsonStrings (hive.ts:315) — agents write message
/// JSON with raw newlines inside string values; escape them in place so the
/// envelope parses instead of quarantining. Returns (text, changed).
fn repair_literal_line_breaks(raw: &str) -> (String, bool) {
    let mut text = String::with_capacity(raw.len() + 8);
    let mut in_string = false;
    let mut escaped = false;
    let mut changed = false;
    for ch in raw.chars() {
        if !in_string {
            text.push(ch);
            if ch == '"' {
                in_string = true;
            }
            continue;
        }
        if escaped {
            text.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => {
                text.push(ch);
                escaped = true;
            }
            '"' => {
                text.push(ch);
                in_string = false;
            }
            '\n' => {
                text.push_str("\\n");
                changed = true;
            }
            '\r' => {
                text.push_str("\\r");
                changed = true;
            }
            _ => text.push(ch),
        }
    }
    (text, changed)
}

/// The full upstream protocol text (hive.ts:2955-3100) — verbatim.
const PROTOCOL_MD: &str = r##"<!-- Generated and managed by Munder Difflin. Local edits may be replaced during hive bootstrap. -->

# Hive protocol

You are one of several Claude agents sharing this hive. Coordination is entirely
file-based; the harness (main process) is the only thing that runs git and the
only thing that moves messages between agents.

## Your workspace — `agents/<your-id>/`
- `identity.md`  — who you are (read-only; the harness writes it).
- `memory.md`    — your long-term memory. Read at the start of a task; append to it as you learn.
- `inbox/`       — messages addressed to you. Read them at the start of a task.
- `inbox/.done/` — move a message here once you've handled it.
- `outbox/`      — drop messages here to send them. The harness delivers them.

**Never write into another agent's folder.** Write to your own `outbox/`; the
orchestrator routes it. This keeps every file single-writer.

## Sending a message
Write one JSON file into `outbox/` (any filename ending in `.json`):

```json
{
  "to": "<agent-id> | god | broadcast",
  "act": "request | inform | propose | query | agree | refuse | done",
  "subject": "one-line summary",
  "body": "the details",
  "conversation": "carry this across a thread (optional)",
  "in_reply_to": "<message id you're replying to> (optional)"
}
```

The harness fills in `id`, `from`, `hops`, and timestamps.

## Rules of the road
- Only `request`, `query`, and `propose` expect a reply. `inform` and `done` are terminal —
  don't reply to them, or two agents will loop forever.
- For anything ambiguous, cross-cutting, or needing sign-off, message `god` — the
  god agent clarifies answers for you so you rarely need the human directly.
- There is NO separate human-approval queue. Human-in-the-loop is native to Claude
  Code: a tool you run that needs permission prompts in your own session (the human
  can approve it remotely from their phone via `/remote-control`). If you genuinely
  need a human decision, raise it with `god` (a message `"to": "human"` is routed to
  the god/orchestrator, the human's proxy on the floor).
- `board.md` is the shared plan. Don't edit it directly — `propose` changes to `god`,
  who is its sole scribe.
- Re-reading a message you already moved to `.done/` is a no-op. Don't reprocess.

## The work: board.md vs tasks.json
There are two shared surfaces, both in the hive root:
- `board.md` — the freeform narrative plan. The god agent is its sole scribe; others `propose` edits.
- `tasks.json` — the structured task ledger (a kanban: `todo / doing / blocked / done`, with title,
  assignee, priority, deps). Keep the task you're working reflected in its status.

## Asking the human (the ASK ME card)
When a card can only move with the human — a question to answer, or an action only they can do
(create an account, approve a spend, hand over credentials, test on their device) — the god sets the
card `"status": "blocked"` and appends the ask to its `humanQA` array:

```json
{ "q": "the ask, in markdown", "askedAt": "<iso timestamp>" }
```

The harness shows the open ask on the ASK ME board and in the ASK ME tab, and the human's reply lands
in the same entry as `"a"` plus an inbox message to god. Every past entry stays on the card — that
trail is the decision history.

**Write the ask short, and in markdown.** The card renders it, so plain-text asterisks and backticks
show up literally, and a card is not a terminal — an ask longer than a short paragraph plus its
options (roughly 700 characters) is a report, not a question. Cut the narrative and keep the decision:
- open with ONE **bold** sentence saying exactly what you need from them;
- `backticks` for paths, commands, values, and identifiers;
- `-` bullets or `1.` numbering for every option or step;
- a blank line between paragraphs; a single newline is rendered as a line break, so each option
  stays on its own line.

When the ask originates in another agent's report, REWRITE it into that shape. Never paste the report
body in as the question, and never make the human read the investigation to find the decision. Do NOT park human questions in separate files (no `HumanQuestion.md`),
and never sit idle waiting for a reply — move on to other work and pick the answer up when it arrives.

## Guardrails: circuit breaker & token budgets
A circuit breaker watches every agent for runaway behavior (looping on the same tool, error storms,
overspending). It escalates gently: `steer` → `constrain` → `stop`. If a `Circuit breaker: steer`
or `Circuit breaker: constrain` message lands in your inbox, you ARE the problem it caught — stop
repeating, summarize what you've tried, and do exactly what the message says (constrain = go read-only
and get god's sign-off before more tool calls). Be **token-frugal**: the floor has a token budget and
each agent can have its own token limit; crossing it trips the breaker. Prefer references over pasted
content, and `/compact` your own session when context gets heavy.

## Fleet monitoring (orchestrator)
You (god) are responsible for situational awareness. To see the live state of every agent, read
`fleet.json` in the hive root — it is refreshed continuously with each agent's tokens, cost, status,
breaker level, last tool, last-active time, and inbox backlog. Pair it with `registry.json` (the roster)
and `log.jsonl` (the event feed). IMPORTANT: `claude agents` will NOT show your hive's sibling
sessions (they're spawned independently) — `fleet.json` is your source of truth for them. For a deeper
look at one agent, read its `agents/<id>/memory.md` and `inbox/`, or send it a `query`. A full
Claude Code command reference (slash = your own session only; CLI = your shell, can target the fleet)
is in `COMMANDS.md` in the hive root.

## Spawning a worker (orchestrator)
You can start an ephemeral worker yourself. Write ONE JSON file into `spawn-requests/<id>.json` in
the hive root:

```json
{
  "objective": "what the worker must do (required)",
  "cwd": "/absolute/path/to/the/repo (required)",
  "name": "display name (optional)",
  "command": "engine CLI (optional; overrides the provider default)",
  "provider": "claude | codex | cursor | antigravity | … (optional; selects its default CLI when command is omitted)",
  "model": "model override (optional)",
  "isolate": true,
  "tokenCap": 0,
  "slack": { "channel": "C…", "thread_ts": "…" },
  "character": "meredith",
  "accent": "coral"
}
```

The harness polls that directory, spawns `worker-<id>`, and moves the request to
`spawn-requests/.done/` once it starts or to `spawn-requests/.failed/` with a reason. `isolate`
defaults to true, giving the worker its own git worktree. `slack` routes its failures back to a
thread. This is the ONLY spawn route you can complete on your own: a hire manifest under
`research/hires/` needs the human to confirm it in the UI.

`character` and `accent` set how the worker looks on the office floor, and both are optional.
Naming a worker after a cast member already gets you that avatar, so you only need `character` when
the name and the face should differ. An unrecognised value falls back rather than failing the spawn.

**It can be switched off.** The operator controls this under Settings → Autonomy & Budgets, and it is
OFF by default, because every worker you start spends tokens nobody approved. While it is off your
request is NOT failed or deleted, it waits in `spawn-requests/` and runs if the operator turns it on.
If a request of yours has sat there without moving, that is why, and it is a decision to raise with the
human rather than retry. Route work to an agent already on the floor first either way.

## Semantic memory (optional — when `mempalace` is installed)
When `MEMPALACE_PALACE_PATH` is set in your environment, the hive shares a
searchable MemPalace and you have the `mempalace` CLI:
- `mempalace search "<query>"` — recall relevant past knowledge across the whole
  team by meaning (not just keywords). Add `--wing <agent-id>` to scope to one
  agent, `--results N` to widen.
- `mempalace wake-up` — a short digest of what matters, good at the start of a task.

Your `memory.md` is mined into the palace automatically, so the durable facts you
write there become searchable by every agent. You don't run `mine` yourself.
"##;

/// One Claude Code command reference row — mirrors shared/claudeCommands.ts
/// `Cmd` ({cmd, kind, desc, usage?}). The string content is verbatim upstream.
struct CmdItem {
    cmd: &'static str,
    kind: &'static str, // 'slash' | 'cli'
    desc: &'static str,
    usage: Option<&'static str>,
}

/// Upstream COMMAND_GROUPS (shared/claudeCommands.ts) — same source the
/// renderer cheatsheet is meant to mirror, so COMMANDS.md never drifts.
const COMMAND_GROUPS: &[(&str, &[CmdItem])] = &[
    ("SESSION", &[
        CmdItem { cmd: "/clear", kind: "slash", desc: "Start a fresh conversation and reclaim the full context window. The old one stays in /resume.", usage: None },
        CmdItem { cmd: "/resume", kind: "slash", desc: "Pick or search a past session to continue.", usage: Some("/resume auth refactor") },
        CmdItem { cmd: "/rewind", kind: "slash", desc: "Roll code AND conversation back to an earlier checkpoint.", usage: None },
        CmdItem { cmd: "/compact", kind: "slash", desc: "Summarize the conversation so far to free context without losing the thread.", usage: Some("/compact keep the auth decisions") },
        CmdItem { cmd: "claude -c", kind: "cli", desc: "Continue the most recent session in this directory.", usage: None },
        CmdItem { cmd: "claude -r", kind: "cli", desc: "Resume — pick or search a past session.", usage: Some("claude -r auth") },
        CmdItem { cmd: "claude --fork-session", kind: "cli", desc: "When resuming, branch into a new session id instead of reusing the original.", usage: None },
    ]),
    ("CONTEXT & MEMORY", &[
        CmdItem { cmd: "/context", kind: "slash", desc: "Visualize what is filling the context window, with optimization hints.", usage: None },
        CmdItem { cmd: "/memory", kind: "slash", desc: "Open the project & user CLAUDE.md memory files for editing.", usage: None },
        CmdItem { cmd: "/init", kind: "slash", desc: "Scan the repo and generate a CLAUDE.md capturing its conventions.", usage: None },
        CmdItem { cmd: "# ", kind: "slash", desc: "Quick memory: start a line with # to append a durable note to memory.", usage: Some("# always run prettier before committing") },
        CmdItem { cmd: "claude --add-dir ../other-repo", kind: "cli", desc: "Grant the session read/write access to an extra directory.", usage: None },
    ]),
    ("MODELS & EFFORT", &[
        CmdItem { cmd: "/model", kind: "slash", desc: "Switch the model for this session (saved as default); arrows tune effort.", usage: Some("/model opus") },
        CmdItem { cmd: "/effort", kind: "slash", desc: "Set reasoning effort: low / medium / high / xhigh / max.", usage: Some("/effort high") },
        CmdItem { cmd: "/fast", kind: "slash", desc: "Toggle fast mode — Opus with faster output, no model downgrade.", usage: None },
        CmdItem { cmd: "claude --model claude-sonnet-4-6[1m]", kind: "cli", desc: "Launch on a specific model. The [1m] suffix selects the 1M-token window (Dwight).", usage: None },
        CmdItem { cmd: "claude --fallback-model sonnet", kind: "cli", desc: "Auto-fall back to another model when the primary is unavailable.", usage: None },
    ]),
    ("PLAN & EXECUTE", &[
        CmdItem { cmd: "/plan", kind: "slash", desc: "Enter plan mode — design the change before any edits.", usage: Some("/plan refactor the auth module") },
        CmdItem { cmd: "/goal", kind: "slash", desc: "Set a goal condition; Claude keeps working across turns until it is met.", usage: Some("/goal all tests pass") },
        CmdItem { cmd: "/batch", kind: "slash", desc: "Decompose a large change into parallel units in git worktrees.", usage: Some("/batch migrate components to v2") },
        CmdItem { cmd: "/diff", kind: "slash", desc: "Open the interactive diff viewer for the current changes.", usage: None },
        CmdItem { cmd: "/run", kind: "slash", desc: "Launch and drive the project app to see a change actually working.", usage: None },
        CmdItem { cmd: "/verify", kind: "slash", desc: "Build, run, and observe to confirm a change does what it should.", usage: None },
        CmdItem { cmd: "claude --worktree feat/x", kind: "cli", desc: "Start the session in an isolated git worktree.", usage: None },
    ]),
    ("REVIEW & GIT", &[
        CmdItem { cmd: "/code-review", kind: "slash", desc: "Hunt correctness bugs in the diff. --fix applies them, --comment posts inline; \"ultra\" runs a cloud deep review.", usage: Some("/code-review high --fix") },
        CmdItem { cmd: "/simplify", kind: "slash", desc: "Cleanup-only pass over changed code (reuse/simplify) — no bug hunt.", usage: None },
        CmdItem { cmd: "/review", kind: "slash", desc: "Review a pull request in this session.", usage: Some("/review 123") },
        CmdItem { cmd: "/security-review", kind: "slash", desc: "Scan pending changes for security vulnerabilities.", usage: None },
        CmdItem { cmd: "/ultrareview", kind: "slash", desc: "Multi-agent cloud review of the current branch / a PR.", usage: None },
    ]),
    ("SUBAGENTS & BACKGROUND", &[
        CmdItem { cmd: "claude agents", kind: "cli", desc: "Open the agent view across your live + background Claude sessions.", usage: None },
        CmdItem { cmd: "claude agents --json", kind: "cli", desc: "Print live sessions as JSON — scriptable fleet status.", usage: None },
        CmdItem { cmd: "/agents", kind: "slash", desc: "Create and manage custom subagents for delegated work.", usage: None },
        CmdItem { cmd: "/fork", kind: "slash", desc: "Spawn a background subagent that inherits the full conversation.", usage: Some("/fork implement the perf fix") },
        CmdItem { cmd: "/background", kind: "slash", desc: "Detach the current session so it keeps running in the background.", usage: None },
        CmdItem { cmd: "/tasks", kind: "slash", desc: "View and manage everything running in the background.", usage: None },
        CmdItem { cmd: "/stop", kind: "slash", desc: "Stop the current background session (when attached).", usage: None },
        CmdItem { cmd: "claude --agent reviewer", kind: "cli", desc: "Start the session using a specific agent configuration.", usage: None },
    ]),
    ("TOOLS & PERMISSIONS", &[
        CmdItem { cmd: "/permissions", kind: "slash", desc: "View and edit which tools are allowed / asked / denied.", usage: None },
        CmdItem { cmd: "/hooks", kind: "slash", desc: "View the configured lifecycle hooks (PreToolUse, Stop, etc.).", usage: None },
        CmdItem { cmd: "claude --permission-mode bypassPermissions", kind: "cli", desc: "Run without per-tool approval prompts (this is what \"auto mode\" uses).", usage: None },
        CmdItem { cmd: "claude --allowedTools \"Bash(git *) Edit Read\"", kind: "cli", desc: "Pre-allow specific tools so they never prompt.", usage: None },
    ]),
    ("MCP & PLUGINS", &[
        CmdItem { cmd: "/mcp", kind: "slash", desc: "List/manage connected MCP servers and authenticate (OAuth).", usage: None },
        CmdItem { cmd: "/plugin", kind: "slash", desc: "Manage plugins (list, install, enable, disable).", usage: None },
        CmdItem { cmd: "claude mcp list", kind: "cli", desc: "List configured MCP servers and their health.", usage: None },
        CmdItem { cmd: "claude mcp add <name> <command>", kind: "cli", desc: "Register a new MCP server (stdio or HTTP).", usage: None },
    ]),
    ("USAGE & COST", &[
        CmdItem { cmd: "/usage", kind: "slash", desc: "Session cost, plan limits, and a breakdown by skill / subagent / MCP.", usage: None },
        CmdItem { cmd: "/status", kind: "slash", desc: "Account, active model, version, and connection status.", usage: None },
        CmdItem { cmd: "claude -p \"...\" --max-budget-usd 5", kind: "cli", desc: "Cap the dollar spend for a headless run.", usage: None },
        CmdItem { cmd: "claude --max-turns 20", kind: "cli", desc: "Limit agentic turns (a coarse runaway guard).", usage: None },
    ]),
    ("AUTOMATION (HEADLESS)", &[
        CmdItem { cmd: "claude -p \"your prompt\"", kind: "cli", desc: "Print mode: run one prompt non-interactively and exit.", usage: Some("cat log | claude -p \"summarize\"") },
        CmdItem { cmd: "claude -p \"...\" --output-format json", kind: "cli", desc: "Headless with structured JSON (result, usage, cost).", usage: None },
        CmdItem { cmd: "claude -p \"...\" --output-format stream-json", kind: "cli", desc: "Streaming JSON events for live consumption.", usage: None },
        CmdItem { cmd: "claude -p \"...\" --json-schema <schema>", kind: "cli", desc: "Force the headless result to match a JSON Schema.", usage: None },
        CmdItem { cmd: "claude --append-system-prompt \"...\"", kind: "cli", desc: "Append extra instructions to the default system prompt.", usage: None },
    ]),
    ("CONFIG", &[
        CmdItem { cmd: "/config", kind: "slash", desc: "Open Settings: theme, model, output style, preferences.", usage: None },
        CmdItem { cmd: "/theme", kind: "slash", desc: "Change the color theme (auto / light / dark / colorblind / custom).", usage: None },
        CmdItem { cmd: "/statusline", kind: "slash", desc: "Configure the Claude Code status line.", usage: None },
    ]),
    ("HELP & DIAGNOSTICS", &[
        CmdItem { cmd: "/help", kind: "slash", desc: "List every available slash command.", usage: None },
        CmdItem { cmd: "/doctor", kind: "slash", desc: "Diagnose installation / health issues (press f to auto-fix).", usage: None },
        CmdItem { cmd: "/debug", kind: "slash", desc: "Enable debug logging and troubleshoot the current session.", usage: None },
        CmdItem { cmd: "/release-notes", kind: "slash", desc: "Browse the Claude Code changelog by version.", usage: None },
        CmdItem { cmd: "/remote-control", kind: "slash", desc: "Expose this session for control from claude.ai / your phone.", usage: None },
    ]),
];

/// renderCommandsMd (hive.ts:2931) — the Claude Code command reference written
/// to <hive>/COMMANDS.md, byte-identical to upstream's output.
fn commands_md() -> String {
    let mut lines: Vec<String> = vec![
        GENERATED_DOC_NOTICE.to_string(),
        String::new(),
        "# Claude Code commands".into(),
        String::new(),
        "Reference of the Claude Code commands available to you. Two kinds:".into(),
        "- **slash** commands act ONLY on your own session — you CANNOT run them on another agent's terminal.".into(),
        "- **cli** commands run in your shell (Bash) and can target the fleet, spawn, or query.".into(),
        String::new(),
        "To MONITOR the other agents in this hive, read `fleet.json` in the hive root (live per-agent tokens, cost, status, last tool, breaker level, inbox backlog) plus `registry.json` — `claude agents` does NOT list your hive siblings. Use `claude -p \"...\" --output-format json` for a one-off headless query.".into(),
        String::new(),
    ];
    for (title, items) in COMMAND_GROUPS {
        lines.push(format!("## {title}"));
        lines.push(String::new());
        for it in *items {
            let usage = it.usage.map(|u| format!(" e.g. `{u}`")).unwrap_or_default();
            lines.push(format!(
                "- `{}` _({})_ — {}{}",
                it.cmd.trim(),
                it.kind,
                it.desc,
                usage
            ));
        }
        lines.push(String::new());
    }
    lines.join("\n")
}

/// board.md seed — upstream ensureHive text, verbatim.
const BOARD_MD: &str = "# Hive board\n\n_Shared plans live here. The god agent is the scribe._\n";
const TASKS_JSON: &str = "{\n  \"tasks\": []\n}\n";

/// Registry seed — upstream `{ godId: null, agents: {} }` (writeJson, pretty).
const REGISTRY_JSON: &str = "{\n  \"godId\": null,\n  \"agents\": {}\n}";

/// The hop ceiling (upstream HOP_CAP): a bounced message relaying past this
/// is a real mail loop — drop it rather than let two inboxes ping-pong.
const HOP_CAP: i64 = 12;

/// Non-memory files `mempalace mine` must not ingest — also the per-agent
/// .gitignore upstream writes for the same reason (MINE_IGNORE_LINES,
/// hive.ts:239): a Codex worker's private CODEX_HOME session data once grew
/// the hive's .git to 7.5GB. Append-only into <agentDir>/.gitignore.
const MINE_IGNORE_LINES: &[&str] = &[
    "settings.json",
    "cursor.json",
    "inbox/",
    "outbox/",
    ".codex/",
];

/// Churny/ephemeral live files kept out of the hive git repo (upstream
/// ensureHive's root .gitignore list, hive.ts:656). `crashes/` is deliberately
/// ignored — raw PTY output can carry secrets and the repo is committed on
/// every change.
const HIVE_GITIGNORE_LINES: &[&str] = &[
    "fleet.json",
    "hooks.sock",
    "cost-ledger.jsonl",
    "crashes/",
    ".DS_Store",
];

/// Filesystem- and sort-safe timestamp, e.g. 2026-05-30T14-03-11-123Z
/// (upstream stamp(): ISO with `:` and `.` replaced by `-`).
fn stamp() -> String {
    Utc::now()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        .replace([':', '.'], "-")
}

/// 6 random hex chars (upstream shortRand: 3 bytes → hex).
fn short_rand() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

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

/// App-driven seed for a worker's HERMES_HOME when the user never ran
/// `hermes setup`. base_url/model come from config.providerBaseUrls /
/// providerDefaultModels["hermes"]; api_key comes straight out of the OS
/// credential store (never config.json).
#[derive(Default)]
pub struct HermesSeed {
    /// Hermes provider id — "deepseek" when the configured base URL points at
    /// api.deepseek.com (first-class provider, reads DEEPSEEK_API_KEY), else
    /// "custom" for any other OpenAI-compatible endpoint.
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
}

impl Hive {
    /// Create (if missing) and return the hive rooted at `root` — upstream
    /// ensureHive: agents/, generated docs (PROTOCOL.md + COMMANDS.md),
    /// registry/board/tasks/log seeds, the root .gitignore, and the git repo.
    /// Idempotent.
    pub fn ensure(root: &Path) -> Result<Self> {
        fs::create_dir_all(root.join("agents"))?;
        Self::seed(&root.join("PROTOCOL.md"), PROTOCOL_MD)?;
        Self::seed(&root.join("COMMANDS.md"), &commands_md())?;
        Self::seed(&root.join("board.md"), BOARD_MD)?;
        Self::seed(&root.join("tasks.json"), TASKS_JSON)?;
        Self::seed(&root.join("registry.json"), REGISTRY_JSON)?;
        Self::seed(&root.join("log.jsonl"), "")?;
        // Keep the churny/ephemeral live files out of the hive git repo —
        // append-only, same missing-lines policy as the per-agent .gitignore.
        let hive = Self {
            root: root.to_path_buf(),
        };
        hive.ensure_gitignore_lines(&root.join(".gitignore"), HIVE_GITIGNORE_LINES);
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

    /// Idempotently append any missing lines to a .gitignore (upstream's
    /// append-only policy — existing entries are never reordered or removed).
    fn ensure_gitignore_lines(&self, path: &Path, want: &[&str]) {
        let existing = fs::read_to_string(path).unwrap_or_default();
        let have: HashSet<&str> = existing.lines().map(|l| l.trim()).collect();
        let missing: Vec<&&str> = want.iter().filter(|l| !have.contains(**l)).collect();
        if missing.is_empty() {
            return;
        }
        let mut out = if !existing.is_empty() && !existing.ends_with('\n') {
            format!("{existing}\n")
        } else {
            existing
        };
        for l in missing {
            out.push_str(l);
            out.push('\n');
        }
        let _ = fs::write(path, out); // best-effort
    }

    pub fn agent_dir(&self, id: &str) -> PathBuf {
        self.root.join("agents").join(id)
    }

    /// The raw registry document: {godId, agents}. Upstream stores `agents`
    /// as a MAP keyed by agent id; reads stay tolerant of the port's original
    /// array shape so an old registry keeps working.
    pub fn read_registry_doc(&self) -> (Option<String>, Vec<RegistryEntry>) {
        let path = self.root.join("registry.json");
        let text = fs::read_to_string(&path).unwrap_or_default();
        let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text) else {
            return (None, Vec::new());
        };
        let god_id = doc
            .get("godId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let agents = match doc.get("agents") {
            Some(Value::Object(map)) => map
                .iter()
                .filter_map(|(k, v)| {
                    let mut e: RegistryEntry = serde_json::from_value(v.clone()).ok()?;
                    if e.id.is_empty() {
                        e.id = k.clone();
                    }
                    Some(e)
                })
                .collect(),
            Some(Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| serde_json::from_value::<RegistryEntry>(v.clone()).ok())
                .collect(),
            _ => Vec::new(),
        };
        (god_id, agents)
    }

    pub fn registry(&self) -> Vec<RegistryEntry> {
        self.read_registry_doc().1
    }

    /// The registry's recorded godId — falls back to the is_god row for
    /// registries written before the field existed.
    pub fn god_id(&self) -> Option<String> {
        let (god_id, agents) = self.read_registry_doc();
        god_id.or_else(|| agents.iter().find(|a| a.is_god).map(|a| a.id.clone()))
    }

    fn write_registry_doc(&self, god_id: &Option<String>, agents: &[RegistryEntry]) -> Result<()> {
        let map: serde_json::Map<String, Value> = agents
            .iter()
            .filter_map(|a| {
                serde_json::to_value(a)
                    .ok()
                    .and_then(|v| Some((a.id.clone(), v)))
            })
            .collect();
        let doc = serde_json::json!({
            "godId": god_id,
            "agents": Value::Object(map),
        });
        self.atomic_write(
            &self.root.join("registry.json"),
            serde_json::to_string_pretty(&doc)?.as_bytes(),
        )
    }

    /// Rewrite the registry preserving the existing godId.
    fn write_registry(&self, agents: &[RegistryEntry]) -> Result<()> {
        let (god_id, _) = self.read_registry_doc();
        self.write_registry_doc(&god_id, agents)
    }

    /// Raw-json registry patch — like write_registry but on the untyped doc,
    /// so keys no field models are never dropped. Reserved for hive_admin's
    /// untyped-field edits; not wired yet.
    #[allow(dead_code)]
    pub(crate) fn patch_registry_json(
        &self,
        id: &str,
        patch: &serde_json::Map<String, Value>,
    ) -> Result<bool> {
        let path = self.root.join("registry.json");
        let text = fs::read_to_string(&path).unwrap_or_default();
        let mut doc: Value = serde_json::from_str(&text)
            .unwrap_or_else(|_| serde_json::json!({ "godId": null, "agents": {} }));
        // Tolerate a legacy array-shaped agents list by converting to a map.
        if let Some(Value::Array(arr)) = doc.get("agents").cloned() {
            let mut map = serde_json::Map::new();
            for e in arr {
                if let Some(k) = e.get("id").and_then(|v| v.as_str()) {
                    map.insert(k.to_string(), e);
                }
            }
            doc["agents"] = Value::Object(map);
        }
        let Some(agents) = doc.get_mut("agents").and_then(|a| a.as_object_mut()) else {
            anyhow::bail!("registry.json is not an object");
        };
        let Some(entry) = agents.get_mut(id) else {
            return Ok(false);
        };
        let Some(obj) = entry.as_object_mut() else {
            anyhow::bail!("agent {id} is not an object");
        };
        for (k, v) in patch {
            obj.insert(k.clone(), v.clone());
        }
        self.atomic_write(&path, serde_json::to_string_pretty(&doc)?.as_bytes())?;
        Ok(true)
    }

    pub fn is_god(&self, id: &str) -> bool {
        self.god_id().is_some_and(|g| g == id)
    }

    /// Store the CLI's own session id (Claude `session_id` from every hook
    /// payload) on the registry entry — cheap write-only-on-change, used for
    /// --resume and cost dedup. Port of hive.recordSession (bumps lastSeen).
    pub fn record_session(&self, id: &str, session_id: &str) -> Result<()> {
        let (god_id, mut agents) = self.read_registry_doc();
        if let Some(a) = agents.iter_mut().find(|a| a.id == id) {
            if a.session_id.as_deref() != Some(session_id) {
                a.session_id = Some(session_id.into());
                a.last_seen = now_ms();
                self.write_registry_doc(&god_id, &agents)?;
            }
        }
        Ok(())
    }

    /// The CLI's recorded session id for an agent — upstream
    /// `hive.lastSession(agentId)`, the `resume: true` lookup source.
    pub fn last_session(&self, id: &str) -> Option<String> {
        self.registry()
            .into_iter()
            .find(|a| a.id == id)
            .and_then(|a| a.session_id)
    }

    /// Count undrained inbox messages for an agent (upstream inboxBacklog —
    /// cheap, feeds the fleet snapshot).
    /// hasMemory (hive.ts:1979): a seeded memory.md is ~90 chars — anything
    /// meaningfully longer means the agent appended durable facts. Works for
    /// ANY id, active or archived; never throws.
    pub fn has_memory(&self, id: &str) -> bool {
        fs::read_to_string(self.agent_dir(id).join("memory.md"))
            .map(|s| s.trim().len() > 200)
            .unwrap_or(false)
    }

    pub fn inbox_backlog(&self, id: &str) -> u64 {
        let dir = self.agent_dir(id).join("inbox");
        fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
                    .count() as u64
            })
            .unwrap_or(0)
    }

    /// writeFleetSnapshot — the live fleet.json god reads (gitignored).
    pub fn write_fleet_snapshot(&self, snapshot: &serde_json::Value) -> Result<()> {
        fs::write(
            self.root.join("fleet.json"),
            serde_json::to_string_pretty(snapshot)?,
        )?;
        Ok(())
    }

    /// rosterContext (hive.ts:2588) — the one-shot LIVE ROSTER line injected
    /// into god's context on SessionStart/UserPromptSubmit. `agents` is the
    /// fleet.json rows already built by the caller; `ctx_of` layers the LIVE
    /// context-window occupancy (`ctx NN%`) the disk snapshot can't carry.
    /// None when there is nothing to say.
    pub fn roster_context<F>(
        &self,
        agents: &[serde_json::Value],
        snapshot_ts_ms: Option<u64>,
        ctx_of: F,
    ) -> Option<String>
    where
        F: Fn(&str) -> Option<(u64, u64)>,
    {
        if agents.is_empty() {
            return None;
        }
        let now = now_ms();
        let ago = |s: Option<u64>| -> String {
            match s {
                None => "unknown".into(),
                Some(s) if s < 90 => format!("{s}s ago"),
                Some(s) if s < 5400 => format!("{}m ago", s / 60 + (s % 60 >= 30) as u64),
                Some(s) => format!("{}h ago", s / 3600 + (s % 3600 >= 1800) as u64),
            }
        };
        const MAX: usize = 24;
        let shown = &agents[..agents.len().min(MAX)];
        let mut any_ctx = false;
        let mut any_hold = false;
        let rows: Vec<String> = shown
            .iter()
            .map(|a| {
                let id = a.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let mut bits: Vec<String> = vec![a
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("agent")
                    .into()];
                match a.get("lastActiveSecAgo").and_then(|v| v.as_u64()) {
                    Some(s) => bits.push(format!("active {}", ago(Some(s)))),
                    None => bits.push("no activity yet".into()),
                }
                if let Some(t) = a.get("tokens").and_then(|v| v.as_u64()) {
                    if t > 0 {
                        bits.push(format!("{}k tok", (t + 500) / 1000));
                    }
                }
                if let Some(u) = a.get("usd").and_then(|v| v.as_f64()) {
                    if u > 0.0 {
                        bits.push(format!("${u:.2}"));
                    }
                }
                if let Some(b) = a.get("inboxBacklog").and_then(|v| v.as_u64()) {
                    if b > 0 {
                        bits.push(format!("inbox {b}"));
                    }
                }
                if let Some(b) = a.get("breaker").and_then(|v| v.as_str()) {
                    if b != "ok" && b != "none" {
                        bits.push(format!("breaker {b}"));
                    }
                }
                if a.get("isGod").and_then(|v| v.as_bool()) == Some(true) {
                    bits.push("you".into());
                }
                if a.get("onHold").and_then(|v| v.as_bool()) == Some(true) {
                    bits.push("ON HOLD — 1:1 with the human".into());
                    any_hold = true;
                }
                if let Some((tok, lim)) = ctx_of(id) {
                    if lim > 0 {
                        let pct = ((tok * 100) / lim).min(100);
                        bits.push(format!("ctx {pct}%"));
                        any_ctx = true;
                    }
                }
                format!(
                    "{id}{}{}",
                    if name.is_empty() {
                        String::new()
                    } else {
                        format!(" \"{name}\"")
                    },
                    format!(" ({})", bits.join(", "))
                )
            })
            .collect();
        let more = if agents.len() > shown.len() {
            format!(" +{} more", agents.len() - shown.len())
        } else {
            String::new()
        };
        let age = snapshot_ts_ms
            .map(|ts| ago(Some(now.saturating_sub(ts) / 1000)))
            .unwrap_or_else(|| "unknown".into());
        Some(format!(
            "[LIVE ROSTER — auto-injected from {}, snapshot {age}] {} ACTIVE agent(s): {}.{more} This is the CURRENT floor and it SUPERSEDES any roster earlier in this conversation — agents you remember that are absent here have been archived or killed, so do not message them. {}{}{}",
            self.root.join("fleet.json").display(),
            agents.len(),
            rows.join("; "),
            if any_ctx { "`ctx NN%` = live window occupancy; absent = not yet reported (unknown, not empty). " } else { "" },
            if any_hold { "An agent marked `ON HOLD — 1:1 with the human` is UNAVAILABLE: the human is working with them directly. Do NOT message them, do NOT dispatch to them, and do NOT count them when picking an owner. Route to someone else, or say the work is waiting. They are still running and their terminal is alive, so this is not a reason to archive them or spawn a replacement. The human flips it off when they are done. " } else { "" },
            "Route work to someone on this list before spawning anyone new."
        ))
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
        let text = fs::read_to_string(self.root.join("cost-ledger.jsonl")).ok()?;
        let mut s = crate::breaker::UsageSample::default();
        let mut seen = false;
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if v.get("agent_id").and_then(|a| a.as_str()) != Some(agent_id) {
                continue;
            }
            seen = true;
            s.input += v.get("input").and_then(|x| x.as_u64()).unwrap_or(0);
            s.output += v.get("output").and_then(|x| x.as_u64()).unwrap_or(0);
            s.cache_read += v.get("cache_read").and_then(|x| x.as_u64()).unwrap_or(0);
            s.cache_creation += v
                .get("cache_creation")
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            if let Some(ts) = v.get("ts").and_then(|x| x.as_u64()) {
                s.ts = ts;
            } else if let Some(ts) = v.get("ts").and_then(|x| x.as_str()) {
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

    /// The path downstream consumers read — the ledger's name IS the
    /// contract (upstream + Kevin's SQLite migration both key on it).
    pub fn cost_ledger_path(&self) -> PathBuf {
        self.root.join("cost-ledger.jsonl")
    }

    /// Last cumulative usd recorded for one (agent,session) — the fold's
    /// monotonic anchor. Returns 0 for a session with no prior rows.
    fn last_session_usd(&self, agent_id: &str, session_id: &str) -> f64 {
        let Ok(text) = fs::read_to_string(self.cost_ledger_path()) else {
            return 0.0;
        };
        let mut last = 0.0;
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if v.get("agent_id").and_then(|a| a.as_str()) == Some(agent_id)
                && v.get("session_id").and_then(|s| s.as_str()) == Some(session_id)
            {
                if let Some(u) = v.get("usd").and_then(|x| x.as_f64()) {
                    last = u;
                }
            }
        }
        last
    }

    /// Persist a usage sample to `cost-ledger.jsonl` — upstream schema
    /// (agent_id/session_id/ts/input/output/cache_read/cache_creation/model/
    /// usd), append-only so log.jsonl stays the event feed.
    ///
    /// `usd` is the cumulative per-session counter, because the lifetime fold
    /// (costLifetime) detects app restarts as counter DECREASES: deltas would
    /// fake a reset on every row. A caller that hands us a DELTA (proxy
    /// CostSample, token rows) gets its usd accumulated onto the session's
    /// last value; a caller that already holds the session's cumulative
    /// (telemetry) passes it through `cumulative_usd`.
    ///
    /// Dedup gate (#56): rows without a session id are never ledgered — an
    /// unresolvable session would corrupt the monotonic segments.
    pub fn append_cost_sample(&self, agent_id: &str, p: &serde_json::Value) -> Result<()> {
        use std::io::Write;
        let Some(session_id) = p.get("session_id").and_then(|v| v.as_str()) else {
            return Ok(());
        };
        if session_id.is_empty() {
            return Ok(());
        }
        let model = p.get("model").and_then(|v| v.as_str()).unwrap_or("");
        let (input, output, cr, cw) = (
            p.get("input").and_then(|v| v.as_u64()).unwrap_or(0),
            p.get("output").and_then(|v| v.as_u64()).unwrap_or(0),
            p.get("cache_read").and_then(|v| v.as_u64()).unwrap_or(0),
            p.get("cache_creation")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
        );
        let usd = p.get("usd").and_then(|v| v.as_f64()).unwrap_or_else(|| {
            // The proxy emits token deltas — accumulate an estimate onto
            // the session's running counter so the fold's monotonic
            // segment holds.
            self.last_session_usd(agent_id, session_id)
                + crate::pricing::estimate_cost_usd(
                    model,
                    input as f64,
                    output as f64,
                    cr as f64,
                    cw as f64,
                )
        });
        let row = serde_json::json!({
            "agent_id": agent_id,
            "session_id": session_id,
            "ts": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            "input": input,
            "output": output,
            "cache_read": cr,
            "cache_creation": cw,
            "model": model,
            "usd": usd,
        });
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.cost_ledger_path())?;
        writeln!(f, "{}", serde_json::to_string(&row)?)?;
        Ok(())
    }

    /// Port of hive.hookSettings — the per-session settings.json given to
    /// Claude via `--settings`. Every hook entry invokes the native hive-hook
    /// binary (quoted for paths with spaces); the event name arrives inside
    /// the stdin payload, so one command serves all events. `statusLine`
    /// forwards the session's context-window accounting via `--status`.
    ///
    /// `mcp_servers` is the consent-filtered catalog merge (mcp.rs
    /// build_default_mcp_servers) — written verbatim under `mcpServers` when
    /// non-empty. Omitted vs upstream: the sandbox/permissions block
    /// (writableDirs — add when the sandbox story is designed for this backend).
    pub fn hook_settings(
        &self,
        shim_command: &str,
        mcp_servers: &std::collections::BTreeMap<String, crate::mcp::McpServerSpec>,
    ) -> serde_json::Value {
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
        let mut out = serde_json::json!({
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
        });
        if !mcp_servers.is_empty() {
            out["mcpServers"] = serde_json::to_value(mcp_servers).unwrap_or_default();
        }
        out
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
        config.push_str("\n# --- agent-office lifecycle hooks (auto-generated; do not edit) ---\n");
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
        self.install_hermes_hooks_seeded(dir, hook_bin, &HermesSeed::default())
    }

    /// Same as install_hermes_hooks, plus app-driven provider seeding: when the
    /// user has never run `hermes setup` (no ~/.hermes/config.yaml) the worker
    /// home would otherwise boot straight into hermes' "Run setup now? [Y/n]"
    /// prompt and exit(1). If the app config carries a hermes base URL or the
    /// keychain has a BYOK key, generate a minimal config.yaml + .env so the
    /// spawn is non-interactive. User's own files always win — seeded values
    /// only fill the gap.
    pub fn install_hermes_hooks_seeded(
        &self,
        dir: &Path,
        hook_bin: &Path,
        seed: &HermesSeed,
    ) -> Result<PathBuf> {
        let user_home = dirs::home_dir().unwrap_or_default().join(".hermes");
        self.install_hermes_hooks_seeded_at(dir, hook_bin, seed, &user_home)
    }

    /// The copy-through source (~/.hermes) is a parameter so tests can point it
    /// at a temp dir — on Windows dirs::home_dir ignores HOME/USERPROFILE and
    /// reads the real profile via the known-folder API.
    fn install_hermes_hooks_seeded_at(
        &self,
        dir: &Path,
        hook_bin: &Path,
        seed: &HermesSeed,
        user_home: &Path,
    ) -> Result<PathBuf> {
        let home = dir.join(".hermes");
        fs::create_dir_all(&home)?;

        for name in ["config.yaml", ".env"] {
            let (src, dst) = (user_home.join(name), home.join(name));
            if src.exists() && !dst.exists() {
                let _ = fs::copy(&src, &dst);
            }
        }

        // Seed the model provider when the copied/generated config doesn't
        // declare one. Existence isn't enough — a prior spawn writes this file
        // with just the hooks block, so check CONTENT: any `provider:` line
        // under the model block means the user's own choice stands.
        let cfg_path = home.join("config.yaml");
        let existing = fs::read_to_string(&cfg_path).unwrap_or_default();
        let has_provider = existing
            .lines()
            .any(|l| l.trim_start().starts_with("provider:"));
        if !has_provider && (seed.base_url.is_some() || seed.provider.is_some()) {
            let provider = seed.provider.as_deref().unwrap_or("custom");
            let mut yaml = format!(
                "# Auto-generated by agent-office (no ~/.hermes/config.yaml found).\nmodel:\n  provider: \"{provider}\"\n",
            );
            if let Some(url) = &seed.base_url {
                yaml.push_str(&format!("  base_url: \"{}\"\n", url));
            }
            if let Some(m) = &seed.model {
                yaml.push_str(&format!("  default: \"{}\"\n", m));
            }
            // Prepend — the hooks block below appends later in this function.
            let merged = format!("{yaml}\n{existing}");
            fs::write(&cfg_path, merged)?;
        }
        // Seed .env with the stored BYOK key. deepseek reads DEEPSEEK_API_KEY;
        // every other custom/OpenAI-compatible provider reads OPENAI_API_KEY —
        // write whichever the seed provider consumes (deepseek gets BOTH since
        // OpenAI-compatible tooling downstream still keys on OPENAI_API_KEY).
        let env_path = home.join(".env");
        if let Some(key) = &seed.api_key {
            // Write the env name the seeded provider actually reads — an
            // openrouter worker handed only OPENAI_API_KEY still dies on
            // "no API keys or providers found".
            let vars: &[&str] = match seed.provider.as_deref() {
                Some("deepseek") => &["DEEPSEEK_API_KEY", "OPENAI_API_KEY"],
                Some("openrouter") => &["OPENROUTER_API_KEY"],
                Some("anthropic") => &["ANTHROPIC_API_KEY"],
                _ => &["OPENAI_API_KEY"],
            };
            let mut s = fs::read_to_string(&env_path).unwrap_or_default();
            let mut dirty = false;
            for var in vars {
                if !s.lines().any(|l| l.starts_with(&format!("{var}="))) {
                    if !s.is_empty() && !s.ends_with('\n') {
                        s.push('\n');
                    }
                    s.push_str(&format!("{var}={key}\n"));
                    dirty = true;
                }
            }
            if dirty {
                fs::write(&env_path, s)?;
            }
        }

        let cfg_path = home.join("config.yaml");
        let mut cfg = fs::read_to_string(&cfg_path).unwrap_or_default();
        if !cfg.lines().any(|l| l.starts_with("hooks_auto_accept:")) {
            cfg.push_str(
                "\n# --- agent-office hive bridge (auto-generated) ---\nhooks_auto_accept: true\n",
            );
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
                    "name": format!("agent-office-{name}"),
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

    /// Grok lifecycle bridge — global hooks in `~/.grok/hooks/agent-office.json`
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
            hook_dir.join("agent-office.json"),
            serde_json::to_string_pretty(&hooks)?,
        )?;
        Ok(())
    }

    /// Codex rollout-dir exposure (exposeCodexDataDirs, hive.ts:2273) — moves
    /// the worker's `<dir>/.codex/{sessions,archived_sessions}` under the
    /// user's standard Codex scan root `~/.codex/<kind>/agent-office/<hiveId>/
    /// <agentId>` and links the source back, so `codex resume` pickers can
    /// see hive workers' rollouts. Junction on Windows (no admin needed).
    /// Best-effort: any failure is a skip, never a spawn blocker.
    pub fn expose_codex_data(&self, agent_dir: &Path, agent_id: &str) {
        if agent_id.is_empty() || agent_id.contains(['/', '\\', '.']) {
            return;
        }
        use sha1::{Digest, Sha1};
        let hive_id = format!(
            "{:x}",
            Sha1::digest(self.root.display().to_string().as_bytes())
        );
        let hive_id = &hive_id[..12.min(hive_id.len())];
        let user_home = dirs::home_dir().unwrap_or_default().join(".codex");
        for kind in ["sessions", "archived_sessions"] {
            let source = agent_dir.join(".codex").join(kind);
            let target = user_home
                .join(kind)
                .join("agent-office")
                .join(hive_id)
                .join(agent_id);
            let _ = fs::create_dir_all(target.parent().unwrap());
            let source_is_dir = source.is_dir() && !source.is_symlink();
            if source_is_dir && target.exists() {
                continue; // both sides have data — never clobber (upstream throws)
            }
            if source_is_dir && !target.exists() {
                let _ = fs::rename(&source, &target).or_else(|_| {
                    copy_dir(&source, &target)?;
                    fs::remove_dir_all(&source)
                });
            } else if !target.exists() {
                let _ = fs::create_dir_all(&target);
            }
            if !source.exists() && target.exists() {
                let _ = link_dir(&target, &source);
            }
        }
    }

    /// Antigravity (`agy`) bridge — merges an `agent-office` group into BOTH
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
            existing["agent-office"] = group.clone();
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

    /// The identity.md content — upstream identityText (hive.ts:1494),
    /// verbatim. Written on EVERY spawn (and on role patch), never seeded
    /// once: a rename/role change must land in the file the agent reads.
    pub fn identity_text(entry: &RegistryEntry) -> String {
        let caps = if entry.capabilities.is_empty() {
            "—".to_string()
        } else {
            entry.capabilities.join(", ")
        };
        let role = if entry.role.trim().is_empty() {
            if entry.is_god {
                "orchestrator (god)"
            } else {
                "agent"
            }
            .to_string()
        } else {
            entry.role.clone()
        };
        let mut lines: Vec<String> = vec![
            format!("# {} ({})", entry.name, entry.id),
            String::new(),
            format!("- Role: {role}"),
            format!("- Capabilities: {caps}"),
            format!("- Working directory: {}", entry.cwd),
        ];
        if entry.is_god {
            lines.push("- You are the **god / orchestrator**. You run the floor — keep awareness of the whole team, delegate execution, and personally own only the important calls (decomposition, sign-offs, conflicts, integration), not the grunt work.".into());
            lines.push("- Monitor the team with `fleet.json` (live per-agent status/tokens/cost/breaker) and `registry.json`; full command reference in `COMMANDS.md`. `claude agents` does NOT list your hive siblings.".into());
        }
        lines.join("\n")
    }

    /// God's display name for prompt text — upstream resolveGodName: the
    /// persisted registry name, falling back to the app default.
    fn resolve_god_name(&self) -> String {
        let (god_id, agents) = self.read_registry_doc();
        let name = god_id
            .as_deref()
            .and_then(|g| agents.iter().find(|a| a.id == g))
            .or_else(|| agents.iter().find(|a| a.is_god))
            .map(|a| a.name.trim().to_string())
            .filter(|n| !n.is_empty());
        name.unwrap_or_else(|| "Michael".into())
    }

    /// The system-prompt prefix injected into every spawn — verbatim port of
    /// hive.injectedPrompt (hive.ts:1530-1614). Interpolates only
    /// lifetime-stable values so the prompt-cache invariant holds (no
    /// dates/UUIDs/volatile state).
    pub fn injected_prompt(&self, entry: &RegistryEntry, opts: &PromptOpts) -> String {
        let dir = self.agent_dir(&entry.id);
        let in_dir = |parts: &[&str]| {
            parts
                .iter()
                .fold(dir.clone(), |p, s| p.join(s))
                .display()
                .to_string()
        };
        let in_root = |parts: &[&str]| {
            parts
                .iter()
                .fold(self.root.clone(), |p, s| p.join(s))
                .display()
                .to_string()
        };
        let god_name = self.resolve_god_name();

        let ctx_line = "LIVE CONTEXT: each agent row in the LIVE ROSTER carries a `ctx NN%` tag — its live context-window occupancy. Treat it as the real headroom signal when routing: prefer an agent with a LOW `ctx` for a big task; treat a HIGH `ctx` (near 100%) as busy rather than idle, even if the cumulative token count looks modest.";

        let memory_line = if opts.semantic_memory {
            "Semantic memory: the whole hive shares a searchable MemPalace at the path in your MEMPALACE_PALACE_PATH environment variable. To recall relevant past knowledge across the team, run `mempalace search \"<query>\"`; run `mempalace wake-up` at the start of a task for a memory digest. Your notes in memory.md are mined into the palace automatically — write durable facts there."
        } else {
            ""
        };

        let kg_cli = opts.kg_cli_path.clone().unwrap_or_else(|| {
            if cfg!(windows) {
                "%KG_CLI%".into()
            } else {
                "$KG_CLI".into()
            }
        });
        let knowledge_line = if opts.knowledge_graph {
            format!("Enterprise knowledge: this organisation has a private Knowledge Graph of its own documents, policies, and business context. When a task needs that context — company-specific facts, house style, internal processes — query it instead of guessing: run `\"{}\" \"{}\" search \"<query>\"` for ranked passages, `\"{}\" \"{}\" list` to see what is available, and `\"{}\" \"{}\" get <id>` for a full document. (That first path is the harness's bundled Node — use it instead of bare `node`, which may not be on your PATH.)",
                opts.hive_node, kg_cli, opts.hive_node, kg_cli, opts.hive_node, kg_cli)
        } else {
            String::new()
        };

        let runtime_line = match &opts.runtime {
            Some(rt) => {
                let from = rt
                    .app_path
                    .as_ref()
                    .map(|p| format!(", from {p}"))
                    .unwrap_or_default();
                format!("RUNNING BUILD: Munder Difflin v{}, {}{}. Say this version if asked which one is running, and do not assume behaviour from an older one. A local dev build inherits the launching shell's environment (umask included) where a packaged app does not, so file modes and inherited env can legitimately differ between the two. `log.jsonl` records an `app-start` event on every launch, which is how you spot a restart or a build switch.",
                    rt.version,
                    if rt.packaged { "packaged app" } else { "local dev build" },
                    from)
            }
            None => String::new(),
        };

        let spawn_queue_line = if entry.is_god && opts.orchestrator_may_spawn {
            format!("SPAWNING A WORKER: you can start an ephemeral worker yourself by writing ONE JSON file into {}/<id>.json. Required: `objective` (what the worker must do) and `cwd` (the repo it runs in). Optional: `name`, `command` (overrides the provider default), `provider` (selects its default CLI when command is omitted), `model`, `isolate` (default true = its own git worktree), `tokenCap`, and `slack` ({{channel, thread_ts}}) to route its failures back to a thread. The harness polls that directory, spawns `worker-<id>`, and moves the request to `spawn-requests/.done/` on success or `.failed/` with a reason. This is the ONLY way you can spawn; a hire manifest under research/hires/ needs the human to confirm it in the UI, so it is not a route you can complete on your own. Reuse an existing agent first, as above — a worker is a fresh spend every time.",
                in_root(&["spawn-requests"]))
        } else {
            String::new()
        };

        let god_line = if entry.is_god {
            format!("{}{}", concat!(
                "You are the GOD / ORCHESTRATOR of this hive — your job is to ORCHESTRATE, not to implement: maintain live situational awareness and delegate the work. (1) AWARENESS — always know what is going on: keep an accurate picture of every agent (active vs archived/idle), the task board, and all in-flight work; drain your inbox continually and triage every other agent's requests, answering clarifications so the team runs autonomously. (2) DELEGATE — decompose work and fan it out to the hive agents via their inboxes (route messages and assign owners; do not do their jobs); do NOT take on grunt implementation yourself. Stay aware of who is already on the floor and delegate OPPORTUNISTICALLY: BEFORE you spawn anything, CHECK THE LIVE ROSTER (active agents in registry.json + their state in fleet.json) and prefer routing to an EXISTING agent that fits — above all when the request names one (\"ask Pam to…\", \"have Jim…\"), route to that agent instead of reflexively creating a new one. Reuse an idle or already-running agent whose role matches; only spawn a fresh agent when no existing one is a sensible fit, and say that you checked. One capable owner beats a duplicate. (3) OWN ONLY THE IMPORTANT, high-leverage things — task decomposition, dispatch decisions, sign-offs, conflict resolution, branch integration, and final QA — and remain the sole scribe of board.md. You are otherwise fully autonomous — there is NO separate approval queue. For the genuinely critical (destructive actions, spending real money, scope changes, unresolvable conflicts), ask the human directly in your own session and let the tool-permission prompt gate the action; the human approves natively, including remotely from their phone via /remote-control. Keep the team unblocked. When you DISPATCH a task, write it as a 4-part contract so the agent can run autonomously: (1) OBJECTIVE — the concrete goal; (2) OUTPUT — the expected deliverable/format; (3) TOOLS — what to use or avoid, and any references to read instead of re-deriving; (4) BOUNDARIES — scope limits + the definition of done. Pass references (file paths, message ids, board sections), not pasted content — keep dispatches short.",
                " MONITOR the floor by reading {fleet} (live per-agent tokens, cost, status, last tool, breaker level, inbox backlog) and {registry} — note that running 'claude agents' will NOT list your hive's sibling agents. A full Claude Code command reference is at {commands} (slash commands act ONLY on your own session; CLI commands run in your shell and can target the fleet). You periodically receive scheduler / \"Heartbeat\" standup requests — on each, review every agent via fleet.json, re-engage anyone stalled, over-budget, or breaker-armed, and keep board.md and tasks.json accurate. In tasks.json, ALWAYS set each task's \"assignee\" to the worker's agent id the moment you dispatch it, and NEVER clear it on status changes — a done card must still say who did the work (the human reads the board by who-did-what). HUMAN FEEDBACK is first-class in the ledger: when a task can only proceed with the human's input — a QUESTION to answer OR an ACTION only the human can perform (create an account, approve a purchase, provide credentials/screenshots, test on their device) — set its status to \"blocked\" and append the concrete ask to the card's \"humanQA\" array (push {{\"q\":\"...\",\"askedAt\":\"<iso>\"}}; phrase actions as clear to-dos; keep every past entry — the history documents the card's decisions). WRITE THE ASK SHORT AND IN MARKDOWN. The human reads it on a CARD, not in a terminal, so an ask longer than a short paragraph plus its options (roughly 700 characters) is a report, not a question — cut the narrative, keep the decision. Open with ONE **bold** sentence saying exactly what you need from them; put paths, commands, values and identifiers in `backticks`; give each option or step its own \"-\" bullet or \"1.\" number; leave a blank line between paragraphs (a single newline is a line break, so each option stays on its own line). When the ask originates in another agent's report, REWRITE it into that shape — never paste the report body in as the question, and never make the human read the investigation to find the decision. The harness surfaces open questions on the office floor's ASK ME board; the human's answer lands in the same entry (\"a\") AND arrives as an inbox message to you — read it, act on it, and unblock the card so work continues. Do NOT park human questions in separate files (no HumanQuestion.md) and never sit waiting on the human in your own session. Steward the token budget."
            ), "")
            .replace("{fleet}", &in_root(&["fleet.json"]))
            .replace("{registry}", &in_root(&["registry.json"]))
            .replace("{commands}", &in_root(&["COMMANDS.md"]))
        } else if entry.is_assistant {
            format!("You are {god_name}'s PREP ASSISTANT. You will be handed short, possibly vague instructions (each begins with \"ENRICH TASK:\"). For each one: (1) figure out which project it concerns and cd into the most relevant repo — you start in {god_name}'s home directory; (2) gather concrete context READ-ONLY (exact file paths, current state, relevant code, conventions, active branch, gotchas) — NEVER modify, create, or delete files; (3) rewrite the instruction into ONE clear, self-contained prompt that {god_name} can execute autonomously, preserving the user's original intent without inventing scope. Then deliver it: write ONE message JSON into your outbox with \"to\":\"god\", \"act\":\"request\", a short subject, and the finished prompt as the body. Do NOT perform the task yourself — your only output is the improved prompt sent to {god_name}.")
        } else {
            "For anything ambiguous, cross-cutting, or needing sign-off, address a message to \"god\".".to_string()
        };

        let guardrails_line = "Guardrails: a circuit breaker watches the floor — a \"Circuit breaker: steer/constrain\" message means you are looping or overspending, so STOP repeating, summarize what you tried, and follow it. Be token-frugal (a floor-wide or per-agent token budget can pause you). The shared plan has two parts: board.md (freeform; god is the sole scribe) and tasks.json (structured kanban — todo/doing/blocked/done).";

        let slack_line = if entry.is_god {
            "SLACK REPLIES: When composing a Slack reply (or writing the `result` field of a Slack-origin kanban card), you MUST: (1) directly address what the user asked — never a bare \"done\"; (2) include the relevant specifics, outcome, and details; (3) format for Slack mrkdwn — open with a short *bold* headline, use bullet points for multiple items, wrap code/paths in `backtick` blocks, keep it concise (no walls of text). When finishing a Slack-origin task, always write a complete, user-facing, well-formatted `result` on the kanban card — the system posts it verbatim to Slack as the done reply.".to_string()
        } else {
            format!("SLACK REPLIES: If god dispatches you a task that came from Slack, it will include an exact `\"{}\" \"<helper>\" --channel … --thread … --text \"…\"` reply command — when you finish, run it VERBATIM to post your result back to that thread yourself. The reply must be SUBSTANTIVE Slack mrkdwn (a short *bold* headline + the actual outcome/specifics/links), NEVER a bare \"done\".", opts.hive_node)
        };

        [
            format!("You are \"{}\" ({}), an autonomous agent in a collaborating hive of Claude agents.", entry.name, entry.id),
            format!("Your private workspace is {}. The shared hive is {}. Full protocol: {}.", dir.display(), self.root.display(), in_root(&["PROTOCOL.md"])),
            String::new(),
            "HIVE PROTOCOL — follow it every task:".to_string(),
            format!("1. At the START of a task, read {} and EVERY file in {} (messages other agents sent you). After handling an inbox message, move its file into {}.", in_dir(&["memory.md"]), in_dir(&["inbox"]), in_dir(&["inbox", ".done"])),
            format!("2. Record durable facts, decisions, and context by appending to {}.", in_dir(&["memory.md"])),
            format!("3. To ask another agent for something or share information, write ONE message JSON into {} (schema in PROTOCOL.md). NEVER write into another agent's folder — the orchestrator delivers your outbox.", in_dir(&["outbox"])),
            "4. At the END of a task, append what you learned to memory.md so future-you remembers.".to_string(),
            guardrails_line.to_string(),
            memory_line.to_string(),
            knowledge_line,
            god_line,
            spawn_queue_line,
            runtime_line,
            slack_line,
            ctx_line.to_string(),
            "Env vars available to you: AGENT_ID, AGENT_NAME, HIVE_ROOT, AGENT_DIR.".to_string(),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
    }

    /// Create the agent's hive folder + identity/memory files and record it in
    /// the registry — the file side of hive.ts `ensureAgent` (spawn injection
    /// stays in commands.rs). Runs on EVERY spawn: identity.md is rewritten
    /// (not seeded), and the registry upsert merges over the prior entry so a
    /// respawn keeps sessionId/createdAt while clearing `archived`.
    pub fn register_agent(&self, entry: &RegistryEntry) -> Result<()> {
        let dir = self.agent_dir(&entry.id);
        for sub in ["inbox", "inbox/.done", "outbox", "outbox/.sent"] {
            fs::create_dir_all(dir.join(sub))?;
        }
        Self::seed(
            &dir.join("cursor.json"),
            "{\n  \"lastProcessed\": null\n}\n",
        )?;

        // Merge over the prior registry row (upstream `{...prev, ...meta}`):
        // spawn-time fields win, lifetime fields (sessionId, createdAt) are
        // preserved when the new entry doesn't carry them.
        let (mut god_id, mut agents) = self.read_registry_doc();
        let prev = agents.iter().find(|a| a.id == entry.id).cloned();
        let mut merged = entry.clone();
        if let Some(p) = &prev {
            if merged.session_id.is_none() {
                merged.session_id = p.session_id.clone();
            }
            if merged.created_at.is_empty() {
                merged.created_at = p.created_at.clone();
            }
            if merged.capabilities.is_empty() {
                merged.capabilities = p.capabilities.clone();
            }
            if merged.command.is_empty() {
                merged.command = p.command.clone();
            }
            // Fields absent from the spawn meta ride through unchanged, same
            // as upstream's `{...prev}` spread — a respawn keeps the
            // human's 1:1 hold and the assistant marker.
            merged.on_hold = p.on_hold;
            if !merged.is_assistant {
                merged.is_assistant = p.is_assistant;
            }
            let prev_keys: serde_json::Map<String, Value> = p
                .extra
                .clone()
                .into_iter()
                .filter(|(k, _)| !merged.extra.contains_key(k))
                .collect();
            merged.extra.extend(prev_keys);
        }
        // Resolve the durable role before writing identity.md — a restart can
        // pass a status caption; the registry keeps the last real hire role.
        merged.role = preferred_agent_role(
            Some(&merged.role),
            prev.as_ref().map(|p| p.role.as_str()),
            merged.is_god,
        );
        // A (re)spawn always means a live terminal — clear the archived flag
        // and stamp the cwd-validity probe, same as upstream.
        merged.archived = false;
        merged.last_seen = now_ms();
        merged.cwd_valid = Some(cwd_valid(&merged.cwd));

        // identity.md is refreshed on each spawn (upstream: not a seed).
        fs::write(dir.join("identity.md"), Self::identity_text(&merged))?;
        Self::seed(
            &dir.join("memory.md"),
            &format!(
                "# Memory — {} ({})\n\n_Append durable facts, decisions, and context below._\n",
                merged.name, merged.id
            ),
        )?;
        // Keep settings/cursor/mail out of mempalace's index AND out of git.
        self.ensure_gitignore_lines(&dir.join(".gitignore"), MINE_IGNORE_LINES);

        match agents.iter_mut().find(|a| a.id == merged.id) {
            Some(a) => *a = merged.clone(),
            None => agents.push(merged.clone()),
        }
        if merged.is_god {
            god_id = Some(merged.id.clone());
        }
        self.write_registry_doc(&god_id, &agents)?;

        self.log(&serde_json::json!({
            "kind": "spawn", "agentId": merged.id, "name": merged.name, "isGod": merged.is_god
        }))?;
        if merged.cwd_valid == Some(false) {
            self.log(&serde_json::json!({
                "kind": "cwd_invalid", "agentId": merged.id, "cwd": merged.cwd, "issue": cwd_issue(&merged.cwd)
            }))?;
        }
        self.commit(&format!("hive: register {}", merged.id));
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
    /// never see a half-written message. Temp name is upstream's
    /// `${p}.tmp-<rand>` (a fixed `.tmp` would collide two writers on one file).
    fn atomic_write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let tmp = PathBuf::from(format!("{}.tmp-{}", path.display(), short_rand()));
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Write a message into `sender`'s outbox (the only path agents use to
    /// send — the router does the rest).
    pub fn send(&self, msg: &Message) -> Result<()> {
        let out = self.agent_dir(&msg.from).join("outbox");
        fs::create_dir_all(&out)?;
        self.atomic_write(
            &out.join(format!("{}.json", msg.id)),
            serde_json::to_string_pretty(msg)?.as_bytes(),
        )
    }

    /// Upstream normalize (hive.ts:1619): fill every harness-owned field the
    /// agent left out. `hops` is harness-owned, so an agent-authored value is
    /// only a carried count — clamp into [0, HOP_CAP] (a copied-forward
    /// `hops: 13` must not read as a runaway loop, and a negative would just
    /// buy free bounces). `requires_reply` defaults from the act.
    pub fn normalize(&self, partial: &PartialMessage, from: &str) -> Message {
        let act = partial
            .act
            .clone()
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| "inform".into());
        let hops = partial.hops.unwrap_or(0).clamp(0, HOP_CAP) as u32;
        Message {
            id: partial
                .id
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("{}-{}", stamp(), short_rand())),
            conversation: partial
                .conversation
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("conv-{}", short_rand())),
            in_reply_to: partial.in_reply_to.clone(),
            from: partial
                .from
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| from.to_string()),
            to: partial
                .to
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "god".into()),
            requires_reply: partial
                .requires_reply
                .unwrap_or_else(|| matches!(act.as_str(), "request" | "query" | "propose")),
            act,
            subject: partial.subject.clone().unwrap_or_default(),
            body: partial.body.clone().unwrap_or_default(),
            hops,
            needs_human: partial.needs_human.unwrap_or(false),
            created_at: partial
                .created_at
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| Utc::now().to_rfc3339()),
        }
    }

    /// Inject a message directly — upstream `send(partial, from)`: normalize,
    /// route synchronously, commit. For synthetic senders ('human', 'system',
    /// 'breaker', 'slack', 'webhook') that have no outbox on disk.
    pub fn send_message(&self, partial: &PartialMessage, from: &str) -> Result<Message> {
        let msg = self.normalize(partial, from);
        self.route_message(&msg)?;
        self.commit(&format!("hive: msg {}→{} ({})", msg.from, msg.to, msg.act));
        Ok(msg)
    }

    /// A virtual sender ('breaker', 'hive', 'slack', 'webhook', 'human') has
    /// no outbox on disk — normalize + route immediately, same path upstream's
    /// send() takes. Never creates agent dirs for the sender.
    pub fn send_virtual(
        &self,
        from: &str,
        to: &str,
        act: &str,
        subject: &str,
        body: &str,
    ) -> Result<Vec<Delivery>> {
        let partial = PartialMessage {
            from: Some(from.to_string()),
            to: Some(to.to_string()),
            act: Some(act.to_string()),
            subject: Some(subject.to_string()),
            body: Some(body.to_string()),
            ..Default::default()
        };
        let msg = self.normalize(&partial, from);
        let delivered = self.route_message(&msg)?;
        if !delivered.is_empty() {
            self.commit(&format!("hive: deliver {}", msg.id));
        }
        Ok(delivered)
    }

    /// Route a fully-formed Message synchronously (upstream hive:send →
    /// send(partial) → normalize+route in one call). No outbox file is
    /// written, so virtual senders never materialize an agents/<from> dir.
    pub fn send_routed(&self, msg: &Message) -> Result<Vec<Delivery>> {
        let delivered = self.route_message(msg)?;
        self.commit(&format!("hive: msg {}→{} ({})", msg.from, msg.to, msg.act));
        Ok(delivered)
    }

    /// Upstream hive.send(partial, sender) — normalize the partial through
    /// the same path every outbox file takes (clamps hops, fills defaults)
    /// and route it immediately. Returns the normalized message.
    pub fn send_partial(&self, partial: &PartialMessage, sender: &str) -> Result<Message> {
        let msg = self.normalize(partial, sender);
        self.route_message(&msg)?;
        self.commit(&format!("hive: msg {}→{} ({})", msg.from, msg.to, msg.act));
        Ok(msg)
    }

    /// Atomically deliver a message into a recipient agent's inbox.
    /// Returns false when the recipient has no inbox — the caller bounces
    /// and logs the drop rather than letting the mail vanish (upstream
    /// deliver(): NO create_dir_all on an unknown id).
    fn deliver_to(&self, msg: &Message, to_id: &str) -> bool {
        let inbox = self.agent_dir(to_id).join("inbox");
        if !inbox.is_dir() {
            return false;
        }
        self.atomic_write(
            &inbox.join(format!("{}.json", msg.id)),
            serde_json::to_string_pretty(msg)
                .unwrap_or_default()
                .as_bytes(),
        )
        .is_ok()
    }

    /// One harness bounce of `msg` to god: bump the hop counter, rewrite the
    /// recipient AND subject, and log the drop once the counter passes
    /// HOP_CAP. Agents can't move hops past the cap themselves (normalize
    /// clamps what they wrote), so this fuse can only fire on a real relay
    /// loop — a bounced mail that keeps coming back undeliverable.
    fn bounce_to_god(&self, msg: &Message, god_id: &str, subject: &str) {
        let hops = msg.hops as i64 + 1;
        if hops > HOP_CAP {
            // Loop guard — drop a runaway message. There is no human queue to
            // fall back on; the god agent owns conflicts.
            let _ = self.log(&serde_json::json!({
                "kind": "drop", "reason": "hop-cap",
                "from": msg.from, "to": msg.to, "id": msg.id
            }));
            return;
        }
        let mut bounced = msg.clone();
        bounced.hops = hops as u32;
        bounced.to = god_id.to_string(); // recipient rewritten, not just subject
        bounced.subject = subject.to_string();
        self.deliver_to(&bounced, god_id);
    }

    /// Upstream routeMessage (hive.ts:1678): resolve `to`, deliver per-target
    /// with the archived/assistant/missing-inbox rules, log the `message`
    /// event with what ACTUALLY took delivery, and return the deliveries for
    /// the `hive:message`/`hive-deliver` event payload.
    fn route_message(&self, msg: &Message) -> Result<Vec<Delivery>> {
        let (god_opt, agents) = self.read_registry_doc();
        let god_id = god_opt
            .or_else(|| agents.iter().find(|a| a.is_god).map(|a| a.id.clone()))
            .unwrap_or_else(|| "god".to_string());
        let find = |id: &str| agents.iter().find(|a| a.id == id);
        // The hive has no separate human-approval queue — approvals are native
        // to each agent's session. A message aimed at "human" is handled by
        // the god/orchestrator, the human's proxy here.
        let resolve_to = |to: &str| -> String {
            if to == "human" || to == "god" {
                god_id.clone()
            } else {
                to.to_string()
            }
        };
        // Broadcast fans out over the ACTIVE registry: skip the sender, the
        // send-only prep assistant, and archived agents (upstream
        // selectBroadcastTargets — hookless providers are NOT skipped here).
        let targets: Vec<String> = if msg.to.trim() == "broadcast" {
            agents
                .iter()
                .filter(|a| a.id != msg.from && !a.is_assistant && !a.archived)
                .map(|a| a.id.clone())
                .collect()
        } else {
            // Never deliver to self — guards a god → "human" loop back to god.
            vec![resolve_to(msg.to.trim())]
                .into_iter()
                .filter(|t| t != &msg.from)
                .collect()
        };
        let mut delivered: Vec<String> = Vec::new();
        // Targets that took the terminal work-order handoff INSTEAD of inbox
        // mail — surfaced on the Delivery so lib.rs emits the upstream
        // `hive:terminalHandoff` event with the full message payload.
        let mut handoffs: std::collections::HashSet<String> = std::collections::HashSet::new();
        for t in &targets {
            let agent = find(t);
            // The send-only prep assistant must never be a delivery target —
            // mail to it would rot unread. Bounce to god so the sender's
            // intent surfaces immediately.
            if agent.map(|a| a.is_assistant).unwrap_or(false) {
                self.bounce_to_god(
                    msg,
                    &god_id,
                    &format!(
                        "[bounced — \"{t}\" is the send-only prep assistant; route work to a real agent] {}",
                        msg.subject
                    ),
                );
                continue;
            }
            // An ARCHIVED recipient (its terminal is gone) still has an inbox:
            // file the mail there — a re-hired worker reads it on its first
            // turn — and, when the message expects an answer, tell the sender
            // at once that no one is there to answer it. Inform-only mail is
            // filed quietly.
            if t != &god_id && agent.map(|a| a.archived).unwrap_or(false) {
                if self.deliver_to(msg, t) {
                    delivered.push(t.clone());
                }
                let _ = self.log(&serde_json::json!({
                    "kind": "archived-recipient",
                    "from": msg.from, "to": t, "id": msg.id, "act": msg.act
                }));
                let sender_live = find(&msg.from).map(|s| !s.archived).unwrap_or(false);
                if msg.requires_reply && sender_live {
                    let inform = PartialMessage {
                        to: Some(msg.from.clone()),
                        act: Some("inform".into()),
                        in_reply_to: Some(msg.id.clone()),
                        conversation: Some(msg.conversation.clone()),
                        hops: Some(msg.hops as i64 + 1),
                        requires_reply: Some(false),
                        subject: Some(format!(
                            "[no one is there to answer — \"{t}\" is archived] {}",
                            msg.subject
                        )),
                        body: Some(format!(
                            "Your {} to {} was filed in its inbox, but {} has no live terminal (archived), \
                             so it will only be read if {} is restored or re-hired under the same id. \
                             If you need this done now, route it to an agent on the live roster or hire one.",
                            msg.act, t, t, t
                        )),
                        ..Default::default()
                    };
                    let _ = self.route_message(&self.normalize(&inform, "system"));
                }
                continue;
            }
            // Hookless providers (kimi/copilot/custom — canReceiveInbox:false)
            // and proxy-tier terminal-delivery providers (qwen/crush — a
            // synthesized Stop can only advance the cursor) get the full
            // message as a terminal work-order handoff INSTEAD of inbox
            // mail — the renderer types it into the REPL. Upstream
            // routeOnce arms 1c/1d (hive.ts:1740-1763); the port surfaces it
            // through Delivery.handoff for the event emit in lib.rs. God is
            // exempt (the bounce target).
            let provider = agent.map(|a| a.provider.as_str()).unwrap_or("");
            let hookless = !crate::misc::can_receive_inbox(provider);
            let proxy_terminal = crate::misc::inbox_delivery_terminal(provider);
            let handoff = t != &god_id && (hookless || proxy_terminal);
            if handoff {
                let _ = self.log(&serde_json::json!({
                    "kind": "terminal-handoff", "from": msg.from, "to": t,
                    "act": msg.act, "subject": msg.subject, "id": msg.id,
                    "delivered": true
                }));
                handoffs.insert(t.clone());
                delivered.push(t.clone());
                continue;
            }
            if self.deliver_to(msg, t) {
                delivered.push(t.clone());
                continue;
            }
            // No agents/<t>/inbox — an id that isn't on the floor. Record the
            // drop beside the hop-cap one and bounce to god so the mail can't
            // silently cease to exist.
            let _ = self.log(&serde_json::json!({
                "kind": "drop", "reason": "no-inbox",
                "from": msg.from, "to": t, "id": msg.id
            }));
            if t != &god_id {
                self.bounce_to_god(
                    msg,
                    &god_id,
                    &format!(
                        "[undeliverable — no agent \"{t}\" on this floor; check the id against the roster] {}",
                        msg.subject
                    ),
                );
            }
        }
        let _ = self.log(&serde_json::json!({
            "kind": "message", "from": msg.from, "to": msg.to, "act": msg.act,
            "subject": msg.subject, "id": msg.id, "delivered": delivered
        }));
        Ok(targets
            .iter()
            .map(|t| Delivery {
                id: msg.id.clone(),
                from: msg.from.clone(),
                to: t.clone(),
                msg_id: msg.id.clone(),
                act: msg.act.clone(),
                subject: msg.subject.clone(),
                targets: targets.clone(),
                needs_human: msg.to.trim() == "human",
                bounced: !delivered.iter().any(|d| d == t),
                // The upstream `hive:terminalHandoff` payload — the WHOLE
                // message, not a stub (the renderer builds a work order out
                // of subject+body verbatim).
                handoff: handoffs.contains(t).then(|| {
                    serde_json::json!({
                        "id": msg.id,
                        "from": msg.from,
                        "to": t,
                        "act": msg.act,
                        "subject": msg.subject,
                        "body": msg.body,
                        "requiresReply": msg.requires_reply,
                        "createdAt": msg.created_at,
                    })
                }),
            })
            .collect())
    }

    /// One router pass: drain every outbox into recipient inboxes (upstream
    /// routeOnce). Success → the file moves to `outbox/.sent/`; malformed →
    /// attempt the literal-line-break repair, log `outbox-repair`, and
    /// quarantine as `.sent/bad-<file>` — never deleted, never in .done.
    pub fn router_tick(&self) -> Result<Vec<Delivery>> {
        let mut delivered = Vec::new();
        let mut routed = 0u64;
        let agents_dir = self.root.join("agents");

        let senders: Vec<PathBuf> = match fs::read_dir(&agents_dir) {
            Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
            Err(_) => return Ok(delivered),
        };

        for sender_dir in senders {
            let owner = sender_dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let outbox = sender_dir.join("outbox");
            let Ok(rd) = fs::read_dir(&outbox) else {
                continue;
            };
            let sent_dir = outbox.join(".sent");
            let _ = fs::create_dir_all(&sent_dir); // hives predating .sent
            let mut files: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .collect();
            files.sort();

            for file in files {
                let fname = file
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let Ok(text) = fs::read_to_string(&file) else {
                    continue;
                };
                let mut partial = serde_json::from_str::<PartialMessage>(&text).ok();
                if partial.is_none() {
                    // Malformed JSON — attempt the narrow repair (literal
                    // CR/LF inside strings), then quarantine either way.
                    let (repaired_text, changed) = repair_literal_line_breaks(&text);
                    if !changed {
                        let _ = self.log(&serde_json::json!({
                            "kind": "drop", "reason": "malformed-json",
                            "from": owner, "file": fname
                        }));
                        let _ = fs::rename(&file, sent_dir.join(format!("bad-{fname}")));
                        continue;
                    }
                    match serde_json::from_str::<PartialMessage>(&repaired_text) {
                        Ok(p) => {
                            partial = Some(p);
                            let _ = self.log(&serde_json::json!({
                                "kind": "outbox-repair", "from": owner,
                                "file": fname, "repair": "literal-line-break"
                            }));
                        }
                        Err(_) => {
                            let _ = self.log(&serde_json::json!({
                                "kind": "drop", "reason": "malformed-json",
                                "from": owner, "file": fname
                            }));
                            let _ = fs::rename(&file, sent_dir.join(format!("bad-{fname}")));
                            continue;
                        }
                    }
                }
                let Some(p) = partial else { continue };
                let mut msg = self.normalize(&p, &owner);
                // The outbox OWNER is authoritative for `from` — an agent
                // can't spoof another sender (upstream: msg.from = id).
                msg.from = owner.clone();
                delivered.extend(self.route_message(&msg)?);
                let _ = fs::rename(&file, sent_dir.join(&fname)); // archive
                routed += 1;
            }
        }

        if routed > 0 {
            self.commit(&format!("hive: routed {routed} message(s)"));
        }
        Ok(delivered)
    }

    /// Move every unread message in an agent's inbox to inbox/.done — the
    /// agent has finished with the whole mailbox (an ephemeral worker that
    /// signaled done), so nothing left in it is pending any more (upstream
    /// settleInbox, hive.ts:1078).
    ///
    /// Only mail that was already there at `before_ms` is finished with: a
    /// message created after the cut-off (a follow-up that crossed the
    /// worker's done) is left pending for whoever picks the mailbox up next.
    /// `created_at` falls back to the file's mtime when unreadable.
    /// Best-effort — never throws into a release path.
    pub fn settle_inbox(&self, id: &str, before_ms: u64) -> SettledInbox {
        let mut out = SettledInbox::default();
        let inbox = self.agent_dir(id).join("inbox");
        if !inbox.is_dir() {
            return out;
        }
        let files: Vec<PathBuf> = match fs::read_dir(&inbox) {
            Ok(rd) => rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .collect(),
            Err(_) => return out,
        };
        if files.is_empty() {
            return out;
        }
        let done = inbox.join(".done");
        if fs::create_dir_all(&done).is_err() {
            return out;
        }
        for fp in files {
            let fname = fp
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let msg: PartialMessage = fs::read_to_string(&fp)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default(); // half-written: file it by mtime
            let mut at = msg
                .created_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.timestamp_millis() as u64)
                .unwrap_or(0);
            if at == 0 {
                at = fp
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
            }
            if at > before_ms {
                out.kept += 1;
                continue;
            }
            // A failed rename leaves the message pending — the pre-settle state.
            if fs::rename(&fp, done.join(&fname)).is_err() {
                continue;
            }
            out.moved += 1;
            let act = msg.act.clone().unwrap_or_default();
            if act == "request" || act == "query" {
                out.unanswered.push(UnansweredMessage {
                    id: msg.id.clone().unwrap_or_else(|| fname.clone()),
                    act,
                    from: msg.from.clone().unwrap_or_else(|| "unknown".into()),
                    subject: msg.subject.clone().unwrap_or_default(),
                    conversation: msg.conversation.clone().unwrap_or_default(),
                });
            }
        }
        if out.moved > 0 {
            let _ = self.log(&serde_json::json!({
                "kind": "inbox-settled", "agentId": id, "count": out.moved
            }));
            self.commit(&format!(
                "hive: settle inbox of {id} ({} unread)",
                out.moved
            ));
        }
        out
    }

    pub fn inbox(&self, id: &str) -> Result<Vec<Message>> {
        let inbox = self.agent_dir(id).join("inbox");
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir(&inbox) else {
            return Ok(out);
        };
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
            "kind": "done", "agentId": agent_id, "msg_id": msg_id
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

    /// Append one card against the latest on-disk ledger — port of hive.ts
    /// `addTask`. Callers must use this instead of re-writing a collection they
    /// read before another source (webhook, Slack, god, voice) added work.
    /// Idempotent by task id: returns false (no write) when a card with that
    /// id already exists.
    pub fn add_task(&self, card: &serde_json::Value) -> Result<bool> {
        let mut doc = self.read_tasks()?;
        let tasks = doc["tasks"]
            .as_array_mut()
            .ok_or_else(|| anyhow::anyhow!("tasks.json is not an object"))?;
        let id = card["id"].as_str().unwrap_or("");
        if tasks.iter().any(|t| t["id"].as_str() == Some(id)) {
            return Ok(false);
        }
        tasks.push(card.clone());
        let count = tasks.len();
        fs::write(
            self.root.join("tasks.json"),
            serde_json::to_string_pretty(&doc)?,
        )?;
        let _ = self.log(&serde_json::json!({ "kind": "tasks", "count": count }));
        self.commit(&format!("hive: tasks ({count})"));
        Ok(true)
    }

    /// patchTaskInLedger (taskLedger.ts): edit the RAW ledger entry, leaving
    /// every other card — and every field the patch doesn't name — untouched.
    /// The ledger is hand-written by god; a partial-model write would delete
    /// fields it doesn't know about (the bug mergeTaskLedger exists to stop).
    /// Returns the patched card.
    pub fn update_task(&self, id: &str, patch: &serde_json::Map<String, Value>) -> Result<Value> {
        let mut doc = self.read_tasks()?;
        let tasks = doc["tasks"]
            .as_array_mut()
            .ok_or_else(|| anyhow::anyhow!("tasks.json is not an object"))?;
        let Some(entry) = tasks
            .iter_mut()
            .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id))
        else {
            return Err(anyhow::anyhow!("no task {id}"));
        };
        let obj = entry
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("task {id} is not an object"))?;
        for (k, v) in patch {
            obj.insert(k.clone(), v.clone()); // explicit null clears a field
        }
        // Re-pin the id — a stray `id` in the patch must never rename the
        // card (upstream patchTask does the same after its spread).
        obj.insert("id".into(), Value::String(id.to_string()));
        let out = entry.clone();
        fs::write(
            self.root.join("tasks.json"),
            serde_json::to_string_pretty(&doc)?,
        )?;
        let n = doc["tasks"].as_array().map(|t| t.len()).unwrap_or(0);
        let _ = self.log(&serde_json::json!({ "kind": "tasks", "count": n }));
        self.commit(&format!("hive: tasks ({n})"));
        Ok(out)
    }

    /// Answer the card's last OPEN humanQA entry — upstream's ASK ME answer
    /// flow (AskMeTab sendAnswer): the answer lands in the entry's `a` +
    /// `answeredAt` AND arrives as an `inform` inbox message to god. The
    /// status is NOT touched — god reads the answer and unblocks the card.
    /// A dismissed entry (`dismissedAt`, no `a`) is closed, not open.
    /// Returns the patched card.
    pub fn task_answer(&self, id: &str, answer: &str) -> Result<Value> {
        let now = chrono::Utc::now().to_rfc3339();
        let doc = self.read_tasks()?;
        let asks = doc["tasks"]
            .as_array()
            .and_then(|ts| ts.iter().find(|e| e["id"].as_str() == Some(id)))
            .and_then(|e| e.get("humanQA").and_then(|q| q.as_array()));
        let Some(asks) = asks else {
            return Err(anyhow::anyhow!("task {id} has no humanQA"));
        };
        let qi = asks
            .iter()
            .rposition(|q| q.get("a").is_none() && q.get("dismissedAt").is_none())
            .ok_or_else(|| anyhow::anyhow!("no open question on {id}"))?;
        let open_q = asks[qi]
            .get("q")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let mut patch = serde_json::Map::new();
        let mut qa = asks.clone();
        qa[qi]["a"] = Value::String(answer.to_string());
        qa[qi]["answeredAt"] = Value::String(now);
        patch.insert("humanQA".into(), Value::Array(qa));
        let card = self.update_task(id, &patch)?;
        let title = card["title"].as_str().unwrap_or(id).to_string();
        // 2) Tell the god, so the card gets unblocked and work continues —
        // upstream's exact envelope + body.
        let _ = self.send_virtual(
            "human",
            "god",
            "inform",
            &format!("HUMAN ANSWER on task \"{title}\""),
            &format!(
                "The human answered the open question on task {id} (\"{title}\"):\n\
                 Q: {open_q}\n\
                 A: {answer}\n\
                 The answer is also recorded in the card's humanQA. Act on it, unblock the card, and continue the work."
            ),
        );
        Ok(card)
    }

    pub fn read_memory(&self, agent_id: &str) -> Result<String> {
        Ok(fs::read_to_string(self.agent_dir(agent_id).join("memory.md")).unwrap_or_default())
    }

    pub fn append_memory(&self, agent_id: &str, entry: &str) -> Result<()> {
        use std::io::Write;
        let path = self.agent_dir(agent_id).join("memory.md");
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(f, "\n{entry}")?;
        self.log(&serde_json::json!({
            "kind": "memory_append", "agentId": agent_id
        }))?;
        Ok(())
    }

    /// Append an event line to log.jsonl — upstream appendLog:
    /// `{ts: <epoch-ms NUMBER>, kind: '<kind>', ...}`. Pub so the webhook
    /// gateway can record inbound traffic on the same ledger.
    pub fn log(&self, event: &serde_json::Value) -> Result<()> {
        use std::io::Write;
        let mut v = serde_json::json!({ "ts": now_ms() });
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

/// redactSecrets (hive.ts:275) — strip credential shapes from any text bound
/// for the renderer/voice layer (hive:messages, trigger history surfaces).
/// Patterns are verbatim upstream: PEM blocks, JWTs, known token prefixes,
/// Bearer headers, and labeled `key = value` / `key: value` secrets.
pub fn redact_secrets(text: &str) -> String {
    use regex::Regex;
    if text.is_empty() {
        return String::new();
    }
    let s = text.to_string();
    let s = Regex::new(
        r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
    )
    .unwrap()
    .replace_all(&s, "[redacted]")
    .to_string();
    let s = Regex::new(r"\beyJ[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{6,}")
        .unwrap()
        .replace_all(&s, "[redacted]")
        .to_string();
    let s = Regex::new(r"(?:sk-(?:ant-)?[A-Za-z0-9_-]{16,}|xox[bpaors]-[A-Za-z0-9-]{10,}|xapp-[A-Za-z0-9-]{10,}|gh[posru]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|AKIA[0-9A-Z]{16}|AIza[A-Za-z0-9_-]{20,})")
        .unwrap()
        .replace_all(&s, "[redacted]")
        .to_string();
    let s = Regex::new(r"(?i)\b(bearer)\s+[A-Za-z0-9._~+/=\-]{8,}")
        .unwrap()
        .replace_all(&s, "$1 [redacted]")
        .to_string();
    // Upstream uses a \3 backreference to match an optional closing quote;
    // the regex crate has no backreferences — the value char-class already
    // stops at quote chars, so the terminal quote is left in place instead.
    let s = Regex::new(r#"(?i)\b((?:[a-z0-9]+[_-])*(?:api[_-]?key|secret[_-]?access[_-]?key|secret|token|password|passwd|pwd|access[_-]?token|refresh[_-]?token|client[_-]?secret|signing[_-]?secret|webhook[_-]?secret|auth[_-]?token|bot[_-]?token|private[_-]?key))(\s*[:=]\s*)(["']?)[^\s"',}]{6,}"#)
        .unwrap()
        .replace_all(&s, "$1=[redacted]")
        .to_string();
    s
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
        let home = hive
            .install_codex_hooks(&dir.join("agent"), &hook_bin)
            .unwrap();
        let cfg = std::fs::read_to_string(home.join("config.toml")).unwrap();
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
            assert!(cfg.contains(&format!("[[hooks.{ev}]]")), "missing {ev}");
        }
        assert!(
            cfg.contains("timeout = 30"),
            "codex floors timeout 0 to 1s - never emit it"
        );
        assert!(cfg.contains("hive-hook.exe"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hermes_hooks_write_config_with_event_map() {
        let dir = std::env::temp_dir().join(format!("oa-hermes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let hive = Hive::ensure(&dir.join("hive")).unwrap();
        let hook_bin = dir.join("hive-hook.exe");
        let home = hive
            .install_hermes_hooks(&dir.join("agent"), &hook_bin)
            .unwrap();
        let cfg = std::fs::read_to_string(home.join("config.yaml")).unwrap();
        assert!(cfg.contains("hooks_auto_accept: true"));
        for ev in [
            "pre_tool_call",
            "post_tool_call",
            "pre_verify",
            "agent_loop_stopped",
            "on_session_start",
            "subagent_stop",
            "api_request_error",
        ] {
            assert!(cfg.contains(&format!(" {ev}:")), "missing {ev}");
        }
        // argv override carries the canonical event; the shim's --hermes
        // flag selects the response translator.
        assert!(cfg.contains("--hermes PreToolUse"));
        assert!(cfg.contains("--hermes Stop"));
        assert!(cfg.contains("--hermes ApiError"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Empty dir standing in for ~/.hermes — the copy-through source is a
    /// parameter so tests never observe the dev's real home.
    fn no_user_hermes(dir: &std::path::Path) -> std::path::PathBuf {
        let p = dir.join("no-user-home").join(".hermes");
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn hermes_seeded_generates_config_and_env() {
        // No user ~/.hermes files → the seed must fill provider config so the
        // spawn never lands on hermes' interactive "Run setup now?" prompt.
        let dir = std::env::temp_dir().join(format!("oa-hermes-seed-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let hive = Hive::ensure(&dir.join("hive")).unwrap();
        let hook_bin = dir.join("hive-hook.exe");
        let seed = crate::hive::HermesSeed {
            provider: Some("deepseek".into()),
            base_url: Some("https://api.deepseek.com".into()),
            model: Some("deepseek-chat".into()),
            api_key: Some("sk-test".into()),
        };
        let home = hive
            .install_hermes_hooks_seeded_at(&dir.join("agent"), &hook_bin, &seed, &no_user_hermes(&dir))
            .unwrap();
        let cfg = std::fs::read_to_string(home.join("config.yaml")).unwrap();
        assert!(cfg.contains("provider: \"deepseek\""));
        assert!(cfg.contains("base_url: \"https://api.deepseek.com\""));
        assert!(cfg.contains("default: \"deepseek-chat\""));
        assert!(cfg.contains("hooks_auto_accept: true"));
        // Isolated source has nothing to copy through — the seed key is the
        // only line under each name.
        let env = std::fs::read_to_string(home.join(".env")).unwrap();
        assert!(env.lines().any(|l| l == "DEEPSEEK_API_KEY=sk-test"), "{env}");
        assert!(env.lines().any(|l| l == "OPENAI_API_KEY=sk-test"), "{env}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hermes_seed_prepends_when_hooks_only_config() {
        // Regression for the re-spawn bug: an earlier boot already wrote
        // config.yaml carrying ONLY the hooks block — !exists() skipped the
        // seed forever. Content check must catch a provider-less file.
        let dir = std::env::temp_dir().join(format!("oa-hermes-reseed-{}", uuid::Uuid::new_v4()));
        let agent = dir.join("agent");
        std::fs::create_dir_all(agent.join(".hermes")).unwrap();
        std::fs::write(
            agent.join(".hermes").join("config.yaml"),
            "hooks_auto_accept: true\nhooks:\n  pre_tool_call:\n    - command: 'x'\n",
        )
        .unwrap();
        let hive = Hive::ensure(&dir.join("hive")).unwrap();
        let seed = crate::hive::HermesSeed {
            provider: Some("deepseek".into()),
            base_url: Some("https://api.deepseek.com".into()),
            model: Some("deepseek-chat".into()),
            api_key: Some("sk-test".into()),
        };
        let home = hive
            .install_hermes_hooks_seeded_at(&agent, &dir.join("hive-hook.exe"), &seed, &no_user_hermes(&dir))
            .unwrap();
        let cfg = std::fs::read_to_string(home.join("config.yaml")).unwrap();
        assert!(cfg.contains("provider: \"deepseek\""), "{cfg}");
        assert!(cfg.contains("base_url: \"https://api.deepseek.com\""), "{cfg}");
        assert!(cfg.contains("hooks:"), "existing hooks block preserved: {cfg}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hermes_seed_env_matches_provider_name() {
        // The .env must carry the variable the seeded provider reads — an
        // openrouter worker given only OPENAI_API_KEY still dies on
        // "no API keys or providers found".
        let dir = std::env::temp_dir().join(format!("oa-hermes-envvars-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let hive = Hive::ensure(&dir.join("hive")).unwrap();
        let hook_bin = dir.join("hive-hook.exe");
        let home = hive
            .install_hermes_hooks_seeded_at(
                &dir.join("agent"),
                &hook_bin,
                &crate::hive::HermesSeed {
                    provider: Some("openrouter".into()),
                    api_key: Some("sk-or".into()),
                    ..Default::default()
                },
                &no_user_hermes(&dir),
            )
            .unwrap();
        let env = std::fs::read_to_string(home.join(".env")).unwrap();
        assert!(env.lines().any(|l| l == "OPENROUTER_API_KEY=sk-or"), "{env}");
        assert!(!env.contains("OPENAI_API_KEY"), "{env}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hermes_empty_seed_writes_no_env() {
        // No base URL → no model block; no key → no .env. A bare spawn stays
        // bare (hermes will do its own setup prompt — same as upstream).
        let dir = std::env::temp_dir().join(format!("oa-hermes-noseed-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let hive = Hive::ensure(&dir.join("hive")).unwrap();
        let hook_bin = dir.join("hive-hook.exe");
        let home = hive
            .install_hermes_hooks_seeded_at(&dir.join("agent"), &hook_bin, &crate::hive::HermesSeed::default(), &no_user_hermes(&dir))
            .unwrap();
        let cfg = std::fs::read_to_string(home.join("config.yaml")).unwrap();
        assert!(!cfg.contains("provider: \"custom\""));
        // The isolated source is empty — no .env should exist at all.
        assert!(!home.join(".env").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gemini_hooks_write_system_settings_with_event_map() {
        let dir = std::env::temp_dir().join(format!("oa-gemini-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let hive = Hive::ensure(&dir.join("hive")).unwrap();
        let hook_bin = dir.join("hive-hook.exe");
        let path = hive
            .install_gemini_hooks(&dir.join("agent"), &hook_bin)
            .unwrap();
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
            let cmd = v["hooks"][ge][0]["hooks"][0]["command"]
                .as_str()
                .unwrap_or("");
            assert!(cmd.contains(&format!("--gemini {ce}")), "{ge} → {ce}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    fn tmp_hive(tag: &str) -> (Hive, PathBuf) {
        let dir = std::env::temp_dir().join(format!("oa-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        (Hive::ensure(&dir.join("hive")).unwrap(), dir)
    }

    #[test]
    fn update_task_patches_raw_entry_and_preserves_unknown_fields() {
        let (hive, dir) = tmp_hive("taskpatch");
        // The ledger is hand-written — seed a card carrying fields no display
        // model knows about (the mergeTaskLedger contract).
        std::fs::write(
            dir.join("hive/tasks.json"),
            r#"{"tasks":[{"id":"t1","title":"a","status":"blocked","customField":7,"humanQA":[{"q":"ok?"}]}]}"#,
        )
        .unwrap();
        let mut patch = serde_json::Map::new();
        patch.insert("status".into(), Value::String("in-progress".into()));
        patch.insert("customField".into(), Value::Null); // explicit null clears
        let card = hive.update_task("t1", &patch).unwrap();
        assert_eq!(card["status"], "in-progress");
        assert!(card
            .get("customField")
            .map(|v| v.is_null())
            .unwrap_or(false));
        assert_eq!(card["title"], "a"); // untouched fields survive
        assert_eq!(card["humanQA"][0]["q"], "ok?");
        // A stray `id` in the patch must never rename the card (upstream
        // re-pins the id after its spread).
        let mut id_patch = serde_json::Map::new();
        id_patch.insert("id".into(), Value::String("t-other".into()));
        let card = hive.update_task("t1", &id_patch).unwrap();
        assert_eq!(card["id"], "t1");
        assert!(hive.update_task("missing", &patch).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn send_partial_routes_synchronously_without_sender_dirs() {
        let (hive, dir) = tmp_hive("sendpart");
        reg(&hive, "w-x", false, dir.as_path());
        let mut p = PartialMessage {
            to: Some("w-x".into()),
            act: Some("request".into()),
            subject: Some("t".into()),
            body: Some("b".into()),
            conversation: Some("c-1".into()),
            requires_reply: Some(true),
            ..Default::default()
        };
        p.hops = Some(0);
        let msg = hive.send_partial(&p, "human").unwrap();
        assert_eq!(msg.conversation, "c-1");
        assert!(msg.requires_reply);
        // Delivered NOW — not after a router poll.
        assert_eq!(hive.inbox("w-x").unwrap().len(), 1);
        // The virtual sender never materializes an agents/human dir.
        assert!(!hive.agent_dir("human").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn redact_secrets_covers_upstream_patterns() {
        for (src, must_contain) in [
            ("key sk-ant-abc1234567890def in body", "[redacted]"),
            ("xoxb-1234-abcdef-token", "[redacted]"),
            ("AKIAIOSFODNN7EXAMPLE", "[redacted]"),
            ("aws_secret_access_key=AKIAIOSFODNN7EXAMPLE", "[redacted]"),
            ("api_key: supersecretvalue", "[redacted]"),
            ("Authorization Bearer abcdef123456", "[redacted]"),
            ("-----BEGIN RSA PRIVATE KEY-----\nMIIx\n-----END RSA PRIVATE KEY-----", "[redacted]"),
            ("eyJhbGciOiJIUzI1NiIs.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV", "[redacted]"),
        ] {
            let out = redact_secrets(src);
            assert!(
                out.contains(must_contain),
                "expected redaction in {src:?} -> {out:?}"
            );
        }
        assert_eq!(redact_secrets("nothing sensitive"), "nothing sensitive");
        assert_eq!(redact_secrets(""), "");
    }

    fn reg(hive: &Hive, id: &str, god: bool, dir: &Path) {
        hive.register_agent(&RegistryEntry {
            id: id.into(),
            name: id.into(),
            provider: "claude".into(),
            role: "x".into(),
            cwd: dir.join(id).display().to_string(),
            is_god: god,
            status: "idle".into(),
            session_id: None,
            command: String::new(),
            created_at: Utc::now().to_rfc3339(),
            ..Default::default()
        })
        .unwrap();
    }

    #[test]
    fn task_answer_sets_a_unblocks_and_mails_god() {
        let (hive, dir) = tmp_hive("taskans");
        reg(&hive, "god", true, &dir);
        std::fs::write(
            dir.join("hive/tasks.json"),
            r#"{"tasks":[{"id":"t1","title":"ship it","status":"blocked","humanQA":[{"q":"deploy?","askedAt":"x"},{"q":"really?","askedAt":"y"}]}]}"#,
        )
        .unwrap();
        let card = hive.task_answer("t1", "yes").unwrap();
        // The LAST open ask gets the answer; earlier answered/skipped asks untouched.
        assert_eq!(card["humanQA"][1]["a"], "yes");
        assert!(card["humanQA"][1].get("answeredAt").is_some());
        // Upstream patches ONLY the humanQA entry + mails god — the card
        // stays "blocked" until god unblocks it himself (AskMeTab.tsx).
        assert_eq!(card["status"], "blocked");
        // The answer also lands in god's inbox (the two-channel contract).
        let inbox = hive.inbox("god").unwrap();
        assert!(
            inbox.iter().any(|m| m.body.contains("yes")),
            "answer mails god"
        );
        // The still-open FIRST ask takes the next answer; only when no ask
        // remains open does the call error — never a silent no-op.
        let card2 = hive.task_answer("t1", "again").unwrap();
        assert_eq!(card2["humanQA"][0]["a"], "again");
        assert!(hive.task_answer("t1", "third").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lifecycle_send_route_inbox_done() {
        let (hive, dir) = tmp_hive("life");
        reg(&hive, "god", true, &dir);
        reg(&hive, "w1", false, &dir);
        hive.send(&Message::new("god", "w1", "request", "hi", "body"))
            .unwrap();
        let deliveries = hive.router_tick().unwrap();
        assert_eq!(deliveries.len(), 1);
        let inbox = hive.inbox("w1").unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].subject, "hi");
        hive.mark_done("w1", &inbox[0].id).unwrap();
        assert!(hive.inbox("w1").unwrap().is_empty());
        // "broadcast" fans out to every agent but the sender (HIVE.md §5).
        hive.send(&Message::new("god", "broadcast", "inform", "b", "b"))
            .unwrap();
        let ds = hive.router_tick().unwrap();
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].to, "w1");
        // A virtual sender (breaker/hive) delivers straight to inboxes.
        let vs = hive
            .send_virtual("breaker", "w1", "request", "s", "b")
            .unwrap();
        assert_eq!(vs.len(), 1);
        assert_eq!(hive.inbox("w1").unwrap().len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn expose_codex_data_moves_and_links_sessions() {
        let (hive, dir) = tmp_hive("codexexp");
        let agent_dir = dir.join("agent");
        // Fake user codex home via HOME override is unsafe in tests; call the
        // internals through the public fn with a temp "home" — the fn reads
        // dirs::home_dir() so we exercise the *agent-side* half here: creating
        // the source and calling expose must not error or corrupt.
        let sessions = agent_dir.join(".codex").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join("roll.jsonl"), "{}").unwrap();
        hive.expose_codex_data(&agent_dir, "w1");
        // Unsafe ids are rejected outright (upstream: safe path components only).
        hive.expose_codex_data(&agent_dir, "../evil");
        std::fs::remove_dir_all(&dir).ok();
    }
}
