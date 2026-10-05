# Agent Office

A desktop "crew office" that runs a team of coding-agent CLIs side by side
and lets them coordinate — a visual floor of AI workers that talk to each
other over a shared file-based message bus, supervised by an orchestrator
("god") agent, while you watch, steer, and approve from one window.

## What it is for

Coding-agent CLIs are single-player tools: you open one terminal, run one
agent, and babysit it until it finishes. Agent Office turns them into a
team. Instead of juggling a dozen terminals, you get a single window —
literally rendered as an office floor — where each agent sits at a desk,
shows its live status, and can be handed work, nudged, corrected, or shut
down.

The point is delegation, not just parallel terminals. You give the
orchestrator ("god") a goal in plain language; it hires agents, splits the
work, routes messages between them, and reports back. You stay in the loop
as the manager: approvals, steering nudges, and human-QA questions reach
you on one screen, wherever they came from.

## What it does

- **The floor.** Spawn any number of agents — each gets a real PTY, a
  workspace directory, an identity, a memory file, and a seat on the office
  floor. Supported providers: Claude Code, Codex, Gemini, Antigravity,
  Grok, Hermes, Pi, OpenCode, Qwen, Crush, OpenClaw — each wired through a
  provider-specific lifecycle bridge (native hooks, bundled plugins, or a
  loopback API proxy) so every agent shares the same status, inbox, HITL
  and cost pipeline.
- **The hive.** Agents message each other through `outbox/` → router →
  `inbox/` inside a versioned `hive/` directory — routing rules cover
  direct, `human` (via god), `broadcast`, and bounce-to-god for unknown
  ids. A shared board (`board.md`), a task ledger (`tasks.json` with
  `humanQA` asks), missions/schedules, and per-agent memory.
- **Work isolation.** An agent can be spawned into a git worktree instead of
  your working directory — dependencies get linked in, the worker's changes
  stay out of your checkout, and the tree is only reclaimed once its work
  is provably integrated. Nothing an agent touches is lost silently.
- **The guardrails.** A circuit breaker watches each agent's token velocity,
  repeated-tool and error-storm signals and can pause, steer, or halt a
  runaway agent. A context-pressure trigger auto-`/compact`s when the
  context window fills. Lifetime spend is folded from `cost-ledger.jsonl`
  across app restarts — you always know what the crew has cost you.
- **Human-in-the-loop.** Agents can pause and ask you a question from inside
  a task (`humanQA`), and it lands as an inbox item you answer from the UI.
  Agents that can't receive mail directly (print-mode CLIs) bounce their
  traffic to the god, so nothing is ever dropped on the floor.
- **The outside world.** HMAC-gated webhooks (review or auto mode) post
  into the floor; a Slack Events-API adapter pipes channel @-mentions into
  the god agent — so "hey @office, deploy the staging branch" becomes an
  agent task; shareable **hire manifests** (`agent-office://hire` /
  file import) pre-fill new hires with an SSRF-gated fetch and a
  default-deny flag allowlist. Every inbound trigger lands in a capped,
  secret-free trigger-history ledger.
- **Memory that survives restarts.** Each agent keeps a `memory.md` that is
  condensed automatically when it grows past budget — an interactive hidden
  Claude session summarises the oldest sections under a
  verify-don't-trust gate (a bad summary is rejected, never written).
- **The exit.** "Closing time" is a graceful shutdown protocol: the god
  broadcasts the close, every live worker parks work and saves memory,
  ACKs are independently verified, and the app only exits once the floor
  is safe. A sleep/wake event hands the floor cleanly across suspend.

## Stack

Tauri v2 (Rust + React/TypeScript/Vite). The backend is one native binary —
no Node runtime to install: lifecycle hooks run through a bundled `hive-hook`
shim, PTYs via `portable-pty`, secrets via the OS credential store,
durable state in SQLite/WAL.

## Run

```bash
npm install
npm run tauri dev        # dev build
npm run tauri build      # release bundle
cargo test --manifest-path src-tauri/Cargo.toml   # backend suite
```
