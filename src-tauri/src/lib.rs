mod breaker;
mod commands;
mod config;
mod control;
mod hive;
mod hooks;
mod pty;
mod ptyenv;
mod usage;

use std::time::Duration;

use commands::AppState;
use tauri::{Emitter, Manager};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let harness_home = app
                .path()
                .app_data_dir()
                .expect("app_data_dir")
                .join("harness");
            // HookServer first: its TCP addr is injected as HIVE_SOCK into every
            // agent spawn. Connections arriving before state is managed harmlessly
            // answer {} (none can — no agents exist yet).
            let server = hooks::HookServer::start(&app.handle())
                .expect("hook server bind");
            let mut state = AppState::new(harness_home).expect("init app state");
            state.sock_addr = Some(server.addr.clone());
            app.manage(state);

            // Router loop — the Electron version watched outboxes via chokidar;
            // a 250ms poll keeps the same deliver contract with no watcher dep.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_millis(250));
                loop {
                    tick.tick().await;
                    let st = handle.state::<AppState>();
                    match st.hive.router_tick() {
                        Ok(deliveries) => {
                            for d in deliveries {
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
                }
            });

            // Breaker beat — index.ts runBreakerBeat: ~30s interval, 300s
            // progress window. Policy lives in breaker.rs; enforcement here:
            // steer/constrain ride the hive mail channel, stop kills the PTY.
            let handle2 = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(30));
                const PROGRESS_WINDOW_MS: u64 = 300_000;
                loop {
                    tick.tick().await;
                    let st = handle2.state::<AppState>();
                    let cfg = st.config.lock().unwrap().clone();
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    // Only agents with a live PTY — dead entries steer into a
                    // god-bounce flood (upstream startup-migration note).
                    let live: std::collections::HashSet<String> =
                        st.ptys.lock().unwrap().keys().cloned().collect();
                    let inputs: Vec<breaker::BreakerInput> = st
                        .hive
                        .registry()
                        .iter()
                        .filter(|a| live.contains(&a.id))
                        .map(|a| breaker::BreakerInput {
                            agent_id: a.id.clone(),
                            // Live ledger first (proxy CostSamples); Claude
                            // Code falls back to its transcript JSONL — the
                            // same OTel→transcript fallback order as upstream.
                            sample: st
                                .hive
                                .usage_sample(&a.id)
                                .or_else(|| st.usage.sample(&a.cwd, a.session_id.as_deref())),
                            progressing: now.saturating_sub(st.hive.last_coordination_at(&a.id))
                                < PROGRESS_WINDOW_MS,
                            last_work_at: hive::Hive::last_work_at(&a.cwd),
                        })
                        .collect();
                    if inputs.is_empty() {
                        continue;
                    }
                    let decisions = st.breaker.lock().unwrap().tick(
                        &inputs,
                        now,
                        &cfg.circuit_breaker,
                        cfg.cost_cap_usd,
                        cfg.cost_cap_tokens,
                        &cfg.agent_token_caps,
                    );
                    for d in decisions {
                        let _ = handle2.emit("breaker-state", &d.state);
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
                                if let Some(mut h) = st.ptys.lock().unwrap().remove(&id) {
                                    let _ = h.kill();
                                }
                                let _ = st.hive.set_status(&id, "stopped");
                                let _ = handle2.emit(
                                    "breaker-toast",
                                    serde_json::json!({ "text": format!("{name} stopped by circuit breaker"), "reason": d.state.reason }),
                                );
                            }
                            breaker::BreakerAction::None => {}
                        }
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
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
            commands::control_steer,
            commands::control_gate_tool,
            commands::control_snapshot,
            commands::breaker_level,
        ])
        .run(tauri::generate_context!())
        .expect("error while running office-agent");
}
