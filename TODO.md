# TODO — Remaining upstream surface

Fitur dari aplikasi Electron asli yang belum di-port ke Tauri.

> **Audit ulang (verifikasi source, bukan daftar):** dari 7 item yang
> sebelumnya dicatat "belum/partial", hanya **1 yang benar-benar belum
> di-port**. Sisanya sudah ada — PORTING.md basi, file-nya ada, dan
> fungsionalnya jalan. Detail di bawah.

## Belum di-port (verified)

### 1. Voice Michael — transport WebRTC saja

- Upstream: `src/main/realtimeActions.ts` (823 lines),
  `realtimeCompletionWatcher.ts` (421 lines), `realtimeCost.ts`.
- **Yang sudah ported:**
  - `realtime_actions.rs` (1900 lines) — full port realtimeActions.ts:
    semua verb voice (dispatch, steer, pause/halt/kill, schedule, task
    board) ke backend fns yang sama dengan renderer buttons.
  - `realtime.rs` (716 lines) — full port sessionless half dari
    realtimeCompletionWatcher.ts: detect_completion (task done / inbox
    reply / idle-with-done), pending queue, drain commands.
  - Frontend `realtime/` identik line-per-line: session.ts (499 lines,
    mic + Realtime WebRTC), actions.ts, tools.ts, costStore.ts,
    DevicePicker, CostHud, CompletionToast — semua 8 file.
  - Commands: `realtime_mint_token` (ephemeral key), `realtime_action*`,
    `realtime_drain_completions`, `realtime_wait_for`.
- **Yang belum:** tidak ada. Voice Michael sebenarnya sudah di-port penuh
  — transport WebRTC hidup di renderer (`session.ts`), backend rails dan
  watcher semua ada. Entry ini dicatat "belum" karena PORTING.md basi.

## Belum di-port (verified, masih outstanding)

### 2. Self-updater — signed update feed

- Upstream: `src/main/updater.ts` (524 lines).
- `cmd_update.rs` (163 lines) sudah ported — tapi **notify-only path
  saja**: cek GitHub `releases/latest`, emit `update:status`, semver
  compare. Ini upstream's *fallback* path.
- **Yang belum:** `tauri-plugin-updater` (signed feed). Upstream pakai
  electron-updater dengan signed release. Port butuh:
  - signing keypair (`tauri signer generate`)
  - `tauri-plugin-updater` dep + `updater` config di `tauri.conf.json`
  - `latest.json` artifact di release pipeline
  - capability entry `updater:default`
- UI-nya sudah ada (`ReleaseDrop.tsx`, `UpdateBadge.tsx`, `UpdateToast.tsx`,
  `UpdatesSection.tsx` — identik dengan upstream).

### 3. Analytics — PostHog product analytics

- Upstream: `src/main/analytics.ts` (480 lines).
- `analytics.rs` (581 lines) **sudah ported** — PostHog, anonymous by
  construction (no person profile, random UUID distinct id, per-event
  property allowlist, DO_NOT_TRACK env respected, opt-in via
  `analytics_opt_out` kv — default OFF/opt-out).
- Commands: `analytics_message_sent`, `analytics_set_opt_in`.
- **Ini sudah di-port** — entry sebelumnya di TODO salah.

### 4. MemPalace — semantic memory

- Upstream: `src/main/memory.ts` (418 lines) + `palaceReap.ts`.
- `memory_palace.rs` (779 lines) **sudah ported** — shared palace under
  harnessHome, `MEMPALACE_PALACE_PATH` env per-agent, `mempalace mine`/
  `search`/`wake-up` via CLI, degrades silently jika CLI tidak installed.
- **Ini sudah di-port** — entry sebelumnya di TODO salah.

### 5. Integration broker — per-agent integration secrets

- Upstream: `src/main/integrationBroker.ts` (283 lines) +
  `shared/integrations.ts`.
- `integrations.rs` (1210 lines) **sudah ported** — registry CRUD +
  secret store (OS keychain via `keyring` crate) + loopback broker:
  per-worker capability token → inject real secret → forward upstream.
  Security invariants preserved (127.0.0.1 bind, constant-time token
  compare, no secret leak).
- Commands: `integrations_broker_start/stop/status/token` — semua
  implemented, bukan stub.
- **Ini sudah di-port** — entry sebelumnya di TODO salah.

## Partial — TERNYATA SUDAH FULL

### 6. Telemetry waterfall UI — ✅ done
`ToolWaterfall.tsx` + `useTelemetry.ts` identik dengan upstream;
`telemetry.rs` sudah jalan.

### 7. Multi-window renderer — ✅ done
`CodeEditor.tsx`, Pixi floor, semua komponen ada; `window:newFloor`
sudah buka extra webview.

## Kesimpulan

| Kategori | Jumlah |
|----------|--------|
| Benar-benar belum ported | **1** — self-updater signed feed (notify-only sudah jalan) |
| Sudah ported (salah catat) | 4 — voice Michael, analytics, MemPalace, integration broker |
| Sudah full (salah catat "partial") | 2 — telemetry waterfall, multi-window |

**Effective remaining work: 1 fitur** — wire `tauri-plugin-updater` dengan
signing key + release pipeline.
