mod commands;
mod config;
mod hive;
mod pty;
mod ptyenv;

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
            let state = AppState::new(harness_home).expect("init app state");
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running office-agent");
}
