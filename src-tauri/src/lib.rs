mod analytics;
pub mod breaker;
mod bridges;
mod closing_time;
mod cmd_analytics;
mod cmd_freeflow;
mod cmd_git;
mod cmd_github;
mod cmd_hive;
mod cmd_integrations;
mod cmd_kg;
mod cmd_memory;
mod cmd_misc;
mod cmd_realtime;
mod cmd_update;
mod codex_remote;
pub mod commands;
pub mod config;
pub mod conversation;
pub mod mcp;
mod control;
pub mod cost_lifetime;
mod db;
pub mod engine_setup;
mod floors;
mod freeflow;
pub mod fs_ops;
mod git_ops;
mod github;
pub mod hidden_claude;
mod hire;
pub mod hive;
pub mod hive_admin;
mod hooks;
pub mod integrations;
pub mod kg;
pub mod memory_palace;
pub mod lock_ext;
mod mic_gate;
pub mod misc;
pub mod missions;
pub mod panic_guard;
mod power_monitor;
pub mod pricing;
pub mod proxy;
mod pty;
mod ptyenv;
mod realtime;
mod realtime_actions;
pub mod reflect;
pub mod roster;
pub mod skills;
mod slack;
mod spawn_ext;
mod telemetry;
pub mod trigger_history;
mod usage;
mod webhook;
mod workers;
mod workerwake;

use crate::lock_ext::LockExt;
use std::time::Duration;

use commands::AppState;

/// Delta gate for the Grok usage-file ledger append (see the breaker beat).
static GROK_LEDGER_GATE: std::sync::LazyLock<usage::CumulativeSampleGate> =
    std::sync::LazyLock::new(usage::CumulativeSampleGate::default);
use tauri::{Emitter, Manager};

/// Dispatch one due mission (index.ts fire()): non-compact kinds mail `body`
/// to `to` from the 'scheduler' virtual sender; a compact mission queues a
/// `/compact` per live agent — typed immediately if idle, else parked in the
/// compact queue for the next idle boundary.
/// emitContextTrigger (index.ts:855): emit `trigger:context` with the CURRENT
/// rule so the renderer acts on what config says NOW, plus the legacy
/// `mission:autoCompact` alias for compact until every consumer moves over.
fn emit_context_trigger(app: &tauri::AppHandle, action: &str) {
    let st = commands::state(app);
    let cfg = st.config.lock_safe();
    let rule = if action == "compact" {
        serde_json::to_value(&cfg.context_trigger.compact).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::to_value(&cfg.context_trigger.clear).unwrap_or_else(|_| serde_json::json!({}))
    };
    drop(cfg);
    let _ = app.emit(
        "trigger:context",
        serde_json::json!({ "action": action, "rule": rule }),
    );
    if action == "compact" {
        let _ = app.emit("mission:autoCompact", serde_json::json!({}));
    }
}

fn fire_mission(app: &tauri::AppHandle, st: &AppState, m: &missions::ScheduledMission) {
    // kind 'shardx' runs a launcher automation project — `body` is the
    // project id. No mail, no agent: replay is the whole point.
    if m.kind.as_deref() == Some("shardx") {
        let out = commands::shardx_fire_automation(st, &m.body);
        let _ = app.emit(
            "hive-log",
            serde_json::json!({ "event": "mission_fired", "id": m.id, "label": m.label, "detail": out }),
        );
        return;
    }
    if m.kind.as_deref() != Some("compact") && !m.to.is_empty() {
        let _ = st
            .hive
            .send_virtual("scheduler", &m.to, "request", &m.label, &m.body);
        // God is the heartbeat's whole point: if the target sits idle, type
        // the wake nudge now rather than waiting for a Stop that isn't coming.
        let target_id = if m.to == "god" {
            st.hive.god_id()
        } else {
            Some(m.to.clone())
        };
        if let Some(tid) = target_id {
            let idle = st
                .hive
                .registry()
                .iter()
                .any(|a| a.id == tid && a.status == "idle");
            if idle {
                let nudge = format!(
                    "[{}] {} — check your inbox for the mission message.",
                    m.label, m.body
                );
                // `tid` is an AGENT id (registry/mission target) — the PTY map
                // is keyed by pty id, so translate first.
                if let Some(pty_id) = st.pty_for_agent(&tid) {
                    pty::write_pty(st, &pty_id, &format!("{nudge}\r"));
                }
            }
        }
    }
    if m.auto_compact || m.kind.as_deref() == Some("compact") {
        // Upstream funnels the legacy per-mission autoCompact flag through the
        // SAME emitContextTrigger path — the renderer applies the per-agent
        // pressure gate + provider verb.
        emit_context_trigger(app, "compact");
    }
    let _ = app.emit(
        "hive-log",
        serde_json::json!({ "event": "mission_fired", "id": m.id, "label": m.label }),
    );
}

/// A fatal startup error: show it natively (the window never opens, so a bare
/// `expect` would abort silently) and hand back the error for `setup` to return.
fn startup_failure(
    app: &tauri::AppHandle,
    what: &str,
    err: impl std::fmt::Display,
) -> Box<dyn std::error::Error> {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
    let msg = format!("Agent Office could not {what}.\n\n{err}");
    log::error!("[startup] {msg}");
    app.dialog()
        .message(msg.clone())
        .title("Agent Office failed to start")
        .kind(MessageDialogKind::Error)
        .blocking_show();
    msg.into()
}

/// Kill every live agent PTY so quitting never orphans the agent CLIs (and
/// their MCP/helper process trees). Idempotent: the map is drained.
///
/// Order matters (upstream killAll, 276f782a): on Windows every agent's process
/// TREE is swept synchronously FIRST — taskkill /T /F while the roots are still
/// alive, since the app exits within ~a second and a deferred sweep would never
/// run (a Windows child is not killed when its parent exits) — and only then
/// are the PTY roots closed. The Slack public-tunnel child is stopped here
/// too, so it doesn't outlive the app either.
fn kill_all_ptys(app: &tauri::AppHandle) {
    let Some(st) = app.try_state::<AppState>() else {
        return;
    };
    let mut handles: Vec<pty::PtyHandle> = st
        .ptys
        .lock_safe()
        .drain()
        .map(|(_, h)| h)
        .collect();
    let pids: Vec<u32> = handles.iter().filter_map(|h| h.pid).collect();
    pty::kill_trees(&pids);
    for h in handles.iter_mut() {
        let _ = h.kill_root();
    }
    // The tunnel (npx.cmd shim → node grandchild) — tree-kill like any child.
    let tunnel = st.slack_tunnel.lock_safe().take();
    if let Some((_url, mut child)) = tunnel {
        misc::kill_process_tree(&mut child);
    }
    power_monitor::release_keep_awake();
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// bootstrapHiveServices (index.ts) — the once-per-launch hive work that must
/// run right after state exists and BEFORE the renderer can respawn anything.
fn bootstrap_hive_services(app: &tauri::AppHandle) {
    use tauri::Emitter;
    let st = app.state::<AppState>();

    // The hive's renderer event sink (upstream's `emit` ctor arg): reports
    // whether the event reached a live window, which is what lets a terminal
    // handoff that cannot be delivered bounce to god instead of vanishing.
    {
        let h = app.clone();
        st.hive.set_emitter(move |channel, payload| {
            h.get_webview_window("main").is_some() && h.emit(channel, payload).is_ok()
        });
    }

    // refreshGeneratedDocs: PROTOCOL.md / COMMANDS.md follow app updates. Only
    // here — runtime paths never rewrite them.
    st.hive.refresh_generated_docs();

    // Hook-endpoint bind outcome → log.jsonl (a packaged app has no console).
    hooks::log_bind_outcome(&st.hive);

    // `app-start` marker: the god prompt promises one on every launch, and it
    // is how a relaunch or a build switch is spotted. WHICH bundle, not just
    // which version — a stale copy and a fresh one report the same version.
    let exe = std::env::current_exe().ok();
    let _ = st.hive.log(&serde_json::json!({
        "kind": "app-start",
        "version": env!("CARGO_PKG_VERSION"),
        "packaged": !cfg!(debug_assertions),
        "appPath": exe.as_ref().and_then(|p| p.parent()).map(|p| p.display().to_string()),
        "exePath": exe.as_ref().map(|p| p.display().to_string()),
        "platform": if cfg!(windows) { "win32" } else if cfg!(target_os = "macos") { "darwin" } else { "linux" },
    }));

    // archiveOrphanedAgents (#57/#58): no agent owns a PTY yet, so every
    // archived:false non-god entry is a stale carry-over from a session that
    // quit without archiving — archive it so it leaves broadcast fan-out,
    // fleet.json and god's roster. A respawn clears the flag again.
    for id in st.hive.archive_orphaned_agents(&st.live_agent_ids()) {
        log::info!("[migration] archived orphaned agent (no live PTY): {id}");
    }

    // Codex rollout dirs of agents that predate the exposure (it used to
    // happen only at spawn).
    st.hive.reexpose_codex_sessions();

    // replaceAutoDeliveryPauses: the operator's per-agent delivery holds
    // survive a restart (persist kv, written by control_auto_delivery).
    let paused: Vec<String> = st
        .persist
        .lock_safe()
        .get_kv("auto_delivery_paused_agents")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    st.control.lock_safe().replace_auto_delivery_pauses(paused);

    // maint-1 retirement carry-over: a retired compact-maintenance mission's
    // elapsed time moves to the context trigger's cadence, so retiring it
    // mid-cycle doesn't restart a 2h clock from zero.
    let retired = std::mem::take(&mut st.config.lock_safe().retired_compact_last_fired);
    if retired > 0 {
        let mut m = st.context_last_compact.lock_safe();
        m.insert("compact".into(), retired);
        let _ = st.persist.lock_safe().set_kv(
            "context:lastRun",
            &serde_json::to_value(&*m).unwrap_or_else(|_| serde_json::json!({})),
        );
    }
}

/// Epoch-ms of the last UNSAVED resize/move (0 = nothing pending) — the clock
/// of the window-bounds debounce.
static WINDOW_DIRTY_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Persist the main window's geometry (kv `window.bounds`, physical pixels) —
/// never while minimized / maximized / fullscreen, so a restore can't save the
/// minimized sentinel position or the fullscreen rect.
fn save_window_bounds(win: &tauri::WebviewWindow) {
    if win.is_minimized().unwrap_or(false)
        || win.is_maximized().unwrap_or(false)
        || win.is_fullscreen().unwrap_or(false)
    {
        return;
    }
    // inner_size pairs with set_size on restore (outer_position with
    // set_position) — the frame never accumulates across launches.
    let (Ok(pos), Ok(size)) = (win.outer_position(), win.inner_size()) else {
        return;
    };
    let st = win.state::<AppState>();
    st.persist.lock_safe().set_kv(
        db::WINDOW_BOUNDS_KEY,
        &serde_json::json!({
            "x": pos.x, "y": pos.y, "width": size.width, "height": size.height
        }),
    );
}

/// Restore the saved geometry, clamped onto a connected monitor (a window
/// parked on an unplugged display must not open off-screen). No saved bounds
/// → the configured default window is left alone.
fn restore_window_bounds(win: &tauri::WebviewWindow) {
    use tauri::{PhysicalPosition, PhysicalSize};
    let st = win.state::<AppState>();
    let Some(raw) = st.persist.lock_safe().get_kv(db::WINDOW_BOUNDS_KEY) else {
        return;
    };
    let areas: Vec<(i32, i32, u32, u32)> = win
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let w = m.work_area();
            (w.position.x, w.position.y, w.size.width, w.size.height)
        })
        .collect();
    // tauri.conf.json's minWidth/minHeight are logical.
    let sf = win.scale_factor().unwrap_or(1.0);
    let min = ((960.0 * sf) as u32, (600.0 * sf) as u32);
    let Some(b) = db::clamp_window_bounds(&raw, &areas, min) else {
        return;
    };
    let _ = win.set_size(PhysicalSize::new(b.width, b.height));
    match (b.x, b.y) {
        (Some(x), Some(y)) => {
            let _ = win.set_position(PhysicalPosition::new(x, y));
        }
        _ => {
            let _ = win.center();
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The release profile unwinds instead of aborting (Cargo.toml), so a stray
    // panic ends only its own thread/task. Record every one — message,
    // location, thread, backtrace — in `<app data dir>/panic.log` from the very
    // first moment. The context is built first only to learn the bundle
    // identifier (app_data_dir = <OS data dir>/<identifier>).
    let context = tauri::generate_context!();
    panic_guard::install_panic_hook(
        dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(&context.config().identifier)
            .join("panic.log"),
    );
    tauri::Builder::default()
        // Microphone only while Free Flow / a realtime voice session is live;
        // every other permission kind keeps its default behaviour (mic_gate.rs).
        .on_permission_request(mic_gate::on_permission_request)
        // ...and WebView2 must not remember the answer (see mic_gate.rs).
        .plugin(mic_gate::plugin())
        // Upstream requestSingleInstanceLock — a second process launch hands
        // its argv to the running instance (Windows/Linux deep links arrive
        // exactly that way) and focuses the main window, never spawning a
        // second harness on the same hive.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| panic_guard::guarded_unit("single-instance", || {
            use tauri::{Emitter, Manager};
            for arg in &argv {
                if let Some(src) = crate::hire::parse_hire_deep_link(arg) {
                    let home = app
                        .state::<AppState>()
                        .hive
                        .root
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_default();
                    commands::park_pending_hire(&home, &src);
                    let _ = app.emit("hire-pending", &src);
                }
            }
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        })))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        // Signed auto-update (desktop-only app): pubkey + endpoints live in
        // tauri.conf.json `plugins.updater`; driven from cmd_update.rs.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // Deep links only fire for installed apps on Windows/Linux —
            // register_all makes dev builds handle
            // agent-office:// too (official plugin recipe).
            #[cfg(any(target_os = "linux", all(debug_assertions, windows)))]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                if let Err(e) = app.deep_link().register_all() {
                    log::warn!("[deep-link] register_all failed: {e}");
                }
            }
            let app_data = app
                .path()
                .app_data_dir()
                .map_err(|e| startup_failure(app.handle(), "locate the app data directory", e))?;
            // Authoritative location of panic.log (matches the early guess).
            panic_guard::set_log_path(app_data.join("panic.log"));
            let default_home = app_data.join("harness");
            // config_change_home's pointer: `harnessHome` in the FIXED
            // default-home config.json — the only file readable before we
            // know which home is active. A custom home survives restarts
            // this way (upstream's harnessHome config key).
            let harness_home = crate::config::ConfigStore::new(default_home.clone())
                .raw_value()
                .get("harnessHome")
                .and_then(|v| v.as_str())
                .map(|s| crate::misc::expand_tilde(s))
                .map(std::path::PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or(default_home);
            // HookServer first: its TCP addr is injected as HIVE_SOCK into every
            // agent spawn. Connections arriving before state is managed harmlessly
            // answer {} (none can — no agents exist yet).
            let server = hooks::HookServer::start(&app.handle())
                .map_err(|e| startup_failure(app.handle(), "start the hook server", e))?;
            let mut state = AppState::new(harness_home.clone())
                .map_err(|e| startup_failure(app.handle(), "initialise app state", e))?;
            state.sock_addr = Some(server.addr.clone());
            app.manage(state);
            app.manage(cmd_update::UpdateState::default());
            // Upstream a84efe56: a crash mid-voice-session must not boot with
            // the mic gate pre-opened.
            mic_gate::reset_realtime_voice_flag(app.handle());
            bootstrap_hive_services(app.handle());

            // bootstrapHiveServices equivalents: the integration broker +
            // the memory palace mine loop start implicitly with the app.
            let _ = crate::integrations::BROKER.start(&harness_home, 0);
            crate::memory_palace::instance(&harness_home).start();
            // analytics.ts init — first_run / app_launched / update_applied
            // (no-op when the PostHog key isn't compiled in or opted out).
            crate::analytics::global().init(
                &app.handle(),
                &harness_home,
                env!("CARGO_PKG_VERSION"),
            );

            // Close gating (app:cancelClose/app:confirmClose) — a close with
            // live workers is held at the gate until the user confirms the
            // closing-time protocol; an empty floor exits normally.
            if let Some(win) = app.get_webview_window("main") {
                // Window geometry (upstream 1c980498): restore the last
                // size/position (clamped to a visible monitor), then persist
                // on resize/move (debounced below) and on close.
                restore_window_bounds(&win);
                let win2 = win.clone();
                // Event-loop thread: a panic would unwind out of tao's message
                // loop and end the whole app, so it is contained per event.
                win.on_window_event(move |ev| panic_guard::guarded_unit("window-event", || match ev {
                    tauri::WindowEvent::Resized(_) | tauri::WindowEvent::Moved(_) => {
                        WINDOW_DIRTY_AT.store(now_ms(), std::sync::atomic::Ordering::Relaxed);
                    }
                    tauri::WindowEvent::CloseRequested { api, .. } => {
                        save_window_bounds(&win2);
                        let st = win2.state::<AppState>();
                        let live = !st.ptys.lock_safe().is_empty();
                        if live && !st.closing.is_active() {
                            api.prevent_close();
                            // Upstream 'app:closeRequested' carries the live
                            // PTY count — the QuitWarningModal renders it.
                            let count = st.ptys.lock_safe().len();
                            // Main only: a floor's own close prompt is a
                            // different event (floors.rs).
                            floors::emit_to_owner(
                                win2.app_handle(),
                                floors::MAIN_LABEL,
                                "app:closeRequested",
                                serde_json::json!({ "ptyCount": count }),
                            );
                        }
                    }
                    // Closing `main` ends the app even while floors are open
                    // (the CloseRequested gate above already held it for live
                    // PTYs). Without a floor Tauri exits on its own.
                    tauri::WindowEvent::Destroyed => {
                        let app = win2.app_handle();
                        if app
                            .webview_windows()
                            .keys()
                            .any(|l| floors::is_floor_label(l))
                        {
                            app.exit(0);
                        }
                    }
                    _ => {}
                }));
                // Trailing-edge debounce for the move/resize flood: save once
                // the window has been still for ~400ms.
                let win3 = win.clone();
                panic_guard::supervise_arg("window-bounds-saver", win3, |win3| async move {
                    let mut tick = tokio::time::interval(Duration::from_millis(200));
                    loop {
                        tick.tick().await;
                        let at = WINDOW_DIRTY_AT.load(std::sync::atomic::Ordering::Relaxed);
                        if at != 0 && now_ms().saturating_sub(at) >= 400 {
                            // Only clear the mark we observed — an event that
                            // landed meanwhile keeps its own (later) stamp.
                            let _ = WINDOW_DIRTY_AT.compare_exchange(
                                at,
                                0,
                                std::sync::atomic::Ordering::Relaxed,
                                std::sync::atomic::Ordering::Relaxed,
                            );
                            save_window_bounds(&win3);
                        }
                    }
                });
            }

            // Cold-start deep link (upstream: the link rides in on OUR argv
            // when the OS launches a fresh process for the scheme).
            for arg in std::env::args() {
                if let Some(src) = crate::hire::parse_hire_deep_link(&arg) {
                    let home = app
                        .state::<AppState>()
                        .hive
                        .root
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_default();
                    commands::park_pending_hire(&home, &src);
                    let _ = app.emit("hire-pending", &src);
                }
            }

            // agent-office://hire?src=... deep links: park the source for the UI to
            // drain through hire:drainPending.
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let handle3 = app.handle().clone();
                app.deep_link().on_open_url(move |event| panic_guard::guarded_unit("deep-link", || {
                    for url in event.urls() {
                        if let Some(src) = crate::hire::parse_hire_deep_link(url.as_str()) {
                            let home = handle3
                                .state::<AppState>()
                                .hive
                                .root
                                .parent()
                                .map(|p| p.to_path_buf())
                                .unwrap_or_default();
                            commands::park_pending_hire(&home, &src);
                            let _ = handle3.emit("hire-pending", &src);
                        }
                    }
                }));
            }

            // (The unauthenticated sync-IPC loopback server was removed — it
            // served the OS clipboard to any website via CORS `*`. The
            // `sync_port` command stays and reports 0.)

            // Router loop — the Electron version watched outboxes via chokidar;
            // a 250ms poll keeps the same deliver contract with no watcher dep.
            let handle = app.handle().clone();
            panic_guard::supervise_arg("router-loop", handle, |handle| async move {
                let mut tick = tokio::time::interval(Duration::from_millis(250));
                let mut closing_nudge_ticks = 0u32;
                // Archive/log retention: once at startup (first tick), then at
                // most every 10 min — off the async thread, never per tick.
                let mut maintain_ticks = 0u32;
                loop {
                    tick.tick().await;
                    let st = handle.state::<AppState>();
                    if maintain_ticks == 0 {
                        let h = handle.clone();
                        tauri::async_runtime::spawn_blocking(move || {
                            h.state::<AppState>().hive.maintain()
                        });
                    }
                    maintain_ticks = (maintain_ticks + 1) % 2400; // 2400 x 250ms
                    // realtimeCompletionWatcher — poll once per tick so
                    // dispatch completions surface even without a waiter.
                    panic_guard::guarded("realtime-watcher", || {
                        crate::realtime::WATCHER
                            .lock_safe()
                            .poll(&st.hive, Some(&handle))
                    });
                    match st.hive.router_tick() {
                        Ok(deliveries) => {
                            // Closing-time observer: routed subjects carry
                            // the worker ACKs + god's COMPLETE verdict.
                            if st.closing.is_active() {
                                // The kickoff nudge is one-shot at press
                                // time — a god that was BUSY then and goes
                                // idle now would sit on the brief forever
                                // (the mail + steer only land at a hook
                                // boundary an idle REPL never reaches).
                                // Re-nudge every ~12s while the protocol is
                                // open; nudge_idle_god self-guards on a
                                // confirmed-idle god, so a working
                                // orchestrator is never typed into.
                                closing_nudge_ticks += 1;
                                if closing_nudge_ticks >= 48 {
                                    closing_nudge_ticks = 0;
                                    commands::nudge_idle_god(st.inner());
                                }
                                // Agent-namespace live set (the pty map keys
                                // are renderer `pty-<agent>` ids).
                                let live: std::collections::HashSet<String> =
                                    st.live_agent_ids();
                                // Fed straight from the Delivery (from/subject/to
                                // ride on it): an inbox lookup would miss god →
                                // "human" (the CLOSING-TIME-COMPLETE verdict —
                                // the human has no inbox) and any mail the
                                // recipient already filed within this tick.
                                for d in &deliveries {
                                    if let Some(phase) = st.closing.on_routed(
                                        &st.hive,
                                        &live,
                                        &d.from,
                                        &d.subject,
                                        &[d.to.clone()],
                                    ) {
                                        commands::emit_closing(&handle, phase);
                                    }
                                }
                                if st.closing.concluded() {
                                    commands::emit_closing(&handle, "complete");
                                    let h2 = handle.clone();
                                    tauri::async_runtime::spawn(async move {
                                        tokio::time::sleep(Duration::from_millis(
                                            closing_time::TEARDOWN_GRACE_MS,
                                        ))
                                        .await;
                                        h2.exit(0);
                                    });
                                }
                            }
                            if let Some(phase) = st.closing.poll_timeout() {
                                commands::emit_closing(&handle, phase);
                            }
                            // Upstream emits ONE `hive:message` per routed
                            // message with the full targets array (the
                            // renderer's feed contract); `hive-deliver`
                            // stays as this port's per-target detail event.
                            {
                                let mut seen: std::collections::HashSet<String> =
                                    std::collections::HashSet::new();
                                for d in &deliveries {
                                    // One upstream `hive:message` per routed
                                    // message — the floor flies an envelope
                                    // per resolved target (emitMessage).
                                    if seen.insert(d.msg_id.clone()) {
                                        let _ = handle.emit(
                                            "hive:message",
                                            serde_json::json!({
                                                "id": d.msg_id,
                                                "from": d.from,
                                                "to": d.to,
                                                "act": d.act,
                                                "subject": d.subject,
                                                "targets": d.targets,
                                                "needsHuman": d.needs_human,
                                            }),
                                        );
                                    }
                                }
                            }
                            for d in deliveries {
                                // (The terminal work-order handoff event —
                                // `hive:terminalHandoff` — is emitted inside
                                // route_message through the Hive's event
                                // sink, which bounces the mail to god when no
                                // window is there to receive it.)
                                let _ = handle.emit("hive-deliver", &d);
                            }
                        }
                        Err(e) => {
                            let _ = handle.emit(
                                "hive-log",
                                serde_json::json!({ "event": "router_error", "error": format!("{e:#}") }),
                            );
                        }
                    }
                    // Floor delta watcher (realtimeFloorWatcher.ts) — emits
                    // `realtime:floorDelta` while a voice session is live;
                    // no-ops otherwise.
                    panic_guard::guarded("floor-watcher", || {
                        crate::realtime::FLOOR_WATCHER.lock_safe().tick(&handle)
                    });
                }
            });

            // Breaker beat — index.ts runBreakerBeat: ~30s interval, 300s
            // progress window. Policy lives in breaker.rs; enforcement here:
            // steer/constrain ride the hive mail channel, stop kills the PTY.
            let handle2 = app.handle().clone();
            panic_guard::supervise_arg("breaker-beat", handle2, |handle2| async move {
                let mut tick = tokio::time::interval(Duration::from_secs(30));
                const PROGRESS_WINDOW_MS: u64 = 300_000;
                loop {
                    tick.tick().await;
                    let st = handle2.state::<AppState>();
                    let cfg = st.config.lock_safe().clone();
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    // Only agents with a live PTY — dead entries steer into a
                    // god-bounce flood (upstream startup-migration note).
                    // Agent-namespace live set — the breaker steers registry
                    // agents, not renderer terminal ids.
                    let live: std::collections::HashSet<String> = st.live_agent_ids();
                    let inputs: Vec<breaker::BreakerInput> = st
                        .hive
                        .registry()
                        .iter()
                        // Upstream excludes god AND the assistant from the
                        // breaker — a tripping orchestrator must never be
                        // steered into an inbox flood, and hardStop must
                        // never kill the only coordinator (index.ts:1204).
                        .filter(|a| {
                            live.contains(&a.id) && !a.is_god && !a.is_assistant
                        })
                        .map(|a| breaker::BreakerInput {
                            agent_id: a.id.clone(),
                            // Live OTel first, then the proxy-sample ledger,
                            // then the transcript reconciler — upstream's
                            // exact preference order.
                            sample: st
                                .telemetry
                                .sample(&a.id)
                                .or_else(|| st.hive.usage_sample(&a.id))
                                .or_else(|| st.usage.sample(&a.cwd, a.session_id.as_deref())),
                            // Fresh coordination files OR a recent OTel tool
                            // span — a parent supervising subagents whose
                            // tool calls never hit its own hooks still counts
                            // as progressing (upstream's two-leg check).
                            progressing: now
                                .saturating_sub(st.hive.last_coordination_at(&a.id))
                                < PROGRESS_WINDOW_MS
                                || now
                                    .saturating_sub(st.telemetry.last_activity_at(&a.id))
                                    < PROGRESS_WINDOW_MS,
                            last_work_at: hive::Hive::last_work_at(&a.cwd),
                        })
                        .collect();
                    // (No early `continue` on empty inputs: the ledger append,
                    // session fallback, api-error drain and context-trigger
                    // cadence below are independent of the breaker; only
                    // breaker.tick is skipped when no non-god worker is live.)
                    // Cost-ledger append gate (index.ts #56): each live
                    // session's cumulative usd is ledgered when the counter
                    // moved — the fold detects restarts as counter decreases.
                    {
                        let mut moved = st.cost_moved.lock_safe();
                        for a in st.hive.registry() {
                            if !live.contains(&a.id) {
                                continue;
                            }
                            for (sid, row) in st.telemetry.session_samples(&a.id) {
                                let key = format!("{}\t{}", a.id, sid);
                                let usd = row["usd"].as_f64().unwrap_or(0.0);
                                if moved.get(&key).copied().unwrap_or(-1.0) == usd {
                                    continue;
                                }
                                if st.hive.append_cost_sample(&a.id, &row).is_ok() {
                                    moved.insert(key, usd);
                                }
                            }
                            // Grok pushes no OTel and writes no Claude
                            // transcript: cost it from the usage file its CLI
                            // maintains (upstream #536), gated on the numbers
                            // having moved (a running total would otherwise
                            // re-append the same row every beat).
                            if a.provider == "grok" {
                                if let Some(sid) =
                                    a.session_id.as_deref().filter(|s| !s.is_empty())
                                {
                                    if let Some(g) = usage::grok_usage(&a.cwd, sid) {
                                        if GROK_LEDGER_GATE.admits(&a.id, sid, &g) {
                                            let _ = st.hive.append_cost_sample(
                                                &a.id,
                                                &serde_json::json!({
                                                    "session_id": sid,
                                                    "input": g.sample.input,
                                                    "output": g.sample.output,
                                                    "cache_read": g.sample.cache_read,
                                                    "cache_creation": g.sample.cache_creation,
                                                    "model": g.model,
                                                    "usd": g.sample.usd,
                                                }),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                    // recordSession fallback — the resume key must exist even
                    // when hooks don't fire; upstream stamps it from the live
                    // sample on every beat. Also refreshes fleet.json so the
                    // file god is told to Read is never hours stale.
                    for a in st.hive.registry() {
                        if !live.contains(&a.id) {
                            continue;
                        }
                        if let Some((sid, _)) = st.telemetry.freshest_session_pub(&a.id) {
                            if a.session_id.as_deref() != Some(sid.as_str()) {
                                let _ = st.hive.record_session(&a.id, &sid);
                            }
                        }
                    }
                    // (fleet.json refresh lives on its own 8s timer below —
                    // upstream's fleetTimer cadence, not the breaker beat's.)
                    // Drain each agent's api_error stamps into the breaker's
                    // storm window before the tick (telemetry push → pull).
                    {
                        let mut b = st.breaker.lock_safe();
                        for i in &inputs {
                            for _ in st.telemetry.take_api_errors(&i.agent_id) {
                                b.record_error(&i.agent_id);
                            }
                        }
                        drop(b);
                    }
                    let decisions = if inputs.is_empty() {
                        Vec::new()
                    } else {
                        st.breaker.lock_safe().tick(
                            &inputs,
                            now,
                            &cfg.circuit_breaker,
                            cfg.cost_cap_usd,
                            cfg.cost_cap_tokens,
                            &cfg.agent_token_caps,
                            &cfg.agent_cost_caps,
                        )
                    };
                    for d in decisions {
                        let _ = handle2.emit("control:breakerState", &d.state);
                        let id = d.state.agent_id.clone();
                        let name = st
                            .hive
                            .registry()
                            .iter()
                            .find(|a| a.id == id)
                            .map(|a| a.name.clone())
                            .unwrap_or_else(|| id.clone());
                        match d.action {
                            breaker::BreakerAction::Steer => {
                                let _ = st.hive.send_virtual(
                                    "breaker",
                                    &id,
                                    "request",
                                    "Circuit breaker: steer",
                                    &format!(
                                        "Automated guardrail: {}. Re-check your approach — if you're looping or stuck, STOP repeating, summarize what you've tried, and ask god for direction.",
                                        d.state.reason
                                    ),
                                );
                            }
                            breaker::BreakerAction::Constrain => {
                                let _ = st.hive.send_virtual(
                                    "breaker",
                                    &id,
                                    "request",
                                    "Circuit breaker: constrain",
                                    &format!(
                                        "Automated guardrail escalated: {}. Stop active work now: switch to read-only/plan, write a short plan of your next step, and send it to god for sign-off BEFORE running more tools.",
                                        d.state.reason
                                    ),
                                );
                                let _ = handle2.emit(
                                    "breaker-toast",
                                    serde_json::json!({ "text": format!("{name} constrained"), "reason": d.state.reason }),
                                );
                            }
                            breaker::BreakerAction::Stop => {
                                // `id` is an AGENT id — resolve the PTY first
                                // (upstream: ptyForAgent(agentId) → kill +
                                // teardownPty). teardown drops the mapping and
                                // runs the full agent-keyed cleanup.
                                if let Some(pty_id) = st.pty_for_agent(&id) {
                                    if let Some(mut h) =
                                        st.ptys.lock_safe().remove(&pty_id)
                                    {
                                        let _ = h.kill();
                                    }
                                    commands::teardown_pty_core(&handle2, &pty_id, "gone");
                                }
                                let _ = handle2.emit(
                                    "breaker-toast",
                                    serde_json::json!({ "text": format!("{name} stopped by circuit breaker"), "reason": d.state.reason }),
                                );
                            }
                            breaker::BreakerAction::None => {}
                        }
                    }
                    // Context-trigger cadence (index.ts syncContextTriggers):
                    // the backend owns ONLY the cadence — when a rule comes
                    // due we emit `trigger:context` {action,rule} and the
                    // renderer applies the per-agent pressure gate +
                    // provider-specific verb (upstream emits into its queue).
                    for action in ["compact", "clear"] {
                        let rule = if action == "compact" {
                            &cfg.context_trigger.compact
                        } else {
                            &cfg.context_trigger.clear
                        };
                        if !rule.enabled || rule.every_ms == 0 {
                            continue;
                        }
                        let fired = st
                            .context_last_compact
                            .lock_safe()
                            .get(action)
                            .copied()
                            .unwrap_or(0);
                        if now.saturating_sub(fired) < rule.every_ms {
                            continue;
                        }
                        {
                            let mut m = st.context_last_compact.lock_safe();
                            m.insert(action.to_string(), now);
                            let _ = st.persist.lock_safe().set_kv(
                                "context:lastRun",
                                &serde_json::to_value(&*m).unwrap_or_else(|_| serde_json::json!({})),
                            );
                        }
                        emit_context_trigger(&handle2, action);
                    }
                }
            });
            // fleet.json heartbeat — upstream's ~8s fleetTimer keeps the
            // snapshot god is told to Read fresh even when no hook fires.
            let handle6 = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(8));
                loop {
                    tick.tick().await;
                    panic_guard::guarded("fleet-snapshot", || {
                        commands::write_fleet_snapshot(&handle6)
                    });
                }
            });
            // Ephemeral-worker watcher (workers.rs — upstream's 1.5s
            // spawn-request poll): release-on-done + idle/token-cap reaping,
            // then gated intake from hive/spawn-requests/.
            let handle5 = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_millis(1500));
                loop {
                    tick.tick().await;
                    panic_guard::guarded("worker-watcher", || workers::tick(&handle5));
                }
            });
            // Worker-wake watchdog beat (workerwake.rs, #151): the backstop
            // for mail that lands on an ALREADY-idle worker — the Stop-time
            // idle delivery can't see it. 10s cadence, edge-triggered nudges.
            let handle4 = app.handle().clone();
            panic_guard::supervise_arg("worker-wake-watchdog", handle4, |handle4| async move {
                let mut tick = tokio::time::interval(Duration::from_secs(10));
                loop {
                    tick.tick().await;
                    let st = handle4.state::<AppState>();
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    let mut facts = Vec::new();
                    for a in st.hive.registry() {
                        // Registry rows are agent-keyed; the PTY map is not —
                        // translate through the spawn-time mapping.
                        let Some(pty_id) = st.pty_for_agent(&a.id) else {
                            continue;
                        };
                        let pty = st.ptys.lock_safe();
                        let Some(h) = pty.get(&pty_id) else {
                            drop(pty);
                            continue;
                        };
                        let last_output_at =
                            h.last_output_at.load(std::sync::atomic::Ordering::Relaxed);
                        drop(pty);
                        let inbox = st.hive.inbox(&a.id).unwrap_or_default();
                        let mut oldest = 0u64;
                        let ids: Vec<String> = inbox
                            .iter()
                            .map(|m| {
                                if let Ok(t) =
                                    chrono::DateTime::parse_from_rfc3339(&m.created_at)
                                {
                                    let ms = t.timestamp_millis() as u64;
                                    if oldest == 0 || ms < oldest {
                                        oldest = ms;
                                    }
                                }
                                m.id.clone()
                            })
                            .collect();
                        let snap = st.control.lock_safe().snapshot(&a.id);
                        // Telemetry is the watchdog's primary turn-proof; the
                        // transcript read is the same signal at rest.
                        let tel_activity = st.telemetry.last_activity_at(&a.id);
                        let sample = st
                            .telemetry
                            .sample(&a.id)
                            .or_else(|| st.usage.sample(&a.cwd, a.session_id.as_deref()));
                        facts.push(workerwake::WorkerWakeFacts {
                            agent_id: a.id.clone(),
                            is_god: a.is_god,
                            pty_id: Some(pty_id.clone()),
                            last_output_at,
                            inbox_ids: ids,
                            auto_delivery_paused: snap.auto_delivery_paused,
                            paused: snap.paused,
                            halted: snap.halted,
                            // A turn only (upstream activityEvidenceAt): a
                            // zero-token session-start sample is not one.
                            last_activity_at: tel_activity
                                .max(telemetry::sample_activity_ts(sample.as_ref())),
                            has_telemetry: st.telemetry.has_telemetry(&a.id) || sample.is_some(),
                            oldest_mail_at: oldest,
                        });
                    }
                    // Webhook done-observer — the outbound half of the trigger
                    // ledger (upstream polls every 5s; we piggyback this beat).
                    crate::webhook::poll_done_tasks(
                        &st.hive,
                        &st.trigger_history,
                        &handle4,
                    );
                    // Slack done-notifier — the same pass for slack-origin
                    // cards (upstream pollSlackDoneTasks, 5s cadence).
                    commands::poll_slack_done_tasks(&handle4);
                    if facts.is_empty() {
                        continue;
                    }
                    // Report held workers with genuinely old mail once per
                    // cooldown — the watchdog's silence otherwise reads as
                    // "nothing to do" (upstream shouldReportHold).
                    {
                        let mut wake = st.wake.lock_safe();
                        for f in &facts {
                            if f.oldest_mail_at > 0
                                && now.saturating_sub(f.oldest_mail_at) > 60_000
                            {
                                if let Some(why) = wake.explain(f, now) {
                                    if wake.should_report_hold(&f.agent_id, now) {
                                        let _ = handle4.emit(
                                            "hive-log",
                                            serde_json::json!({
                                                "event": "wake_hold",
                                                "agent": f.agent_id,
                                                "reason": why
                                            }),
                                        );
                                    }
                                }
                            }
                        }
                        let nudge = wake.decide(&facts, now);
                        drop(wake);
                        for id in nudge {
                            // `id` is an AGENT id (facts.agent_id) — the PTY
                            // map is keyed by pty id, so translate first.
                            if let Some(pty_id) = st.pty_for_agent(&id) {
                                // Two writes on purpose (text, then Enter) so a
                                // TUI doesn't treat the CR as part of a paste.
                                pty::write_pty(&st, &pty_id, workerwake::WORKER_WAKE_NUDGE);
                                pty::write_pty(&st, &pty_id, "\r");
                            }
                            let _ = handle4.emit(
                                "hive-log",
                                serde_json::json!({ "event": "wake_nudge", "agent": id }),
                            );
                        }
                    }
                }
            });

            // OTLP/HTTP-JSON telemetry collector — dynamic loopback port;
            // claude spawns get the URL via OTEL_EXPORTER_OTLP_ENDPOINT.
            {
                let st = app.state::<AppState>();
                match st.telemetry.clone().run(app.handle().clone()) {
                    Ok(port) => {
                        let _ = st
                            .otel_endpoint
                            .set(format!("http://127.0.0.1:{port}"));
                    }
                    Err(e) => log::warn!("[telemetry] collector not started: {e:#}"),
                }
            }

            // Inbound webhook gateway — binds once endpoints exist (a
            // webhooks_save can also start it later; the port never rebinds,
            // endpoint swaps are hot).
            {
                let st = app.state::<AppState>();
                let (eps, port) = {
                    let cfg = st.config.lock_safe();
                    (cfg.webhooks.clone(), cfg.webhook_port)
                };
                if st.webhook.set_endpoints(&eps) > 0 {
                    // Drop held-token mappings whose ledger row aged out.
                    st.webhook
                        .prune_held(&app.handle(), &st.trigger_history);
                    let wh = st.webhook.clone();
                    let hive = st.hive.clone();
                    let app2 = app.handle().clone();
                    std::thread::spawn(move || {
                        if let Err(e) = wh.run(port, hive, app2) {
                            log::warn!("[webhook] server stopped: {e:#}");
                        }
                    });
                }
            }

            // Mission scheduler — syncMissions port as a delay loop, not a
            // fixed poll: upstream arms a per-mission setTimeout(remaining),
            // so the port sleeps until the earliest due mission, fires it,
            // then re-computes. A missions:save wakes the loop through
            // mission_rearm (upstream's clearMissionTimers+re-arm on write).
            // Heartbeat rides its adaptive cadence map, exactly upstream.
            let handle3 = app.handle().clone();
            panic_guard::supervise_arg("mission-scheduler", handle3, |handle3| async move {
                let mut heartbeat_cadence: std::collections::HashMap<String, u64> =
                    std::collections::HashMap::new();
                loop {
                    let st = handle3.state::<AppState>();
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    let missions_now: Vec<missions::ScheduledMission> =
                        st.config.lock_safe().missions.clone();
                    let mut fired_ids: Vec<String> = Vec::new();
                    for m in &missions_now {
                        let Some(due) =
                            missions::mission_due_ms(m, now, &heartbeat_cadence)
                        else {
                            continue;
                        };
                        if due > 0 {
                            continue;
                        }
                        if m.kind.as_deref() == Some("heartbeat") {
                            // The beat ALWAYS stamps + evaluates: re-engage on
                            // quiet-or-actionable, tighten on looksStuck.
                            let next =
                                missions::heartbeat_beat(&st, m, now);
                            heartbeat_cadence.insert(m.id.clone(), next);
                        } else {
                            fire_mission(&handle3, &st, m);
                        }
                        fired_ids.push(m.id.clone());
                    }
                    if !fired_ids.is_empty() {
                        let mut cfg = st.config.lock_safe();
                        for m in cfg.missions.iter_mut() {
                            if fired_ids.contains(&m.id) {
                                m.last_fired_at = now;
                            }
                        }
                        let _ = st.config_store.save(&cfg);
                        let _ = handle3.emit("missions:updated", serde_json::json!({}));
                    }
                    // Sleep until the earliest next due — upstream's timer
                    // granularity. Re-arm wakes take effect instantly; the
                    // 60s cap bounds drift if a mission is added out-of-band.
                    let wait = st
                        .config
                        .lock_safe()
                        .missions
                        .iter()
                        .filter_map(|m| {
                            missions::mission_due_ms(m, now, &heartbeat_cadence)
                        })
                        .min()
                        .unwrap_or(60_000)
                        .clamp(100, 60_000);
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(wait)) => {},
                        _ = st.mission_rearm.notified() => {},
                    }
                }
            });

            // powerMonitor — real OS power events where the platform exposes
            // them (power_monitor.rs: WM_POWERBROADCAST + WTS session events
            // on Windows). Suspend/lock stamp the clock; resume/unlock run
            // upstream's onSystemResume: re-arm the scheduler immediately
            // and, after the 15s pipe-wake grace, probe PTYs and emit
            // power:resume {reason, awayMs, dead, total}.
            {
                let (pwx, prx) = std::sync::mpsc::channel::<crate::power_monitor::PowerEvent>();
                let monitor_live = crate::power_monitor::start(pwx);
                if monitor_live {
                    let handle_p = app.handle().clone();
                    tauri::async_runtime::spawn(async move {
                        let mut suspend_at: u64 = 0;
                        let now_ms = || {
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0)
                        };
                        while let Ok(ev) = prx.recv() {
                            use crate::power_monitor::PowerEvent::*;
                            panic_guard::guarded("power-event", || match ev {
                                Suspend | LockScreen => suspend_at = now_ms(),
                                Resume | UnlockScreen => {
                                    let reason = if ev == Resume {
                                        "resume"
                                    } else {
                                        "unlock-screen"
                                    };
                                    let away_ms = now_ms().saturating_sub(suspend_at);
                                    // Upstream re-arms every frozen timer on
                                    // wake — our delay loop self-corrects but
                                    // the notify makes the re-eval instant.
                                    let st = handle_p.state::<AppState>();
                                    st.mission_rearm.notify_waiters();
                                    let handle9 = handle_p.clone();
                                    tauri::async_runtime::spawn(async move {
                                        tokio::time::sleep(Duration::from_secs(15)).await;
                                        crate::misc::power_resume_probe(
                                            &handle9, reason, away_ms,
                                        );
                                    });
                                }
                            });
                        }
                    });
                } else {
                    // Fallback for platforms without a power-event API: a 1s
                    // wall-clock gap watchdog — a suspended machine freezes
                    // every timer, so a >5s gap means we slept.
                    let handle10 = app.handle().clone();
                    panic_guard::supervise_arg("wall-clock-gap-watchdog", handle10, |handle10| async move {
                        let mut tick = tokio::time::interval(Duration::from_secs(1));
                        tick.tick().await;
                        let mut last_wall = 0u64;
                        loop {
                            tick.tick().await;
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0);
                            if last_wall != 0 && now.saturating_sub(last_wall) > 5_000 {
                                let away_ms = now - last_wall;
                                let handle9 = handle10.clone();
                                tauri::async_runtime::spawn(async move {
                                    tokio::time::sleep(Duration::from_secs(15)).await;
                                    crate::misc::power_resume_probe(
                                        &handle9, "resume", away_ms,
                                    );
                                });
                            }
                            last_wall = now;
                        }
                    });
                }
            }
            // Keep-awake (upstream syncKeepAwake): while any agent PTY is live
            // the OS must not idle-suspend the harness (Modern Standby froze
            // every agent mid-turn behind the lock screen). Re-evaluated every
            // 2s instead of at each spawn/teardown call site — idempotent, so
            // a toggled `strongKeepalive` also swaps the mode live. Released
            // the moment the floor empties (and at quit, in kill_all_ptys).
            let handle_k = app.handle().clone();
            panic_guard::supervise_arg("keep-awake", handle_k, |handle_k| async move {
                let mut tick = tokio::time::interval(Duration::from_secs(2));
                loop {
                    tick.tick().await;
                    let st = handle_k.state::<AppState>();
                    let live = !st.ptys.lock_safe().is_empty();
                    let strong = st.config.lock_safe().strong_keepalive;
                    power_monitor::sync_keep_awake(live, strong);
                }
            });

            // MemoryReflector timer (upstream reflector.start()): bound
            // oversized agent memory.md files on a 30-min cadence — the first
            // scan one interval OUT (launch must not compete with an LLM call,
            // and a freshly-restored home isn't condensed before it's mined).
            // Tunables are read from config each tick; passes are serialized
            // (the next tick only starts after the previous scan returned).
            // The manual `reflect_now` command uses the same thresholds.
            let handle_r = app.handle().clone();
            panic_guard::supervise_arg("memory-reflector", handle_r, |handle_r| async move {
                let first = {
                    let st = handle_r.state::<AppState>();
                    let cfg = st.config.lock_safe();
                    reflect::ReflectSettings::from_config(&cfg).scan_every_ms()
                };
                let mut wait = first;
                loop {
                    tokio::time::sleep(Duration::from_millis(wait)).await;
                    let (settings, hive_root) = {
                        let st = handle_r.state::<AppState>();
                        let cfg = st.config.lock_safe();
                        (reflect::ReflectSettings::from_config(&cfg), st.hive.root.clone())
                    };
                    wait = settings.scan_every_ms();
                    if !settings.enabled {
                        continue;
                    }
                    let _ = tauri::async_runtime::spawn_blocking(move || {
                        let path = pty::user_shell_path();
                        let claude = match pty::resolve_program("claude", &path) {
                            Some(pty::Resolved::Exe(p)) | Some(pty::Resolved::Script(p)) => {
                                p.display().to_string()
                            }
                            None => "claude".to_string(),
                        };
                        reflect::reflect_scan(&hive_root, &claude, &settings)
                    })
                    .await;
                }
            });
            // Slack boot re-arm from config.slackEnabled (upstream index.ts:5436)
            // and the notify-only updater's background checks / just-updated page.
            commands::slack_autostart(app.handle());
            cmd_update::start_background_checks(app.handle());
            Ok(())
        })
        .invoke_handler(panic_guard::guard_invoke(tauri::generate_handler![
            commands::spawn_agent,
            commands::pty_write,
            commands::pty_resize,
            commands::kill_agent,
            commands::hive_registry,
            commands::hive_send,
            commands::hive_inbox,
            commands::hive_mark_done,
            commands::hive_board,
            commands::hive_tasks,
            commands::read_memory,
            commands::append_memory,
            commands::get_config,
            commands::set_auto_mode,
            commands::control_pause,
            commands::control_resume,
            commands::control_halt,
            commands::control_stop_all,
            commands::control_resume_stopped,
            commands::control_steer,
            commands::control_gate_tool,
            commands::control_pause_delivery,
            commands::control_snapshot,
            commands::breaker_level,
            commands::missions_list,
            commands::missions_save,
            commands::respawn_agent,
            commands::webhooks_list,
            commands::webhooks_save,
            commands::webhook_approve,
            commands::webhook_reject,
            commands::webhook_pending,
            commands::triggers_list,
            commands::triggers_clear,
            commands::trigger_history_decide,
            commands::cost_lifetime,
            commands::roster_read,
            commands::roster_write,
            commands::sync_port,
            commands::roster_archive,
            commands::closing_time_start,
            commands::closing_time_cancel,
            commands::closing_time_status,
            commands::slack_start,
            commands::slack_stop,
            commands::slack_status,
            commands::slack_reply,
            commands::hire_validate,
            commands::hire_fetch,
            commands::hire_import_file,
            commands::hire_drain_pending,
            commands::app_confirm_close,
            cmd_misc::app_force_quit,
            commands::app_cancel_close,
            commands::app_info,
            engine_setup::engine_setup_snapshot,
            engine_setup::engine_setup_retry,
            engine_setup::hermes_user_config_status,
            commands::skills_reveal,
            cmd_hive::hive_add_task,
            cmd_hive::hive_delete_task,
            cmd_hive::hive_patch_task,
            cmd_hive::hive_patch_agent_role,
            cmd_hive::hive_rename_agent,
            cmd_hive::hive_set_archived,
            cmd_hive::hive_set_agent_hold,
            cmd_hive::hive_agent_directory,
            cmd_hive::hive_agent_usage,
            cmd_hive::hive_agent_context,
            cmd_hive::hive_log,
            cmd_hive::hive_messages,
            cmd_hive::hive_text_search,
            cmd_hive::session_resolve_cwd,
            cmd_hive::pty_list,
            cmd_hive::pty_redraw,
            cmd_hive::workers_list,
            cmd_hive::workers_registry,
            cmd_hive::workers_stop,
            cmd_git::git_is_repo,
            cmd_git::git_branch,
            cmd_git::git_status,
            cmd_git::git_log_graph,
            cmd_git::git_branches,
            cmd_git::git_ahead_behind,
            cmd_git::git_main_repo,
            cmd_git::git_diff,
            cmd_git::git_diff_refs,
            cmd_git::git_checkout,
            cmd_git::git_worktrees,
            cmd_git::git_worktree_add,
            cmd_git::git_worktree_remove,
            cmd_git::git_commit_files,
            cmd_git::git_compare_refs,
            cmd_git::git_show_file,
            conversation::conversation_read,
            cmd_kg::kg_status,
            cmd_kg::kg_list,
            cmd_kg::kg_search,
            cmd_kg::kg_get,
            cmd_kg::kg_remove,
            cmd_kg::kg_ingest_files,
            cmd_kg::kg_add_files,
            cmd_memory::memory_status,
            cmd_memory::search_memory,
            cmd_memory::memory_wake_up,
            cmd_memory::mine_now,
            cmd_integrations::integrations_list,
            cmd_integrations::integrations_templates,
            cmd_integrations::integrations_upsert,
            cmd_integrations::integrations_remove,
            cmd_integrations::integrations_set_secret,
            cmd_integrations::integrations_test,
            cmd_integrations::integrations_broker_start,
            cmd_integrations::integrations_broker_stop,
            cmd_integrations::integrations_broker_status,
            cmd_integrations::integrations_broker_token,
            cmd_freeflow::freeflow_set_config,
            cmd_freeflow::freeflow_transcribe,
            cmd_freeflow::freeflow_status,
            cmd_github::github_ci_runs,
            cmd_github::github_issues,
            cmd_github::github_set_repo,
            cmd_analytics::analytics_message_sent,
            cmd_analytics::analytics_set_opt_in,
            cmd_analytics::analytics_status,
            cmd_realtime::realtime_set_session_live,
            cmd_realtime::realtime_wait_for,
            realtime_actions::realtime_action,
            realtime_actions::realtime_action_confirm,
            realtime_actions::realtime_action_cancel,
            cmd_realtime::realtime_has_key,
            cmd_realtime::realtime_mint_token,
            cmd_realtime::realtime_drain_completions,
            cmd_realtime::realtime_track_dispatch,
            cmd_realtime::realtime_actions_status,
            cmd_misc::app_open_external,
            cmd_misc::app_clipboard_write,
            cmd_misc::app_clipboard_read,
            cmd_misc::clipboard_save_image,
            cmd_misc::clipboard_image_to_file,
            cmd_misc::app_reset_all,
            cmd_misc::app_set_login_item,
            cmd_misc::app_set_notifications,
            cmd_misc::terminal_open_at_folder,
            cmd_misc::tools_status,
            cmd_misc::fs_stat_abs,
            cmd_misc::fs_read_binary,
            cmd_misc::fs_write_file,
            cmd_misc::fs_reveal_path,
            cmd_misc::dialog_choose_folder,
            cmd_misc::dialog_attach_files,
            cmd_misc::models_catalog,
            cmd_misc::hero_payload,
            cmd_misc::roster_read_sync,
            cmd_misc::provider_key_set,
            cmd_misc::provider_key_has,
            cmd_misc::provider_key_validate,
            cmd_misc::provider_key_clear,
            cmd_misc::webhook_start,
            cmd_misc::webhook_stop,
            cmd_misc::webhook_status,
            cmd_misc::webhook_generate_secret,
            cmd_misc::webhooks_delete,
            cmd_misc::slack_set_config,
            cmd_misc::slack_reply_script_path,
            cmd_misc::telemetry_snapshot,
            cmd_misc::telemetry_spans,
            cmd_misc::telemetry_usage,
            cmd_misc::window_new_floor,
            cmd_misc::window_close_floor,
            cmd_misc::config_change_home,
            cmd_misc::config_ensure_home,
            cmd_misc::config_home_sync,
            cmd_misc::config_set_agent_token_cap,
            cmd_misc::config_set_agent_cost_cap,
            cmd_misc::control_set_breaker_state,
            cmd_misc::control_auto_delivery,
            cmd_misc::triggers_get_context,
            cmd_misc::triggers_set_context,
            cmd_misc::org_get_trigger,
            cmd_misc::org_set_trigger,
            cmd_update::update_check_now,
            cmd_update::update_current,
            cmd_update::update_open_release,
            cmd_update::update_download,
            cmd_update::update_restart_and_install,
            cmd_update::update_simulate,
            commands::config_update,
            commands::webhook_set_config,
            commands::hire_open_file,
            commands::hire_open_files,
            commands::kv_get,
            commands::kv_set,
            commands::command_history_add,
            commands::command_history_list,
            commands::command_history_search,
            commands::fs_list,
            commands::fs_read,
            commands::git_snapshot,
            cmd_git::git_log,
            commands::reflect_now,
            commands::task_update,
            commands::task_answer,
            commands::skills_local,
            commands::skills_catalog,
            commands::skills_install,
            commands::skills_bundled,
            commands::skills_install_bundled,
            commands::shardx_status,
            commands::shardx_set_token,
            commands::shardx_setup,
            commands::shardx_bootstrap,
            commands::shardx_cleanup,
            commands::shardx_profiles,
            commands::shardx_download_launcher,
            commands::shardx_token_preview,
            commands::shardx_proxies,
            commands::shardx_set_profile_proxy,
            commands::shardx_automations,
            commands::shardx_run_automation,
            commands::shardx_stop_automation,
            commands::shardx_automation_runs,
            commands::skills_uninstall,
            commands::secret_set,
            commands::secret_delete,
            commands::secrets_list,
        ]))
        .build(context)
        .expect("error while building agent-office")
        .run(|app, ev| {
            // Upstream's endSession — one coarse session_ended event on quit,
            // before the process drops (analytics.rs gates on DNT/opt-out).
            if matches!(ev, tauri::RunEvent::Exit) {
                // Each step isolated: a panic in analytics must not skip the
                // PTY kill below (this handler runs on the event-loop thread).
                panic_guard::guarded_unit("exit:analytics", || {
                    crate::analytics::global().end_session(app)
                });
                // Exit is final (ExitRequested can still be prevented, so the
                // kill lives here) — don't orphan the agent CLIs.
                panic_guard::guarded_unit("exit:kill-ptys", || kill_all_ptys(app));
                // A restart-and-install the user asked for while agents were
                // running (and then confirmed the quit warning for).
                panic_guard::guarded_unit("exit:install-staged", || {
                    cmd_update::install_staged_on_exit(app)
                });
            }
        });
}
