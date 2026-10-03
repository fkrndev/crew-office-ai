# PORTING.md — working notes (Electron → Tauri)

My private working notes for porting `office-agent` (munder-difflin, Electron) to
this Tauri v2 app. Not user docs — a tracker + decision log I keep updating as
the port proceeds.

Source repo studied: `../office-agent` @ rewritten HEAD (`c6f6c7ef`).
Spec docs that matter: `HIVE.md` (hive layout + message schema — port
faithfully), `SPEC.md` (terminal/event plane — mostly renderer), `DESIGN.md`
(visual system — later).

## Architecture mapping

| Electron (`src/main`, Node.js) | Tauri (`src-tauri/src`, Rust) | Status |
|---|---|---|
| `pty.ts`, `ptyEnv.ts` (node-pty) | `pty.rs` (`portable-pty` 0.9) | DONE |
| `ptyEnv.ts` env layering (strip `CLAUDE*` markers, keep CLAUDE_CONFIG_* allowlist, TERM/COLORTERM/FORCE_COLOR, LANG/LC_CTYPE posix) | `ptyenv.rs` | DONE |
| `hive.ts` (~3.4k lines: dirs, router, log, git single-committer, hooks socket, hook shims, proxy sidecar, god prompt) | `hive.rs` (dirs+router+log+git2 commit) — hooks socket & shims = PHASE 2 | PARTIAL |
| `hooks.ts` (HookServer, status, roster context, idle-wake) | `hooks.rs` (TCP loopback) + `src/bin/hive-hook.rs` (native shim — no node dep, replaces cth-hook.cjs + ELECTRON_RUN_AS_NODE wrapper) | DONE (core) |
| `config.ts` (config.json + ScheduledMission) | `config.rs` (JSON in app_data_dir) | DONE |
| `memory.ts`, `palaceReap.ts`, `reflect.ts` (memory.md + mempalace CLI) | `memory.rs` (memory.md read/append only; mempalace/condense = TODO) | PARTIAL |
| `knowledge.ts`, `kg-core.cjs`, `resources/kg.cjs` (KG sidecar) | — decide: Rust port or drop | TODO |
| `slack.ts`, `webhook.ts`, `slack-trigger.cjs` (HTTP ingress + tunnelmole) | `ingress.rs` (axum) + `tauri-plugin-localhost`? tunnel = external proc | TODO |
| `breaker.ts` (circuit breaker: loop/error-storm/velocity/caps/no-progress) | `breaker.rs` + 30s beat in lib.rs (steer/constrain mail, stop kills PTY; hardStop off caps at constrained) | DONE (api_error source pending telemetry) |
| `control.ts` (ControlRegistry: pause/halt/steer/tool gates) | `control.rs` + commands + ControlStrip UI; wired into hooks (halt→continue:false, PreToolUse deny, steer→additionalContext) | DONE |
| `transcript.ts` + `pricing.ts` (Claude transcript usage reconciler, per-model pricing) | `usage.rs` — incremental per-file cache, per-session buckets, per-record pricing | DONE (offline path) |
| `telemetry.ts` (OTLP/HTTP-JSON collector: token/cost metrics, tool_result spans, api_error) | `telemetry.rs` — per-session accum → per-agent sample (LIVE source #1), 200-span ring, tool_decision, api_error→breaker, OTEL_* env on claude spawn | DONE (waterfall UI + grok/costLifetime fallbacks = TODO) |
| `index.ts` syncMissions + `config.ts` mission constants | `missions.rs` + 15s scheduler poll in lib.rs; seed-once flags; quiet-gated heartbeat; compact mission → per-agent idle-boundary `/compact` queue | DONE (weekly schedules = TODO) |
| `updater.ts` (electron-updater) | `tauri-plugin-updater` | TODO |
| `integrations.ts` (Electron safeStorage secrets) | `keyring` crate or `tauri-plugin-stronghold` | TODO |
| `modelCatalog.ts`, `ossModels.ts` | `models.rs` | TODO |
| `cliInstall.ts`, `nodeInstall.ts` (missing-CLI installer ladder) | `install.rs` | TODO |
| `realtime.ts` (OpenAI realtime ephemeral secrets) | keep frontend-side via `@openai/agents-realtime`, mint secret via a Rust cmd | TODO |
| `workerWake.ts` (inbox→wake watchdog, edge-triggered + stall rule) | `workerwake.rs` + 10s beat — all holds ported (booting/mid-turn/boot-grace/hitl/announced/cooldown), hold reporting | DONE |
| `webhook.ts` (secret-gated inbound HTTP → hive) | `webhook.rs` — loopback tiny_http server, constant-time secret + decoy compare (no id enumeration), global/per-endpoint/shared-unknown rate limits, 1MB cap, JSON-Schema validation, auto→god / review→pending-token, GET /status/<token>, hot endpoint swap | DONE (public tunnel = TODO; Slack adapter = TODO) |
| `slack.ts` webhook server, `workerLaunch/hire/hiddenClaude` | later | TODO |
| `useRestoreTeam` / respawn | `respawn_agent` command + ↻ button — registry keeps `command` per agent; claude resumes via `--resume <session_id>` | DONE |
| `git.ts`, `worktreeDeps.ts` (worktrees, fs/git broker for IDE) | `git2` usage in `hive.rs` covers commit; IDE git rails = TODO | PARTIAL |
| `fs.ts` (renderer fs w/ path containment) | `fs.rs` cmds or `tauri-plugin-fs` + explicit scope check | TODO |

| Renderer (React) | Port strategy |
|---|---|
| `PtyTerminalView`/`TerminalView` (xterm.js) | port now — xterm works in webview |
| `agentProvider.ts` presets | ported → `src/providers.ts` (data only; bridge shims deferred) |
| Office floor (Pixi.js scene/*) | `OfficeFloorLite.tsx` — canvas2d render of the SAME office.tmj + tilesets (copied w/ ATTRIBUTION.md); spawn-point seating, status glow, envelope flights. Full Pixi pipeline (walk paths, wander, thought bubbles, Character/portraitArt) = TODO | PARTIAL |
| CommandCenter (tasks/triggers/memory/threads) | PHASE 2 |
| Monaco IDE (`src/ide`, FileTree, git rails) | PHASE 3 — needs `fs.rs`/`git.rs` broker cmds |
| Slack/webhooks/settings/onboarding/updater UI | PHASE 3 |
| i18n (i18next) | TODO — copy `src/renderer/src/i18n` |

## Env contract at agent spawn (from ptyEnv.ts + hive.ts:803)

Layered, bottom→top:
1. inherit, minus `^CLAUDE(CODE|_)` EXCEPT allowlist
   {CLAUDE_CONFIG_DIR, CLAUDE_CODE_OAUTH_TOKEN, CLAUDE_CODE_USE_BEDROCK,
   CLAUDE_CODE_USE_VERTEX} — strips parent-session identity.
2. `PATH`, `TERM=xterm-256color`, `COLORTERM=truecolor`, `FORCE_COLOR=1`,
   posix `LANG`/`LC_CTYPE` (default `en_US.UTF-8`; LC_CTYPE only, never LC_ALL).
3. per-agent: `AGENT_ID`, `AGENT_NAME`, `HIVE_ROOT`, `AGENT_DIR`,
   (+ `HIVE_SOCK`, `HIVE_AUTO_APPROVE`, provider dirs like CODEX_HOME later).

## Hive layout (HIVE.md §3 — keep identical)

```
hive/  PROTOCOL.md registry.json board.md tasks.json log.jsonl
  agents/<id>/ identity.md memory.md inbox/ inbox/.done/ outbox/ cursor.json
```

Message schema (FIPA-lite): id, conversation, in_reply_to, from, to
(`agent.*`/`god`/`broadcast`/`human`→god), act, subject, body, hops,
requires_reply, needs_human, created_at. Router: drain outbox → atomic
rename into recipient inbox → log.jsonl → git commit (single committer,
retry+backoff on index.lock).

## Decisions / deviations

- Router = poll loop (tokio interval 250ms) instead of fs watcher — simpler,
  no `notify` dep, same atomic-rename contract.
- HIVE_SOCK = TCP loopback `127.0.0.1:<dynamic>` (upstream: unix socket /
  named pipe + TCP for proxies). Uniform across OSes; shim is our own binary
  so no third-party client needs the socket path semantics.
- hive-hook = native bin target in the same crate, staged to
  `<harness>/bin/` at startup (upstream staged cth-hook.cjs + hive-node
  wrapper because `node` is absent on the hooks' stripped PATH — a native
  binary removes the dependency entirely). Bundle packaging must ship it
  beside the exe (sidecar/resource — TODO at dist time).
- Spawn: resolve program on PATH (+PATHEXT). `.exe` → direct argv spawn
  (node-pty parity; `--append-system-prompt` text arrives byte-exact).
  `.cmd/.bat` → `cmd /d /s /c` wrap with cmd-quoted args and a sanitized
  single-line prompt (cmd can't carry newlines in argv).
- Claude hiveAware wiring: `settings.json` in AGENT_DIR passed via
  `--settings` (theme:auto, statusLine→hive-hook --status, 9 hook events) +
  `--append-system-prompt`. Non-claude: same injected prompt typed as the
  first turn (uniform 'type-into-tui' — upstream's flag/positional seed
  variants collapse into this; copilot -p print mode excluded anyway).
- Stop→drain: upstream REMOVED forced `{decision:'block'}` at Stop (bypassed
  HITL safety). Ported as guarded idle-wake: on Stop → 1.2s later, if still
  idle AND inbox non-empty → work-order typed into the PTY.
- Not yet ported inside hooks: goal injection (roster store), live OTel
  telemetry (the breaker's sample = cost.jsonl ledger → transcript fallback;
  upstream prefers OTel→transcript in the same order).
- usage.rs skips the legacy POSIX project-key spelling (pre-2026 installs
  only) — noted deviation; add if POSIX support lands.
- Breaker beat inputs: live-PTY agents only (dead entries steer into a
  god-bounce flood — same reasoning as upstream's startup migration);
  `progressing` = hive coordination-file mtimes, `last_work_at` = cwd .git
  markers, `sample` = cumulative cost.jsonl fold per agent.
- Virtual senders ('breaker') deliver directly to inboxes via
  `hive.send_virtual` — no outbox dir for non-agents.
- `git2` (0.21) for single-committer commits; degraded to no-op if repo
  can't init (matching electron's best-effort commit).
- Hook shims (`cth-hook.cjs`, agy/codex/grok/gemini/pi/opencode translators,
  qwen/crush loopback proxy): NOT ported yet — needs its own design in Rust.
  Frontend preset data keeps `bridge` field so the info isn't lost.
- Secrets: plan `keyring` crate (OS credential store) over stronghold for
  BYOK — simpler, matches safeStorage's "OS does the crypto" spirit.
- Updater: `tauri-plugin-updater` + GitHub releases json — later.
- Named pipes vs unix socket for HIVE_SOCK on Windows: original used a
  socket shim; portable-pty+Windows → use `\\.\pipe\hive-<id>` plan, or
  TCP loopback (original's proxy used TCP anyway). Decide in PHASE 2.

## Session log

- 2026-10-04: scaffold + core loop. Rust: pty/ptyenv/hive/config cmds.
  Frontend: providers data, agent list, add-agent, xterm terminal, inbox.
  Verified: cargo check clean, 2/2 unit tests, vite build clean, release-less
  smoke run of target/debug binary (window alive 6s, no panic). Commit 6e89460.
  Deviations found during build: @types/react-dom `^18.3.11` in the source
  package.json never resolves (18.x max is 18.3.7) → pinned `^18.3.0`; Windows
  needs icons/icon.ico for the resource file → generated from icon.png.
- 2026-10-04 (2): phase 2 hooks. `src/bin/hive-hook.rs` native shim +
  `hooks.rs` TCP server (Status/CostSample early-return, record_session,
  statuses working/idle/waiting/compacting, god roster additionalContext,
  guarded idle-wake inbox nudge). Spawn: resolve_program + argv/script/none
  paths, settings.json + --append-system-prompt for claude, typed seed for
  others, HIVE_SOCK + HIVE_AUTO_APPROVE env. Verified: contract e2e via python
  socket harness (envelope in, decision out, no-sock noop), cargo build clean,
  app smoke — hive-hook.exe staged to harness/bin, hive tree created.
  Next up: Pixi floor port OR breaker/control (steer→constrain→stop).
- 2026-10-04 (3): phase 3 guardrails. breaker.rs (ladder + all 6 trip arms:
  repeat-loop hash-key, error storm, per-agent work-token cap, floor $/token
  caps w/ top-spender blame, velocity, no-progress w/ 3 progress clocks +
  debounce + compact exemption), control.rs (pause/gate/steer queue/halt/
  snapshot), hooks wiring (halt→continue:false, PreToolUse deny, merged
  roster+steer additionalContext), 30s beat + send_virtual for breaker mail,
  ControlStrip UI + breaker badges. Also default-run=office-agent (cargo run
  ambiguity after hive-hook bin). Verified: cargo build + vite build clean.
  Next up: Pixi floor port, then missions/heartbeat, telemetry/OTel.
- 2026-10-04 (4): usage reader. usage.rs ports transcript.ts + pricing.ts —
  incremental per-file parse cache (stat-only steady state), per-session
  buckets, per-record model pricing, torn-line tolerance. Beat now feeds
  velocity/cap/no-progress arms real data: ledger → transcript fallback.
  4/4 unit tests.
- 2026-10-04 (5): missions/scheduler. missions.rs + 15s poll (poll design
  replaces upstream's setTimeout/re-arm); ops-standup seeded enabled,
  heartbeat + compact seeded disabled; heartbeat quiet-gated via
  is_floor_quiet (log.jsonl + mailbox mtimes + PTY last_output_at); compact
  queues /compact per agent delivered at idle boundary. Verified: build
  clean, app alive, missions seeded in config.json.
  Next up: Pixi office floor (the big one) or Slack/webhook ingress.
- 2026-10-04 (6): floor LITE. OfficeFloorLite.tsx renders the real office.tmj
  tilemap (34×22, 3 tilesets, gid-mapped draw) on canvas2d — below/above
  z-bands pre-rendered, procedural pixel-people stand-ins, deterministic
  spawn-point seating, status-colored glow, envelope arc animation on
  hive-deliver. Full Pixi pipeline (Character.ts 38KB, portraitArt 33KB,
  wander/paths/bubbles) intentionally deferred — documented in mapping table.
- 2026-10-04 (7): wake watchdog + respawn. workerwake.rs ports the #151
  watchdog (edge-triggered announce, stall rule, all holds, hold reporting)
  closing the mail-arrives-after-Stop stall; respawn_agent reuses the
  registry entry's saved command (new field) with claude --resume; control
  gains pause_auto_delivery + is_auto_delivery_paused parity. Build clean,
  zero warnings; npm build clean.
- 2026-10-04 (8): webhook ingress. webhook.rs on tiny_http — POST /<id> +
  x-md-webhook-secret → god inbox (auto) or pending-token (review);
  GET /status/<token>; constant-time + decoy secret compare, rate limits,
  1MB cap, JSON-Schema check, hot set_endpoints swap. E2E verified: POST
  200+token → delivered to god inbox → /status 'delivered'; wrong secret and
  unknown id both 401. Tunnel (tunnelmole) + Slack adapter + review UI =
  TODO. Deps added: tiny_http, jsonschema.
- 2026-10-04 (9): OTel telemetry. telemetry.rs — embedded OTLP/HTTP-JSON
  collector (tiny_http, dynamic port); token.usage + cost.usage → per-agent
  live sample now FIRST in the beat's source order (otlp→ledger→transcript);
  tool_result spans ring + tool_decision; api_error drained into the
  breaker's storm arm; wake watchdog's telemetry evidence wired; OTEL_* env
  injected for claude (same vars as hive.ts). 6/6 unit tests (2 new ingest
  tests with realistic OTLP bodies).
- 2026-10-04 (10): hermes preset + missions UI. Hermes lands as a
  'type-into-tui' peer (hermes --cli REPL, model via --model) — the original
  goal realized at preset level; deeper lifecycle bridging stays TODO.
  MissionsPanel gives the schedules a real toggle (the missing UI for
  heartbeat/compact/standup). npm build clean.
- 2026-10-04 (11): board/tasks view + secrets. BoardPanel reads god's
  board.md + tasks.json (read-only, 5s refresh). SecretsPanel + keyring
  crate: env keys stored in Windows Credential Manager, injected into every
  spawned agent's env (config keeps only key NAMES — values never on disk).
  Deps added: keyring.
- 2026-10-04 (12): codex hook bridge (installCodexHooks). Per-agent CODEX_HOME
  under the agent dir: auth.json linked, packages junctioned, user's
  config.toml seeded + [[hooks.<Event>]] tables for all 8 events pointing at
  the SAME hive-hook shim (codex's payload is already claude-shaped —
  status/inbox-drain/HITL-deny/wake all ride the existing pipeline). timeout
  = 30 SECONDS — the 0→1s floor bug documented upstream is preserved as a
  comment. 7/7 tests (new: config.toml shape). Rollout-dir exposure = TODO.
- 2026-10-04 (13): hermes hook bridge — the original goal, now hive-aware.
  install_hermes_hooks: per-agent HERMES_HOME (+runtime dir) isolation,
  user's config.yaml/.env copied, hooks_auto_accept set, and a `hooks:`
  block wiring pre_tool_call→PreToolUse, post_tool_call→PostToolUse,
  pre_verify+agent_loop_stopped→Stop, on_session_start→SessionStart,
  subagent_stop→SubagentStop, api_request_error→ApiError (breaker feed —
  hermes has no OTel). Shim gains --hermes translator mode: argv override
  maps event names, responses translate deny/{continue:false}→
  {"decision":"block"} and additionalContext→{"context"}. Hermes' own
  stdin contract was already Claude-shaped — no payload transform needed.
  8/8 tests.
