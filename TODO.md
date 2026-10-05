# TODO — Remaining upstream surface

Fitur dari aplikasi Electron asli yang belum di-port ke Tauri. Dari audit
source-level (PORTING.md), ini adalah satu-satunya gap yang tersisa —
semua IPC handler, provider, dan modul lain sudah punya padanan.

## Belum di-port

### 1. Voice Michael — kontrol floor via suara

- Upstream: `src/main/realtimeActions.ts`, `realtimeCompletionWatcher.ts`,
  `realtimeCost.ts`
- OpenAI Realtime API session (ephemeral token mint → WebSocket) yang bisa
  mengucapkan perintah ke god agent: dispatch, steer, pause/halt/kill,
  bikin schedule, baca task board.
- Semua rails yang disetirnya sudah ported (`realtime_actions.rs` +
  `cmd_realtime.rs` punya `realtime_mint_token`, `realtime_track_dispatch`,
  `realtime_drain_completions`, `realtime_wait_for`, `realtime_actions_status`).
- Yang belum: session realtime-nya sendiri (mic → Realtime API → tool-call
  → action) plus completion watcher yang menunggu agent selesai lalu
  speak-back hasilnya.
- Butuh: live OpenAI realtime key untuk verify end-to-end.

### 2. Self-updater — auto-update dari GitHub releases

- Upstream: `src/main/updater.ts` (+ releaseDrop/releaseNotes/updateState).
- `cmd_update.rs` sudah ada surface-nya, tapi `tauri-plugin-updater` belum
  di-wire. Perlu: signing keypair, `latest.json` release artifact, endpoint
  di `tauri.conf.json`, dan capability entry.

### 3. Analytics — PostHog product analytics

- Upstream: `src/main/analytics.ts` (`analytics:messageSent` IPC ada di shim).
- Deliberately skipped upstream-adjacent: TELEMETRY.md mendokumentasikan
  opt-out, dan port ini belum punya opt-in/out UX-nya. Port begitu UX-nya
  diputuskan.

### 4. MemPalace — semantic memory

- Upstream: `src/main/memory.ts`.
- External CLI dependency (bukan modul native). Yang sudah native:
  memory.md append + janitor/condense (`reflect.rs` + `hidden_claude.rs`).
- Port = wrap binary MemPalace + resolve/install path-nya.

### 5. Integration broker — per-agent integration secrets

- Upstream: `src/main/integrationBroker.ts` + `src/shared/integrations.ts`.
- Phase 2 foundation upstream: broker yang memberi agent akses ke secret
  integrasi (API keys dsb) tanpa expose ke env global.
- `cmd_integrations.rs` sudah punya `integrations_broker_start/stop/
  status/token` stubs — cek apakah itu full port atau hanya surface.

## Partial — sebagian sudah ada

### 6. Telemetry waterfall UI

- Backend `telemetry.rs` sudah: embedded OTLP collector, per-session
  accum → per-agent sample, 200-span ring, tool_decision,
  api_error→breaker, OTEL_* env injection saat spawn claude.
- Belum: waterfall visualization di frontend + fallback path
  grok/costLifetime (lihat PORTING.md line 27).

### 7. Multi-window renderer

- `window:newFloor` sudah membuka extra webview window.
- Belum: Monaco editor dan Pixi office floor penuh di window terpisah
  (upstream punya CodeEditor + Pixi surfaces per-window).

## Tidak perlu di-port (bukan gap)

| Upstream | Kenapa tidak perlu |
|----------|--------------------|
| `nodeInstall.ts` + `cliInstall.ts` | Tauri ship native binary — tidak ada Node runtime yang harus di-bootstrap |
| `shellEnv.ts` | Sudah digantikan: `ptyenv.rs` (env sanitization) + `pty::user_shell_path` (login-shell PATH capture) |
| `hiddenClaude.ts` | **Sudah di-port** → `hidden_claude.rs` (interactive PTY, bukan `claude -p`). Entry ini masih tercatat di PORTING.md bagian lama. |
| `git.ts` write half | Sudah selesai di sweep IPC (worktree add/remove, diffs, commit file lists — `git_ops.rs` + `cmd_git.rs`) |

## Catatan verifikasi

- Audit handler→command terakhir: 149 IPC handler Electron ↔ semua punya
  padanan Tauri; 158 invoke di `cth.ts` ↔ semua resolve ke backend;
  191 command unik ↔ semua terdaftar di `invoke_handler`.
- Test suite: 127 green (104 lib + 15 parity + 8 shim).
- Satu divergensi library yang diketahui: `portable_pty` kill langsung
  SIGKILL/TerminateProcess, tanpa 4s grace period upstream (SIGHUP → wait
  → SIGKILL). Observable outcome sama; hanya polite-shutdown window yang
  hilang.
