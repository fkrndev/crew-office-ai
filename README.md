# Agent Office

A desktop "crew office" that runs a team of coding-agent CLIs side by side
and lets them coordinate — a visual floor of AI workers that talk to each
other over a shared file-based message bus, supervised by an orchestrator
("god") agent, while you watch, steer, and approve from one window.

## What it does

- **The floor.** Spawn any number of agents — each gets a real PTY, a
  workspace directory, an identity, a memory file, and a seat on the office
  floor. Supported providers: Claude Code, Codex, Gemini, Antigravity,
  Grok, Hermes, Pi, OpenCode, Qwen, Crush — each wired through a
  provider-specific lifecycle bridge (native hooks, bundled plugins, or a
  loopback API proxy) so every agent shares the same status, inbox, HITL
  and cost pipeline.
- **The hive.** Agents message each other through `outbox/` → router →
  `inbox/` inside a versioned `hive/` directory — routing rules cover
  direct, `human` (via god), `broadcast`, and bounce-to-god for unknown
  ids. A shared board (`board.md`), a task ledger (`tasks.json` with
  `humanQA` asks), missions/schedules, and per-agent memory.
- **The guardrails.** A circuit breaker watches each agent's token velocity,
  repeated-tool and error-storm signals and can pause, steer, or halt a
  runaway agent. A context-pressure trigger auto-`/compact`s when the
  context window fills. Lifetime spend is folded from `cost-ledger.jsonl`
  across app restarts.
- **The outside world.** HMAC-gated webhooks (review or auto mode) post
  into the floor; a Slack Events-API adapter pipes channel @-mentions into
  the god agent; shareable **hire manifests** (`agent-office://hire` /
  file import) pre-fill new hires with an SSRF-gated fetch and a
  default-deny flag allowlist. Every inbound trigger lands in a capped,
  secret-free trigger-history ledger.
- **The exit.** "Closing time" is a graceful shutdown protocol: the god
  broadcasts the close, every live worker parks work and saves memory,
  ACKs are independently verified, and the app only exits once the floor
  is safe.

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

## Layout

```
src/             React frontend (floor, panels, terminal)
src-tauri/       Rust backend
  src/hive.rs      registry, inboxes, router, board, tasks
  src/hooks.rs     loopback lifecycle-hook server (one socket, all agents)
  src/bin/hive-hook.rs  stdin-JSON → TCP shim the CLIs call
  src/webhook.rs   secret-gated webhook ingress + review tokens
  src/slack.rs     Slack Events-API adapter
  src/skills.rs    bundled/user/project skill discovery + install
  src/trigger_history.rs  capped inbound/outbound trigger ledger
  src/proxy.rs     API proxy for providers without native hooks
  src/breaker.rs   runaway-agent circuit breaker
  src/telemetry.rs embedded OTLP collector (live token/cost feeds)
  src/cost_lifetime.rs   restart-safe lifetime spend fold
  src/closing_time.rs    graceful shutdown protocol
  src/reflect.rs   memory.md condense pipeline (verify-don't-trust)
  src/hire.rs      shareable hire manifests (validator + SSRF-gated fetch)
  src/roster.rs    floor-state mirror with append-only backups
  src/db.rs        SQLite persist store (kv + command history)
  tests/           parity + shim contract suites
```
