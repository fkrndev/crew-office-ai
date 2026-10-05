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

Coding-agent CLIs are single-player tools: you open one terminal, run one
agent, and babysit it until it finishes. Agent Office turns them into a
team.

You see every agent on a visual office floor — each one sits at a desk,
shows what it's working on, and reports whether it's idle, busy, stuck, or
waiting on you. You can hire as many agents as you need, mixing engines:
Claude, Codex, Gemini, Grok, Hermes, Pi, OpenCode, Qwen, Crush, Cursor,
Copilot, OpenClaw — they all sit on the same floor and speak the same
language to you.

You don't have to coordinate them by hand. A supervisor agent ("god") takes
your goal in plain language, hires the right agents, splits the work, and
routes messages between them. Agents can hand off tasks, ask each other
for review, or escalate to you when they're unsure — questions land in one
inbox, not scattered across terminals.

Each agent can work inside its own isolated copy of your repo, so parallel
agents never clobber each other's changes or yours. Their work is kept
until it's provably merged — nothing is silently deleted.

Built-in guardrails keep the crew under control: a circuit breaker pauses
runaway agents before they burn your quota, context limits trigger
automatic summarisation, and a running ledger shows the total cost across
sessions. Each agent also keeps its own memory file, condensed
automatically as it grows, so context survives restarts.

The floor connects outward too: webhooks can post tasks from your CI or
other tools, Slack mentions turn into agent work, and shareable "hire
manifests" let someone send you a pre-configured agent that installs in
one click.

When you're done, "closing time" shuts the office down properly: the
supervisor tells everyone to park their work, each agent saves its state
and acknowledges, and the app only exits once every worker is safe —
so you can pick up tomorrow exactly where you left off.

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

## Credits

Office floor tilesets are **Modern Interiors - RPG Tileset [16x16]** by
[LimeZu](https://limezu.itch.io/) — used under the Complete Version licence
(purchased 2026-08-20). Character portraits and walking sprites are
procedurally drawn in-app. See `src/assets/ATTRIBUTION.md` for the full
license details.
