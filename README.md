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

## Requirements

Agent Office itself is a self-contained native app. What it needs on your
machine:

- **At least one agent engine** — the CLI of any supported provider
  (Claude Code is the recommended orchestrator and installs itself on
  first run). Other engines (Codex, Gemini, Grok, Hermes, Pi, OpenCode,
  Qwen, Crush, Cursor, Copilot, OpenClaw) are optional extras you can mix
  in later. If a picked engine is missing, the app offers its official
  installer automatically on first spawn.
- **Git** — required for worktree isolation (each agent working in its
  own copy of your repo) and for the hive's internal history.
- **An API key or login for whichever provider you run** — same as using
  the CLI directly (e.g. `claude login`, or an `ANTHROPIC_API_KEY` stored
  in Settings → Connections).

Optional extras that unlock more features:

- **Node.js** — only needed by the engines that are npm-installed
  (OpenCode, Claude on machines without the native build). The app ships
  a checksum-verified installer and sets it up automatically when needed.
- **uv + MemPalace** — meaning-based memory search. Without it agents
  still keep plain markdown notes; with it they can recall by meaning.
  Both install themselves from the missing-tool banner.

## Install

Build the app from source (Rust + Node required for building, not for
running):

```bash
git clone https://github.com/fkrndev/crew-office-ai.git
cd crew-office-ai
npm install
npm run tauri build
```

The installer lands under `src-tauri/target/release/bundle/` — `.msi`/`.nsis`
on Windows, `.dmg`/`.app` on macOS, `.AppImage`/`.deb` on Linux. Run it,
and Agent Office behaves like any installed desktop app.

For day-to-day development use `npm run tauri dev`, which compiles the
backend once and then hot-reloads the UI on every save.

## First-run setup

The first launch walks a four-step onboarding wizard:

1. **Who are you** — technical or not. The copy throughout the app adjusts
   to match, nothing else changes.
2. **Your clone** — pick the engine that runs the orchestrator. Every
   engine the app can actually boot is listed, marked INSTALLED or
   INSTALLS ON FIRST RUN. Claude Code is the default and the safest pick.
   Engines that can only work as floor workers (no inbox channel) are
   shown but not selectable here.
3. **Harness home** — the folder that holds the hive: agent mail, memory,
   tasks, the roster, and config. The default `~/HarnessAgents` works;
   you can have several homes and switch between them later.
4. **Permissions** — notifications, launch at login, keep-awake. All
   optional, all changeable later in Settings.

Finish creates the folder, writes the config, and boots your orchestrator
with the engine you picked. If the engine binary isn't installed yet, the
app runs its installer in front of you and continues when it succeeds.

API keys for voice features or external integrations live in Settings →
Connections and go to the OS credential store — they never touch the
config file.

## Daily use

- **Hire an agent** — `+` on the floor or the sidebar. Pick a provider,
  model, name, workspace, and whether to isolate it in a git worktree.
  Saved **agent templates** make a hire one click for setups you repeat.
- **Give the orchestrator a goal** — type into the god's chat; it plans,
  hires, and delegates. You can also hand a task to a specific agent.
- **Steer without disturbing** — pause tools mid-run, halt after the
  current step, or send a note that lands as context on the next turn.
  Nothing is typed into the agent's terminal.
- **Answer when asked** — agents that need a human (credentials, a
  decision, a review) post to the Ask Me inbox; answers route back to the
  agent that asked.
- **Watch the money** — the Command Center shows per-agent and floor-wide
  spend. Set a per-agent token or USD cap and the breaker pauses that
  agent alone when it goes over; the floor cap pauses the biggest spender.
- **Schedule recurring work** — Schedules (in the Command Center) dispatch
  a prompt to one agent, the orchestrator, or everyone, on an interval or
  a weekly day/time plan.
- **Export a session** — any agent's identity, usage, mail, and terminal
  tail as one Markdown file, useful for audits or handoffs.
- **Closing time** — quitting the app runs a graceful shutdown: agents
  park their work and acknowledge before the process exits.

## Settings tour

- **General** — theme, language (English, 简体中文, العربية), god's name.
- **Prerequisites** — the tool catalog (engines, git, uv, MemPalace) with
  live install status and one-click "set up everything".
- **Agents & Models** — per-provider defaults and model picks.
- **Autonomy & Budgets** — auto mode, circuit-breaker thresholds, spend
  caps (floor-wide and per-agent).
- **Connections** — webhooks, Slack, integrations, API keys (stored in
  the OS credential store).
- **Voice** — Voice Michael / free-flow dictation providers and keys.
- **Memory & Knowledge** — agent memory, MemPalace, knowledge graph.

## Marketplace automation (ShardX)

Agents can drive real logged-in marketplace sessions (Shopee, TikTok Shop,
Lazada Seller Center) through the **ShardX anti-detect browser** — one profile
per account, stable fingerprint + proxy per profile, driven over CDP by an
MCP server.

Setup once per machine:

1. Install the **ShardX Launcher** from
   [github.com/ProxyShard/ShardBrowser/releases](https://github.com/ProxyShard/ShardBrowser/releases)
   (`.msi` / portable `.exe` on Windows, `.AppImage`/`.deb` on Linux,
   `aarch64.dmg` on macOS). First launch downloads the browser engine.
2. In the launcher: **Settings → Automation API** → copy the Bearer token,
   then **Settings → MCP server → Download MCP server**.
3. Create one **profile per account** and log in manually once (OTP/captcha
   need a human; sessions persist after that).
4. In Agent Office, enabling **ShardX Browser** (Settings → MCP, or the
   "extra tools" check in Add Agent) asks for one yes/no consent — the
   bootstrap then starts the launcher, mints the API token from the
   launcher's own settings, and vendors the MCP bridge. No token copying.

Docs: [launcher + Automation API](https://docs.proxyshard.com/eng/tools/shardx-launcher) ·
[MCP server](https://docs.proxyshard.com/eng/shardx-launcher-api/mcp-server) ·
[HTTP API reference](https://github.com/ProxyShard/ShardBrowser/blob/main/openapi.yaml)

Each spawned agent gets its own launcher profile (`office-<agentId>`,
auto-created on spawn) — passed to it via `$SHARDX_PROFILE`, `SHARDX.md` in
its agent dir, and the injected prompt. Profiles are never shared: one
profile is one account identity, and overlapping fingerprints/cookies are
how marketplaces issue linked-account bans. The same applies to social
media accounts — posting/scheduling is covered by the bundled `postiz`
skill (self-hosted Postiz is free; 28+ channels incl. IG/TikTok/FB);
browser-only actions (DMs, comment replies, stories) ride the same
per-agent ShardX profile.

## Stack

Tauri v2 (Rust + React/TypeScript/Vite). The backend is one native binary —
no Node runtime to install: lifecycle hooks run through a bundled `hive-hook`
shim, PTYs via `portable-pty`, secrets via the OS credential store,
durable state in SQLite/WAL.

## Develop

```bash
npm install
npm run tauri dev        # dev build (backend once, UI hot-reload)
npm run tauri build      # release bundle → src-tauri/target/release/bundle/

npm test                 # full suite: renderer tests + backend suite
npm run test:ui          # renderer only (vitest + jsdom, ~20 tests)
npm run test:rs          # backend only (cargo test: lib + parity + shim +
                         #   onboarding + e2e suites, ~135 tests)
```

## Releasing signed updates

Installed copies update themselves through the official Tauri updater. Every
update is verified against a **minisign public key baked into the app**
(`plugins.updater.pubkey` in `src-tauri/tauri.conf.json`); an installer that
is not signed with the matching private key is rejected before anything is
installed. This signature is unrelated to OS code-signing (SmartScreen /
Gatekeeper) and is mandatory.

Behaviour: the app checks `releases/latest/download/latest.json` in the
background (config `autoUpdate`, default on) and shows an update toast. It
never downloads or installs without a click: *download update* fetches and
verifies the installer, *restart to update* installs it. If no signed
`latest.json` exists yet the check falls back to a plain "a newer release
exists" notice with a link to the release page.

**One-time setup**

1. Generate the key pair (empty password shown; pick a real one if you like):

   ```bash
   npx tauri signer generate -w ~/.tauri/agent-office-updater.key
   ```

   The `.pub` file's content is the value of `plugins.updater.pubkey`
   (already set for the key generated for this repo — only replace it if you
   generate a new pair, and then no existing install can update to a build
   signed by the new key).
2. **Back up the private key (`agent-office-updater.key`) somewhere safe and
   offline, plus its password. Never commit it.**
3. In the GitHub repo: *Settings → Secrets and variables → Actions* and add
   - `TAURI_SIGNING_PRIVATE_KEY` — the full content of the private key file
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — its password (empty string if none)

**Cutting a release**

1. Bump `version` in `src-tauri/tauri.conf.json` **and** `src-tauri/Cargo.toml`
   (the workflow refuses a tag that does not match both).
2. `git tag vX.Y.Z && git push origin vX.Y.Z` — `.github/workflows/release.yml`
   builds the Windows NSIS + MSI installers, signs them, and publishes the
   release with `latest.json`. Installed apps pick it up on their next check.

> **WARNING — losing the private key is permanent.** Installs only accept
> updates signed by the key whose public half they ship with. If the private
> key is lost or leaked you can never again update the installs already out
> there (they must be reinstalled by hand from a build with a new public key),
> and anyone holding a leaked key can push malicious updates to every
> install. Back it up, keep it out of the repo, rotate only deliberately.

Local builds: `createUpdaterArtifacts` is on, so `npm run tauri build` needs
the key in the environment (`TAURI_SIGNING_PRIVATE_KEY` /
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`). For an unsigned local installer use
`npm run tauri build -- --config '{"bundle":{"createUpdaterArtifacts":false}}'`.

## Troubleshooting

- **An engine shows "not installed"** — the picker greys out engines the
  app can't install for you; install the CLI by hand, then press "check
  again" in Settings → Prerequisites.
- **An agent spawns but does nothing** — its engine needs a login first
  (run `claude login`, `codex login`, etc. once in a normal terminal).
- **Cost cap tripped unexpectedly** — the circuit breaker blames the
  biggest spender; its row in the Command Center shows why. Clear or
  raise the cap there.
- **The floor is empty after a restart** — the roster mirror lives in
  `<harness home>/roster.json`; the agents' real sessions live in the
  hive and respawn on next boot even if the mirror is lost.

## Credits

Office floor tilesets are **Modern Interiors - RPG Tileset [16x16]** by
[LimeZu](https://limezu.itch.io/) — used under the Complete Version licence
(purchased 2026-08-20). Character portraits and walking sprites are
procedurally drawn in-app. See `src/assets/ATTRIBUTION.md` for the full
license details.
