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
| `hooks.ts` (unix-socket/named-pipe HookServer, Stop→drain, PreToolUse/PostToolUse/Notification status) | `hooks.rs` — not started; needs per-provider shims (cth-hook equivalent, likely a small Rust sidecar binary or inline hook script) | TODO |
| `config.ts` (config.json + ScheduledMission) | `config.rs` (JSON in app_data_dir) | DONE |
| `memory.ts`, `palaceReap.ts`, `reflect.ts` (memory.md + mempalace CLI) | `memory.rs` (memory.md read/append only; mempalace/condense = TODO) | PARTIAL |
| `knowledge.ts`, `kg-core.cjs`, `resources/kg.cjs` (KG sidecar) | — decide: Rust port or drop | TODO |
| `slack.ts`, `webhook.ts`, `slack-trigger.cjs` (HTTP ingress + tunnelmole) | `ingress.rs` (axum) + `tauri-plugin-localhost`? tunnel = external proc | TODO |
| `breaker.ts`, `control.ts` (steer→constrain→stop) | `breaker.rs` | TODO |
| `transcript.ts`, `usage.ts`, `costLifetime.ts`, `telemetry.ts`, `pricing.ts` | `usage.rs` | TODO |
| `updater.ts` (electron-updater) | `tauri-plugin-updater` | TODO |
| `integrations.ts` (Electron safeStorage secrets) | `keyring` crate or `tauri-plugin-stronghold` | TODO |
| `modelCatalog.ts`, `ossModels.ts` | `models.rs` | TODO |
| `cliInstall.ts`, `nodeInstall.ts` (missing-CLI installer ladder) | `install.rs` | TODO |
| `realtime.ts` (OpenAI realtime ephemeral secrets) | keep frontend-side via `@openai/agents-realtime`, mint secret via a Rust cmd | TODO |
| `slack.ts` webhook server, `workerLaunch/hire/hiddenClaude` | later | TODO |
| `git.ts`, `worktreeDeps.ts` (worktrees, fs/git broker for IDE) | `git2` usage in `hive.rs` covers commit; IDE git rails = TODO | PARTIAL |
| `fs.ts` (renderer fs w/ path containment) | `fs.rs` cmds or `tauri-plugin-fs` + explicit scope check | TODO |

| Renderer (React) | Port strategy |
|---|---|
| `PtyTerminalView`/`TerminalView` (xterm.js) | port now — xterm works in webview |
| `agentProvider.ts` presets | ported → `src/providers.ts` (data only; bridge shims deferred) |
| Office floor (Pixi.js scene/*) | PHASE 2 — pure frontend, reusable as-is once IPC shape matches |
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

- 2026-10-04: scaffold + core loop. Rust: pty/ptyenv/hive/config/memory cmds.
  Frontend: providers data, agent list, add-agent, xterm terminal, inbox.
  Verified: cargo check + vite build.
