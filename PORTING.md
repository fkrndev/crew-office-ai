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

| Renderer (React) | Status |
|---|---|
| Entire renderer tree (~157 files, 2.3MB) | VERBATIM port — all of `src/renderer/src/{components,hooks,freeflow,ide,markdown,integrations,realtime,terminal,store,scene,design,i18n}` copied onto a `window.cth` shim (`src/cth.ts`, generated from the upstream preload→channel map; ~160 invoke channels + ~20 event helpers). tsc clean, vite build green. |
| Office floor (Pixi.js scene/*) | FULL Pixi pipeline — OfficeFloor.tsx + Character/portraitArt/TiledMapRenderer/Camera/bubbles/envelopes/themes/pathfinding/cast copied unchanged. Backend emits `hive:message` in the upstream payload shape so handoff envelopes are real routes. |
| `window.cth` bridge | shim maps upstream preload methods onto Tauri `invoke()`/`listen()`; per-id `pty:data:<id>`/`pty:exit:<id>` channels emitted alongside the aggregate events (upstream contract). |
| `store/useHive` wiring | upstream `store.ts` + `useHive.ts` (68KB) verbatim — roster/status/tool streaming flows through the same code paths as Electron. |
| i18n (en/zh-CN/ar, godName interpolation) | verbatim, `@shared` → `src/shared` alias |
| Monaco IDE + CodeMirror editors + xterm WebGL/unicode11 | deps installed (`monaco-editor`, `@monaco-editor/react`, `@uiw/react-codemirror`, `@xterm/addon-{webgl,unicode11}`), wired through the same components |
| `realtime:action{,:confirm,:cancel}` | `realtime_actions.rs` (885-line port) — full verb table (20 verbs), soft/destructive tiers, single-slot pending + distinct-token confirm (bare "yes" rejected), mass-target + god allowlist, SETTING_POLICY table, scoreCard/findCard task matching, per-provider clear verbs, michael-voice attribution + god cross-notify | DONE |
| `hive:terminalHandoff` | routed mail to hookless providers (kimi/copilot/custom) + proxy-tier terminal-delivery (qwen/crush) emits the full-message event INSTEAD of inbox write — upstream routeOnce arms 1c/1d | DONE |
| All `on*` event channels | renamed to the upstream colon-style (`hive:hookEvent`, `hive:contextUpdate`, `hive:agentSpawned/Archived`, `app:closingTime`, `control:breakerState`, `update:status`, `missions:updated`, `triggerHistory:updated`, `config:changed`, `app:closeRequested`, `realtime:completion`, `control:approvalRequest`, `telemetry:event`, `slack:incomingMessage` (wired; the ack flow is backend-side so the event itself is unproduced), `realtime:floorDelta`, `power:resume` (wired, unproduced — platform gap), `mission:autoCompact` + `trigger:context` (wired, unproduced — compacts deliver backend-side), `hive:enqueueToAgent` (dead channel upstream too), `realtime:enqueue`, `hire:import`, `hire:error`) + `pty:data:<id>`/`pty:exit:<id>`/`pty:relaunch:<id>` per-id channels | DONE |
| `realtime:floorDelta` | `realtime.rs FloorWatcher` — 5s roster/task/pty-activity diff, coalesced ≥12s pushes, cleared on disconnect (realtimeFloorWatcher.ts) | DONE |
| Missing-CLI auto-install | `PENDING_INSTALLS` + `misc::install_script` (native → npm → manual-hint ladder per provider) + `pty:relaunch:<id>` on clean installer exit, `no_auto_install` loop guard | DONE |
| Provider context verbs | `misc::context_commands` = total providerAutomation.ts table (/compress for gemini+qwen, focus-arg gating per provider); drives context-trigger, auto-compact missions, compact-queue drain AND voice clear_context | DONE |
| Compact-queue drain | parked compacts fire the provider verb at the next idle transition (hooks emit_status) — the queue previously had no consumer | DONE |
| `power:resume` keepalive | Electron powerMonitor has no Tauri equivalent — channel is wired but nothing emits; dead-PT detection after OS sleep will not self-heal | PLATFORM GAP |
| `munderdifflin://` deep-link hires | Tauri deep-link plugin not wired; `hire:import`/`hire:error` channels exist but only spawn-request files fire | GAP |
| `fs.ts` (renderer fs w/ path containment) | `fs.rs` cmds + explicit scope check — verify scope parity | PARTIAL |

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
- redactSecrets: upstream's closing-quote backreference (\3) has no regex
  crate equivalent — the value char-class already stops at quotes, so an
  optional trailing quote is left in place (cosmetic only).
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
- 2026-10-04 (14): gemini hook bridge. install_gemini_hooks writes
  per-agent system-settings.json (.gemini-hive/, bound via
  GEMINI_CLI_SYSTEM_SETTINGS_PATH); shim gains --gemini mode mapping
  SessionStart/BeforeAgent→UserPromptSubmit/BeforeTool→PreToolUse/
  AfterTool→PostToolUse/AfterAgent→Stop, replies translated to
  {decision:"deny"}/{continue:false}/additionalContext. 9/9 tests.
- 2026-10-04 (15): grok hook bridge. install_grok_hooks writes the GLOBAL
  ~/.grok/hooks/office-agent.json (trusted dir, shim no-ops outside hive
  workers via AGENT_ID/HIVE_SOCK absence); --grok shim mode normalizes
  grok's camelCase payload (hookEventName + names map + sessionId/toolName/
  toolInput/stopHookActive/workspaceRoot). 9/9 tests, build clean.
  Remaining bridges: agy shim, qwen/crush proxies.
- 2026-10-04 (16): agy hook bridge. install_agy_hooks merges the
  'office-agent' group into BOTH ~/.gemini hook surfaces (#49 dual-path);
  --agy shim mode normalizes toolCall/conversationId/workspacePaths input
  and emits ONLY real directives (agy fail-closes on decision-less JSON —
  silence = allow; block→{decision:block,systemMessage}, deny→
  {decision:deny}, context→{systemMessage}). Build clean. Every upstream
  hook-bridge provider now has a port except pi/opencode extensions and
  qwen/crush proxies.
- 2026-10-04 (17): pi + opencode bundled-file bridges (bridges.rs —
  PI_EXTENSION/OPENCODE_PLUGIN JS ported verbatim; per-agent .pi-agent/ +
  .opencode/ dirs, PI_CODING_AGENT_DIR / OPENCODE_CONFIG_DIR env; plugin
  written to both plugin/ and plugins/). Only qwen/crush proxy sidecars
  remain of the upstream bridge family.
- 2026-10-04 (18): proxy sidecar (proxy.rs + ureq) — the loopback reverse
  proxy that makes hookless CLIs hive citizens. Forwards every request
  unchanged (headers minus accept-encoding/host, streaming bodies), tees
  the response into a <=4MB capture, and synthesizes Status (ctx gauge
  via per-model window table), CostSample, per-tool-call PostToolUse and
  the 800ms-debounced Stop — generation-counter cancellation on new
  requests/tool calls, exactly like the JS shim. qwen rides
  OPENAI_BASE_URL→loopback; crush rides install_crush_config's
  CRUSH_GLOBAL_CONFIG (openai wire only — wrong-wire providers stay on
  their real upstream). Anthropic-wire parse path implemented for future
  crush anthropic upstreams. 11/11 tests (SSE emit + idle-stop debounce).
- 2026-10-04 (19): trigger ledger (trigger_history.rs) — append-only
  trigger-history.json, newest-first, cap 500, field-by-field entries
  (never secrets). Webhook auto→'auto-allowed'+taskId, review park→
  'pending' tied to token (hist_id), approve/reject flip the row.
  classify_inbound_kind ported. commands: triggers_list/clear.
- 2026-10-04 (20): skills manager (skills.rs — full port): local
  discovery (claude SKILL.md frontmatter incl. block scalars; opencode/
  codex as plugins; project>user>bundled dedup), catalog README parse
  +24h cache + stale fallback, hardened install (url resolve incl
  officialskills.sh, repo-root SKILL.md locate, 60-file/2MB/depth-5
  caps, post-resolution containment, write-after-resolve-all) +
  root+SKILL.md-proofed uninstall. commands: skills_local/catalog/
  install/uninstall. Bundled skills copied to .claude/skills per spawn.
- 2026-10-04 (21): pricing module separated (the ONE per-model table)
  + codex rollout-dir exposure (sessions/archived_sessions moved under
  ~/.codex/<kind>/office-agent/<hiveId12>/<agentId>, junction back).
- 2026-10-04 (22): TriggersPanel + SkillsPanel UI (upstream TriggersTab /
  SkillsTab in miniature — pending queue approve/reject, ledger browse,
  skills local+catalog+install/uninstall). Context-pressure trigger:
  config.context_trigger (enabled/every_ms/min_context_pct/message) fed
  by the Status hook's context_usage → /compact typed idle-or-queued,
  DEFAULT_COMPACTION_FOCUS as the default focus text.
- 2026-10-04 (23): task ledger write path — hive.update_task
  (patchTaskInLedger: raw-entry patch, unknown fields preserved,
  explicit null clears) + hive.task_answer (last open humanQA gets `a`,
  blocked→in-progress, answer mailed to god). BoardPanel got the status
  dropdown + ASK answer box (board.md stays god-only).
- 2026-10-04 (24): PARITY SUITE — tests/parity.rs (11 observable-
  contract tests: hive init/registry/routing+quarantine/status/cost/
  session; task patch+answer; trigger ledger cycle+corrupt-file;
  pricing goldens; skill parsing) + tests/shim.rs (8 wire-contract
  tests driving the real hive-hook binary vs a fake hook socket:
  claude verbatim, hermes deny/context, agy silence, grok normalize,
  fail-open paths). 47 tests green. Upstream surfaces deliberately not
  ported are enumerated in parity.rs's closing note (Monaco/Pixi/
  keychain/updater/slack are architecture-bound).
- 2026-10-04 (25): cost-ledger.jsonl parity — upstream snake_case schema
  (agent_id/session_id/numeric-ts/input/output/cache/model/usd), usd as
  CUMULATIVE per-session (proxy deltas accumulate onto the session's
  last row so the restart-detection fold stays monotonic), #56 session
  dedup gate, telemetry→ledger append-on-moved in the breaker beat.
  cost_lifetime.rs: CostLedgerTotals incremental fold (segment peaks +
  open peak; counter decrease = app restart) + roster.rs: roster.json
  mirror with append-only backups + empty-first-write guard + archive.
  commands: cost_lifetime, roster_read/write/archive. 52 tests green.
- 2026-10-04 (26): closing_time.rs — the graceful shutdown protocol:
  god brief mail, steer rail for busy agents, router-observer worker
  ACKs, verified COMPLETE (premature conclusion refused, dead workers
  excused), 6min timeout + 2.5s teardown grace → app.exit. Commands:
  closing_time_start/cancel/status; the router loop emits 'closing-time'
  phases.
- 2026-10-04 (27): slack.rs — the Events-API adapter: HMAC-SHA256
  verify over the raw body + 5min replay guard, url_verification
  handshake, bot-user learning, mention/activated-thread trigger gate,
  channel:ts dedup (dual app_mention+message deliveries), subtype/
  self-loop filters, 1MB body cap, post_slack_reply (chat.postMessage,
  CLAUSE-1 explicit-thread guard), keychain-held SLACK_SIGNING_SECRET /
  SLACK_BOT_TOKEN. Commands:
  slack_start/stop/status/reply. 55 tests green. (tunnelmole ported later —
  see session (10).)
- 2026-10-04 (28): hire.rs — shareable hire manifests (spec
  munderdifflin/hire@1): validator ported verbatim (capped strings, agy
  alias, MODEL_RE/FLAG_RE metachar rejection, default-deny safe-flag
  allowlist with flag-value pairing, bundled-skill + MCP-catalog
  allowlists, write/secret → consentRequired), munderdifflin://hire deep
  links, loopback-only http exception, and the SSRF-gated fetcher
  (resolved-IP blocklist incl. v4-mapped v6, CGNAT, 169.254.169.254).
  Commands: hire_validate/fetch/import_file. 59 tests green.
- 2026-10-04 (29): db.rs — PersistStore (harness.db, WAL): versioned
  migrations, kv table (JSON scalars, upsert), command_history (every
  submitted prompt, agent-scoped + substring search with LIKE escaping).
  Commands: kv_get/set, command_history_add/list/search. 60 tests green.
- 2026-10-04 (30): fs_ops.rs — safeResolve port: canonicalize-through-
  deepest-existing-ancestor, lexical escape rejection, dangling-symlink
  lstat walk (the load-bearing case), Windows \\?\ prefix + 8.3-name
  handling; fs_list/fs_read (2MB cap, image preview). git_ops.rs — the
  git pane read surface via git2 (branch/status/ahead-behind/log/
  unintegrated-work guard for worktree removal). Commands:
  git_snapshot/git_log. 63 tests green.
- 2026-10-04 (31): reflect.rs — the memory janitor's CONDENSE half:
  3-region parse (pinned/condensed/recent), merge+rebuild, the
  verify-don't-trust gate (structure, size, pinned-line, recent
  round-trip, not-smaller checks), framed <<<CONDENSED>>>/<<<HOIST>>>/
  <<<END>>> summary protocol + legacy JSON path, headless `claude -p`
  (Haiku, disallowedTools, 180s timeout), backup→summarize→rebuild→
  verify→atomic-swap pipeline. Command: reflect_now. 66 tests green.

## Remaining upstream surface (deliberately unported — external deps)

- realtimeActions.ts + realtimeCompletionWatcher.ts — voice Michael
  (OpenAI Realtime API sessions, realtimePricing). Requires a live
  realtime session; the underlying rails it drives (dispatch, steer,
  pause/halt/kill, schedules, task board) are ALL ported.
- updater.ts + releaseDrop/releaseNotes/updateState — the GitHub-
  releases self-updater. Tauri has its own updater plugin; wire at
  release time (deviation, not omission).
- analytics.ts — PostHog product analytics (outbound; TELEMETRY.md
  opt-out). Deliberately not ported pending the same opt-in/out UX.
- memory.ts — MemPalace CLI semantic memory. External tool dep;
  memory.md append/janitor (reflect) are native.
- integrationBroker.ts + shared/integrations.ts — per-agent integration
  secret broker (Phase 2 foundation upstream).
- hiddenClaude.ts — superseded: reflect.rs runs `claude -p` directly.
- git.ts write half — worktree add/remove, diffs, commit file lists;
  the read surface + unintegrated-work guard are ported (git_ops.rs).
- nodeInstall.ts + cliInstall.ts — N/A: Tauri ships a native binary
  (the porting win — no Node runtime to bootstrap).
- shellEnv.ts — N/A: ptyenv.rs handles env sanitization natively.
- Monaco/CodeEditor/Pixi multi-window renderer surfaces — frontend
  scope; Tauri single-window + OfficeFloorLite covers the floor.
  (window:newFloor now opens extra webview windows.)

## 2026-10-04 (32): full IPC-surface completion sweep

The upstream `ipcMain.handle` registry (~140 channels) is now fully
mirrored by tauri commands. New this pass:

- hive_admin.rs + cmd_hive.rs — hive:addTask/deleteTask/patchAgentRole/
  renameAgent/setArchived/setAgentHold/agentDirectory/agentUsage/
  agentContext/log/messages/textSearch; session:resolveCwd; pty:list/
  redraw; workers:list/stop. Archived/hold flags live on the registry.json
  row (upstream shape; a registry-extra.json sidecar from an earlier
  pass is still read as a fallback).
- git_ops.rs completed + cmd_git.rs — the whole git:* surface: diff,
  branches, checkout (dirty + worktree-occupancy guards), worktrees
  add/remove (unintegrated-work guard), logGraph, commitFiles,
  compareRefs, showFile, mainRepo, isSafeRev. git2 0.21.
- misc.rs + cmd_misc.rs — 42 commands: clipboard, openExternal, reset,
  login item (winreg Run key), notifications, terminal open, tools
  status, fs stat/binary/write/reveal, dialogs (plugin), model catalog
  (embedded), hero payload, provider keys, webhook lifecycle +
  generateSecret + delete, slack config + reply script, telemetry
  snapshot/spans/usage, window:newFloor, config home management +
  token caps, breaker/autoDelivery toggles, triggers/org context.
- kg.rs — knowledge base (BM25-style local index) port of kg-core.cjs,
  byte-identical storage format.
- memory_palace.rs — MemPalace mine/search/wake shell-out, degrades
  cleanly when the CLI is absent; MEMPALACE_* env injected at spawn.
- integrations.rs — integration records + keyring secrets + loopback
  token-gated broker (MD_BROKER_URL/MD_BROKER_TOKEN injected at spawn,
  revoked at teardown).
- freeflow.rs — Groq Whisper STT (hand-rolled multipart).
- github.rs — ciRuns/issues via REST (token from keyring).
- analytics.rs — PostHog capture adapter (event/prop allowlists,
  opt-out, DNT); no-op without a compiled key.
- realtime.rs — completion watcher (card-done + inbox-reply +
  agent-idle predicates) + drain queue + tool catalog; the OpenAI
  Realtime voice transport itself is not portable (websocket session),
  everything it drives is.
- lib.rs — deep-link plugin (agent-office: scheme), close-gating
  (close-requested → confirm → closing-time), analytics init, watcher
  poll on the router tick.
- api.ts — frontend invoke wrappers for the full surface.
- 102 tests green (83 lib + 11 parity + 8 shim); cargo build + tsc clean.

- 2026-10-04 (8): hive.rs full-parity rewrite + sibling fixes. registry.json
  upstream map shape ({godId, agents:{id:entry}}) with array-shape read
  tolerance; RegistryEntry gains archived/onHold/sessionId/cwdValid/
  capabilities + extra flatten spread (survives unknown keys on rewrite);
  merge-over-prev upsert, preferredAgentRole, cwdValidity probe,
  repairLiteralLineBreaks quarantine to outbox/.sent/bad-<f>, routeMessage
  verbatim (assistant bounce, archived-recipient [no one is there] inform,
  no-inbox [undeliverable] bounce, hop-cap drop, delivered-vs-intent log,
  hive:message event payload), rosterContext LIVE ROSTER injection with
  ctx% + ON HOLD rows, inboxBacklog, writeFleetSnapshot, verbatim
  injectedPrompt via PromptOpts (mempalace/kg/runtime/spawn-queue blocks),
  task_answer (humanQA patch + HUMAN ANSWER mail to god, status untouched �
  god unblocks), COMMANDS.md/PROTOCOL.md verbatim. hooks.rs: accepts BOTH
  envelope shapes (hive-hook wrapped {agent_id,event,payload} AND unwrapped
  cth-hook {hook_event_name,...}); injected_context merges roster+goal+steer;
  fleet.json rows with breaker/usd/inboxBacklog/onHold/lastActiveSecAgo.
  bridges.rs JS: HIVE_SOCK parsed as TCP host:port (last-colon split,
  IPv6-safe) � was treating it as an IPC path. cmd_update.rs: notify-only
  updater surface (checkNow/current/openRelease/download/restartAndInstall/
  simulate). git:diff = working-tree single-file diff (rev-range moved to
  git:diffRefs); checkout detach flag; pty:redraw replays retained tail.
  hive_admin: registry patch map-tolerant, text_search = upstream contract
  (board.md+tasks.json+agents/*/memory.md, 3 hits/file, {source,excerpt}).
  108 tests green (89 lib + 11 parity + 8 shim); cargo build + tsc clean.

- 2026-10-05 (9): hive-audit parity fix sweep + ephemeral-worker watcher.
  Routing/registry: archived/onHold moved onto the registry.json row
  (routeMessage was blind to the sidecar); set_archived/set_hold patch the
  row with the lastSeen bump; register_agent now runs on EVERY spawn
  (ensureAgent semantics — respawn re-merges: identity.md refresh, cwd
  revalidation, archived cleared, sessionId/onHold/isAssistant/extra
  preserved, mailbox dirs recreated); spawn status 'idle', kill status
  'gone' (upstream vocabulary). hive:send = send(partial) → {ok,message}
  synchronous route with conversation/in_reply_to/requires_reply/
  needs_human. hive:registry returns the upstream {godId, agents:{}}
  document. hive:messages = voiceMessages contract (floor-wide or per-agent,
  inbox+outbox+.done+.sent, id dedup, direction/owner/archived, secrets
  redacted via ported redactSecrets, default limit 12 / clamp 1–40, id
  lookup mode). hive:agentDirectory = upstream's consolidated
  {godId, agents[]} join (telemetry/breaker/context/hasMemory/inboxBacklog;
  per-agent file listing stays behind an `id` arg). hive:log = upstream
  logTail (oldest→newest, malformed lines as {raw}). Tasks: createdAt ISO
  string, patch can't rename the id, kind:"tasks" log + "hive: tasks (N)"
  commit on every write; log keys unified on "kind".
  Breaker: god + isAssistant excluded from inputs; `progressing` gains the
  telemetry-activity leg (a supervisor whose tool calls never hit its own
  hooks still counts); recordSession fallback + fleet.json refresh on the
  beat; one `hive:message` event per routed message (targets array) emitted
  beside hive-deliver; hive-terminal-handoff emitted for providers without
  canReceiveInbox.
  workers.rs (NEW): the ephemeral-worker subsystem — spawn-requests/ poll at
  the router cadence, gated on config.orchestratorMaySpawn (default off),
  concurrency cap via maxConcurrentWorkers; processSpawnRequest ports
  objective/cwd/bin validation verbatim (safe-name-or-absolute check,
  installed-CLI gate), worker-<reqId> spawn through the normal path,
  hive-agent-spawned broadcast, god dispatch mail with the autonomous
  worker preamble (Slack protocol when the request carries coords) + the
  WORKER COMPLETION suffix; release-on-done with stale-done guard,
  settle_inbox on release (mail-before-done filed, later mail stays
  pending, unanswered non-order mail reported to god with Slack coords),
  token-cap + idle reaps; workers:list = upstream {live, preserved,
  maxWorkers} snapshot (registry join moved to workers:registry).
  112 tests green (93 lib + 11 parity + 8 shim); cargo build + tsc clean.

- 2026-10-05 (10): worktree isolation, proxy teardown, tunnels, realtime
  mint, fleet timer. SpawnArgs.isolate (spawn + worker default true):
  <harness>/worktrees/<slug> via agent_branch_for + add_worktree, slug +
  path-safety verbatim, link_worktreeDeps node_modules junction.
  finalize_teardown in kill_agent: worker → gated finalizeWorkerWorktree
  (unintegrated → PRESERVE + preservedWorktrees entry + informGod;
  clean → remove + register for deferred scratch GC); regular agent →
  immediate remove. gc_preserved_worktrees runs the 60s GC sweep
  (gone-from-disk → scratch reclaim; worktree_integrated_against =
  clean + (ahead==0 OR diff-empty vs base) → remove + reclaim + god mail);
  workers:list surfaces real `preserved` rows; hive-agent-archived emitted
  for dead workers. ProxyBridge gains stop() + per-agent sidecar map —
  teardown kills the qwen/crush loopback listener (upstream
  stopProxyBridge). fleet.json moved to its own 8s timer (upstream
  fleetTimer). realtime:hasKey + realtime:mintToken — GA client_secrets
  mint w/ legacy-sessions 404 fallback, apikey:openai keyring, key never
  crosses IPC. misc::start_tunnel — `npx tunnelmole <port>` URL scrape,
  wired into webhook:start and slack:start; the child dies on stop.
  114 tests green (95 lib + 11 parity + 8 shim).
