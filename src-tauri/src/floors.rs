//! Multi-window "floors" (upstream 66a46ceb, `createWindow({ floor })`).
//!
//! The Electron original let the app open extra windows, each an independent
//! office: opt-in behind the `multiWindow` config flag, every PTY owned by the
//! window that spawned it (output routed ONLY there), a confirm when closing a
//! floor that still runs terminals, and the primary window keeping the
//! app-quit gate. This module is the Tauri side of that:
//!
//! * labels — `main` is floor 1; extra windows are `floor-<n>` (n >= 2);
//! * routing — [`emit_to_owner`] sends `pty:data` / `pty:exit` / `pty:relaunch`
//!   to the owning window's label. Everything else (hive:*, config:changed,
//!   agent-status…) stays a broadcast, like upstream;
//! * lifecycle — [`open_floor`] builds the window from the main window's
//!   config (same index.html, same webview args), [`attach_floor_events`]
//!   turns a CloseRequested with live PTYs into a `floor:closeRequested` event
//!   for that floor's UI (the confirm lives in the frontend), and
//!   `window_close_floor` (cmd_misc.rs) is the confirmed path:
//!   [`kill_owned`] then destroy.
//!
//! CAVEAT (same as upstream): the on-disk hive (god, registry, board, tasks)
//! stays one process-global instance — floors share it. What a floor owns is
//! its window, its PTYs and its roster (renderer-side, key-namespaced by
//! window label: see src/floors.ts).

use crate::lock_ext::LockExt;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{
    AppHandle, Emitter, EventTarget, Manager, Runtime, WebviewWindow, WebviewWindowBuilder,
    WindowEvent,
};

/// The primary window — owns the hive's god, the roster mirror file and the
/// app-quit gate.
pub const MAIN_LABEL: &str = "main";
pub const FLOOR_PREFIX: &str = "floor-";
/// Sent to a floor whose close was blocked because it still has live PTYs.
pub const CLOSE_REQUESTED_EVENT: &str = "floor:closeRequested";
/// Hard cap so a runaway caller cannot spawn windows forever.
const MAX_FLOOR_NUMBER: u32 = 64;
/// Logical-pixel offset a new floor cascades from the window that opened it.
const CASCADE_OFFSET: i32 = 36;

/// `floor-3` -> `Some(3)`; anything else (including `main`) -> `None`.
pub fn floor_number(label: &str) -> Option<u32> {
    label
        .strip_prefix(FLOOR_PREFIX)
        .and_then(|n| n.parse::<u32>().ok())
        .filter(|n| *n >= 2)
}

pub fn is_floor_label(label: &str) -> bool {
    floor_number(label).is_some()
}

pub fn floor_title(n: u32) -> String {
    format!("Agent Office \u{2014} Floor {n}")
}

/// Lowest unused `floor-<n>` label, starting at 2 (the main window is floor 1)
/// so reopening never collides with a floor that is still open.
pub fn next_floor_label(exists: impl Fn(&str) -> bool) -> Option<String> {
    (2..=MAX_FLOOR_NUMBER)
        .map(|n| format!("{FLOOR_PREFIX}{n}"))
        .find(|l| !exists(l))
}

/// Where a new floor lands: offset from its opener, kept inside the opener's
/// monitor (`(x, y, w, h)`, physical px) so it never opens off-screen.
pub fn cascade_position(
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    monitor: Option<(i32, i32, u32, u32)>,
    offset: i32,
) -> (i32, i32) {
    let (nx, ny) = (x + offset, y + offset);
    match monitor {
        Some((mx, my, mw, mh)) => {
            let max_x = (mx + mw as i32 - w as i32).max(mx);
            let max_y = (my + mh as i32 - h as i32).max(my);
            (nx.clamp(mx, max_x), ny.clamp(my, max_y))
        }
        None => (nx, ny),
    }
}

/// The `multiWindow` flag from the raw config document (default OFF).
pub fn flag_from_raw(raw: &Value) -> bool {
    raw.get("multiWindow").and_then(Value::as_bool) == Some(true)
}

pub fn multi_window_enabled<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<crate::AppState>()
        .map(|st| flag_from_raw(&st.config_store.raw_value()))
        .unwrap_or(false)
}

/// Emit `event` to ONE window, by label. NOTE: JS listeners registered with
/// the default `Any` target receive every emit regardless of the filter, so
/// the frontend must listen with `{ target: <its own label> }` for this to
/// isolate anything (src/floors.ts `listenTarget`). A label with no live
/// window is a silent no-op — a closed floor's stragglers just drop.
pub fn emit_to_owner<R: Runtime, S: Serialize + Clone>(
    app: &AppHandle<R>,
    owner: &str,
    event: &str,
    payload: S,
) {
    #[cfg(test)]
    ROUTED
        .lock_safe()
        .push((owner.to_string(), event.to_string()));
    let _ = app.emit_to(EventTarget::webview_window(owner), event, payload);
}

/// Test seam: every `(owner, event)` handed to [`emit_to_owner`]. Tauri's own
/// Rust-side listeners cannot witness routing under concurrent emits (their
/// pending-emit queue drops the target filter), so the PTY test asserts on
/// this log instead and the filter semantics get their own sequential test.
#[cfg(test)]
pub(crate) static ROUTED: std::sync::LazyLock<std::sync::Mutex<Vec<(String, String)>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(Vec::new()));

/// Live PTYs owned by `label`.
pub fn live_count<R: Runtime>(app: &AppHandle<R>, label: &str) -> usize {
    app.try_state::<crate::AppState>()
        .map(|st| crate::pty::count_by_owner(&st.ptys.lock_safe(), label))
        .unwrap_or(0)
}

/// Stop every PTY owned by `label` (their waiters run the normal teardown).
pub fn kill_owned<R: Runtime>(app: &AppHandle<R>, label: &str) -> usize {
    app.try_state::<crate::AppState>()
        .map(|st| crate::pty::kill_by_owner(&st.ptys, label))
        .unwrap_or(0)
}

/// CloseRequested / Destroyed handling for a floor window. A close with live
/// PTYs is held and handed to the floor's UI to confirm (upstream used a
/// native dialog; the renderer modal is the port's equivalent); a floor with
/// none closes immediately. Destroyed is the backstop that stops any PTY the
/// confirm path did not.
pub fn attach_floor_events<R: Runtime>(win: &WebviewWindow<R>) {
    let label = win.label().to_string();
    let app = win.app_handle().clone();
    // Runs on the event-loop thread: a panic here would unwind out of tao's
    // message loop and end the whole app, so it is contained per event.
    win.on_window_event(move |ev| crate::panic_guard::guarded_unit("floor-window-event", || match ev {
        WindowEvent::CloseRequested { api, .. } => {
            let n = live_count(&app, &label);
            if n > 0 {
                api.prevent_close();
                emit_to_owner(
                    &app,
                    &label,
                    CLOSE_REQUESTED_EVENT,
                    json!({ "ptyCount": n }),
                );
            }
        }
        WindowEvent::Destroyed => {
            kill_owned(&app, &label);
        }
        _ => {}
    }));
}

/// Build a new floor window and return its label. `from` (the invoking
/// window) lends its size and a cascaded position.
pub fn open_floor<R: Runtime>(
    app: &AppHandle<R>,
    from: Option<&WebviewWindow<R>>,
) -> Result<String, String> {
    let label = next_floor_label(|l| app.get_webview_window(l).is_some())
        .ok_or_else(|| "too many floors".to_string())?;
    let n = floor_number(&label).unwrap_or(2);
    // Same window as `main` (decorations, min size, webview args — WebView2
    // requires identical browser args for windows that share a profile), only
    // relabelled and retitled.
    let mut cfg = app
        .config()
        .app
        .windows
        .first()
        .cloned()
        .unwrap_or_default();
    cfg.label = label.clone();
    cfg.title = floor_title(n);
    cfg.center = false;
    let mut builder = WebviewWindowBuilder::from_config(app, &cfg).map_err(|e| e.to_string())?;
    if let Some(src) = from {
        let scale = src.scale_factor().unwrap_or(1.0).max(0.1);
        if let (Ok(size), Ok(pos)) = (src.inner_size(), src.outer_position()) {
            let mon = src.current_monitor().ok().flatten().map(|m| {
                (
                    m.position().x,
                    m.position().y,
                    m.size().width,
                    m.size().height,
                )
            });
            let off = (CASCADE_OFFSET as f64 * scale).round() as i32;
            let (x, y) = cascade_position(pos.x, pos.y, size.width, size.height, mon, off);
            builder = builder
                .inner_size(size.width as f64 / scale, size.height as f64 / scale)
                .position(x as f64 / scale, y as f64 / scale);
        }
    }
    let win = builder.build().map_err(|e| e.to_string())?;
    attach_floor_events(&win);
    Ok(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(floor_number("floor-2"), Some(2));
        assert_eq!(floor_number("floor-12"), Some(12));
        assert_eq!(floor_number("main"), None);
        assert_eq!(floor_number("floor-1"), None, "main is floor 1");
        assert_eq!(floor_number("floor-x"), None);
        assert_eq!(floor_number("xfloor-2"), None);
        assert!(is_floor_label("floor-3") && !is_floor_label("main"));
        assert_eq!(floor_title(3), "Agent Office \u{2014} Floor 3");
    }

    #[test]
    fn next_label_takes_lowest_free_slot_and_caps() {
        assert_eq!(next_floor_label(|_| false).as_deref(), Some("floor-2"));
        let open = ["floor-2", "floor-3", "floor-5"];
        assert_eq!(
            next_floor_label(|l| open.contains(&l)).as_deref(),
            Some("floor-4")
        );
        assert_eq!(next_floor_label(|_| true), None);
    }

    #[test]
    fn cascade_offsets_and_stays_on_monitor() {
        let mon = Some((0, 0, 1920, 1080));
        assert_eq!(cascade_position(100, 100, 800, 600, mon, 36), (136, 136));
        // Would run off the right/bottom edge -> pinned inside the monitor.
        assert_eq!(cascade_position(1100, 500, 800, 600, mon, 36), (1120, 480));
        // Minimized opener (-32000) -> pulled onto the monitor.
        assert_eq!(cascade_position(-32000, -32000, 800, 600, mon, 36), (0, 0));
        // Window bigger than the monitor: never negative of the origin.
        assert_eq!(cascade_position(0, 0, 3000, 2000, mon, 36), (0, 0));
        assert_eq!(cascade_position(10, 10, 800, 600, None, 36), (46, 46));
    }

    #[test]
    fn flag_defaults_off_and_needs_a_true_boolean() {
        assert!(!flag_from_raw(&json!({})));
        assert!(!flag_from_raw(&json!({ "multiWindow": false })));
        assert!(!flag_from_raw(&json!({ "multiWindow": "true" })));
        assert!(flag_from_raw(&json!({ "multiWindow": true })));
    }
}

/// Routing + count/kill against REAL PTYs (`cmd /c ping`, no agent CLIs) and
/// mock windows: one PTY owned by `main`, one by `floor-2`.
#[cfg(all(test, windows))]
mod routing_tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tauri::{Listener, WebviewUrl};

    type Ptys = Mutex<HashMap<String, crate::pty::PtyHandle>>;

    fn routed(owner: &str, event: &str) -> bool {
        ROUTED
            .lock_safe()
            .iter()
            .any(|(o, e)| o == owner && e == event)
    }

    fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(15) {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("timed out waiting for {what}");
    }

    /// ConPTY asks for the cursor position (CSI 6n) and the child blocks on
    /// the reply; the real app's xterm answers, so the test must.
    fn answer_cpr(map: &Ptys, answered: &mut HashMap<String, usize>) {
        let mut m = map.lock_safe();
        for (id, h) in m.iter_mut() {
            let cur = h.tail().matches("\u{1b}[6n").count();
            let done = answered.entry(id.clone()).or_default();
            for _ in *done..cur {
                let _ = h.write("\u{1b}[24;80R");
            }
            *done = cur;
        }
    }

    #[test]
    fn pty_output_is_routed_to_its_owner_and_kill_by_owner_is_scoped() {
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        let cwd = std::env::temp_dir().display().to_string();
        let env: HashMap<String, String> = HashMap::new();
        let spawn = |id: &str, owner: &str| {
            crate::pty::spawn(
                id,
                "ping -n 120 127.0.0.1",
                &cwd,
                &env,
                80,
                24,
                handle.clone(),
                owner,
            )
            .unwrap()
        };
        let map: Ptys = Mutex::new(HashMap::new());
        map.lock_safe()
            .insert("rt-m".into(), spawn("rt-m", "main"));
        map.lock_safe()
            .insert("rt-f".into(), spawn("rt-f", "floor-2"));
        assert_eq!(map.lock_safe()["rt-f"].owner, "floor-2");
        {
            let m = map.lock_safe();
            assert_eq!(crate::pty::count_by_owner(&m, "main"), 1);
            assert_eq!(crate::pty::count_by_owner(&m, "floor-2"), 1);
            assert_eq!(crate::pty::count_by_owner(&m, "floor-9"), 0);
        }

        let mut answered = HashMap::new();
        wait_for("both PTYs to print", || {
            answer_cpr(&map, &mut answered);
            routed("main", "pty:data:rt-m") && routed("floor-2", "pty:data:rt-f")
        });
        // Each stream was addressed to its owner and nobody else.
        assert!(
            !routed("floor-2", "pty:data:rt-m"),
            "main's output addressed to the floor"
        );
        assert!(
            !routed("main", "pty:data:rt-f"),
            "the floor's output addressed to main"
        );

        // Killing the floor's PTYs stops exactly those.
        assert_eq!(crate::pty::kill_by_owner(&map, "floor-2"), 1);
        wait_for("the floor PTY exit", || routed("floor-2", "pty:exit:rt-f"));
        assert!(!routed("main", "pty:exit:rt-f"));
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            !routed("main", "pty:exit:rt-m"),
            "main's PTY died with the floor's"
        );

        assert_eq!(crate::pty::kill_by_owner(&map, "main"), 1);
        wait_for("main PTY exit", || routed("main", "pty:exit:rt-m"));
        assert!(!routed("floor-2", "pty:exit:rt-m"));
        assert_eq!(crate::pty::kill_by_owner(&map, "nobody"), 0);
    }

    /// The Tauri semantics the frontend relies on, pinned: a label-scoped
    /// listener only hears its own window's targeted emits and broadcasts,
    /// while an `Any`-target listener (JS `listen()` WITHOUT `target`) hears
    /// everything — which is why src/floors.ts registers pty listeners with
    /// `{ target: <label> }`. Sequential on purpose (see ROUTED).
    #[test]
    fn targeted_emit_reaches_label_scoped_listeners_only() {
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        let mk = |label: &str| {
            WebviewWindowBuilder::new(&handle, label, WebviewUrl::App("index.html".into()))
                .build()
                .unwrap()
        };
        let (main, floor) = (mk("main"), mk("floor-2"));
        let seen: Arc<Mutex<Vec<&'static str>>> = Arc::default();
        let (s1, s2, s3) = (seen.clone(), seen.clone(), seen.clone());
        main.listen("t:evt", move |_| s1.lock_safe().push("main"));
        floor.listen("t:evt", move |_| s2.lock_safe().push("floor-2"));
        handle.listen_any("t:evt", move |_| s3.lock_safe().push("any"));

        let take = || {
            let mut v: Vec<&str> = std::mem::take(&mut *seen.lock_safe());
            v.sort();
            v
        };
        emit_to_owner(&handle, "floor-2", "t:evt", json!(1));
        assert_eq!(take(), vec!["any", "floor-2"]);
        emit_to_owner(&handle, "main", "t:evt", json!(1));
        assert_eq!(take(), vec!["any", "main"]);
        emit_to_owner(&handle, "floor-7", "t:evt", json!(1)); // no such window
        assert_eq!(take(), vec!["any"]);
        handle.emit("t:evt", json!(1)).unwrap(); // broadcast: everyone
        assert_eq!(take(), vec!["any", "floor-2", "main"]);
    }
}
