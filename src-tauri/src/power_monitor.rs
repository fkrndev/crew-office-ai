//! Real OS power events — the Electron `powerMonitor` parity this port
//! previously approximated with a wall-clock gap watchdog.
//!
//! Windows path: a hidden message-only window subscribes to
//! WM_POWERBROADCAST (PBT_APMSUSPEND / PBT_APMRESUME*) and, via
//! WTSRegisterSessionNotification, WM_WTSSESSION_CHANGE (lock/unlock) —
//! the exact four events upstream's index.ts wires:
//!   suspend      → stamp lastSuspendAt
//!   lock-screen  → stamp lastSuspendAt
//!   resume       → onSystemResume('resume')
//!   unlock-screen→ onSystemResume('unlock-screen')
//!
//! Non-Windows platforms fall back to the caller's gap watchdog.

use crate::lock_ext::LockExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    Suspend,
    LockScreen,
    Resume,
    UnlockScreen,
}

/// Spawn the monitor thread; events arrive on `tx`. Returns false where the
/// OS API is unavailable so the caller can keep the gap watchdog running.
#[cfg(windows)]
pub fn start(tx: std::sync::mpsc::Sender<PowerEvent>) -> bool {
    std::thread::Builder::new()
        .name("power-monitor".into())
        .spawn(move || windows_loop(tx))
        .is_ok()
}

#[cfg(not(windows))]
pub fn start(_tx: std::sync::mpsc::Sender<PowerEvent>) -> bool {
    false
}

#[cfg(windows)]
fn windows_loop(tx: std::sync::mpsc::Sender<PowerEvent>) {
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::System::RemoteDesktop::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    /// FFI boundary: a panic unwinding out of an `extern "system"` fn aborts
    /// the process, so the real handler runs under catch_unwind and a panic
    /// degrades to the default window procedure.
    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            wndproc_inner(hwnd, msg, wparam, lparam)
        }))
        .unwrap_or_else(|p| {
            crate::panic_guard::diag(
                log::Level::Error,
                &format!(
                    "[power-monitor] wndproc panicked ({}); using DefWindowProc",
                    crate::panic_guard::payload_str(&*p)
                ),
            );
            DefWindowProcW(hwnd, msg, wparam, lparam)
        })
    }

    unsafe fn wndproc_inner(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let tx_ptr =
            GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const std::sync::mpsc::Sender<PowerEvent>;
        let send = |e: PowerEvent| {
            if !tx_ptr.is_null() {
                let _ = (*tx_ptr).send(e);
            }
        };
        match msg {
            WM_POWERBROADCAST => {
                match wparam as u32 {
                    PBT_APMSUSPEND => send(PowerEvent::Suspend),
                    PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMECRITICAL => {
                        send(PowerEvent::Resume)
                    }
                    _ => {}
                }
                return TRUE as LRESULT;
            }
            WM_WTSSESSION_CHANGE => {
                match wparam as u32 {
                    WTS_SESSION_LOCK => send(PowerEvent::LockScreen),
                    WTS_SESSION_UNLOCK => send(PowerEvent::UnlockScreen),
                    _ => {}
                }
                return 0;
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe {
        let class_name: Vec<u16> = "AgentOfficePowerMonitor\0".encode_utf16().collect();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            lpszClassName: class_name.as_ptr(),
            ..std::mem::zeroed()
        };
        if RegisterClassW(&wc) == 0 {
            return;
        }
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }
        // Hand the sender to the wndproc via GWLP_USERDATA.
        let tx_box = Box::into_raw(Box::new(tx));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, tx_box as isize);
        // Session lock/unlock notifications (upstream's lock-screen/
        // unlock-screen listeners).
        WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
        let mut msg = std::mem::zeroed::<MSG>();
        while GetMessageW(&mut msg, hwnd, 0, 0) > 0 {
            DispatchMessageW(&msg);
        }
        WTSUnRegisterSessionNotification(hwnd);
        let _ = Box::from_raw(tx_box);
    }
}

// ─── keep-awake (upstream powerSaveBlocker) ─────────────────────────────────
//
// While at least one agent PTY is alive the machine must not idle-suspend:
// Windows Modern Standby suspends desktop apps (and their child agent CLIs)
// shortly after the display sleeps/locks, freezing every agent mid-turn until
// unlock. Upstream holds `prevent-app-suspension` (≈ ES_SYSTEM_REQUIRED: the
// display still turns off and the session still locks) and escalates to
// `prevent-display-sleep` (also ES_DISPLAY_REQUIRED) when the opt-in
// `config.strongKeepalive` is on. Released the moment the floor empties, so an
// idle harness never pins a laptop awake; an explicit sleep (lid close, Start >
// Sleep) still works.

/// What the app currently asks the OS to stay awake for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepAwake {
    Off,
    /// Survive idle suspension (default while PTYs are live).
    AppSuspension,
    /// Also keep the display on (strongKeepalive).
    DisplaySleep,
}

/// upstream syncKeepAwake's decision: nothing live → Off; else the strong
/// toggle picks the stronger blocker.
pub fn keep_awake_mode(live_ptys: bool, strong: bool) -> KeepAwake {
    match (live_ptys, strong) {
        (false, _) => KeepAwake::Off,
        (true, false) => KeepAwake::AppSuspension,
        (true, true) => KeepAwake::DisplaySleep,
    }
}

struct KeepAwakeState {
    mode: KeepAwake,
    #[cfg(windows)]
    tx: Option<std::sync::mpsc::Sender<KeepAwake>>,
    #[cfg(target_os = "macos")]
    child: Option<std::process::Child>,
}

static KEEP_AWAKE: std::sync::Mutex<KeepAwakeState> = std::sync::Mutex::new(KeepAwakeState {
    mode: KeepAwake::Off,
    #[cfg(windows)]
    tx: None,
    #[cfg(target_os = "macos")]
    child: None,
});

/// Idempotent: re-evaluated by the caller on a short cadence (and on config
/// change), only touches the OS when the desired mode differs from the held one.
pub fn sync_keep_awake(live_ptys: bool, strong: bool) {
    let desired = keep_awake_mode(live_ptys, strong);
    let mut st = KEEP_AWAKE.lock_safe();
    if st.mode == desired {
        return;
    }
    st.mode = desired;
    apply_keep_awake(&mut st, desired);
    if desired == KeepAwake::Off {
        log::info!("[power] keep-awake off - no agents");
    } else {
        log::info!("[power] keep-awake ON ({desired:?}) - agents running");
    }
}

/// Release any held request (quit path).
pub fn release_keep_awake() {
    sync_keep_awake(false, false);
}

/// Windows: SetThreadExecutionState is PER-THREAD — ES_CONTINUOUS holds only
/// while the calling thread lives — so one dedicated thread owns the request
/// and applies each mode change it is sent.
#[cfg(windows)]
fn apply_keep_awake(st: &mut KeepAwakeState, mode: KeepAwake) {
    if st.tx.is_none() {
        let (tx, rx) = std::sync::mpsc::channel::<KeepAwake>();
        let ok = std::thread::Builder::new()
            .name("keep-awake".into())
            .spawn(move || {
                use windows_sys::Win32::System::Power::{
                    SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED,
                    ES_SYSTEM_REQUIRED,
                };
                while let Ok(m) = rx.recv() {
                    let flags = match m {
                        KeepAwake::Off => ES_CONTINUOUS,
                        KeepAwake::AppSuspension => ES_CONTINUOUS | ES_SYSTEM_REQUIRED,
                        KeepAwake::DisplaySleep => {
                            ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
                        }
                    };
                    unsafe {
                        SetThreadExecutionState(flags);
                    }
                }
                // Sender dropped: the thread ends and the OS releases the request.
            })
            .is_ok();
        if !ok {
            return;
        }
        st.tx = Some(tx);
    }
    if let Some(tx) = &st.tx {
        // A dead keep-awake thread (receiver gone) is respawned on the next
        // call instead of silently dropping every later mode change.
        if tx.send(mode).is_err() {
            st.tx = None;
        }
    }
}

/// macOS: a `caffeinate` child (`-i` idle-sleep, `-d` display too), tied to our
/// pid with `-w` so it can never outlive the app.
#[cfg(target_os = "macos")]
fn apply_keep_awake(st: &mut KeepAwakeState, mode: KeepAwake) {
    if let Some(mut c) = st.child.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    let flag = match mode {
        KeepAwake::Off => return,
        KeepAwake::AppSuspension => "-i",
        KeepAwake::DisplaySleep => "-di",
    };
    st.child = std::process::Command::new("caffeinate")
        .arg(flag)
        .arg("-w")
        .arg(std::process::id().to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok();
}

/// Other platforms: no inhibitor wired (the sleep/resume catch-up path in
/// lib.rs still runs on wake).
#[cfg(not(any(windows, target_os = "macos")))]
fn apply_keep_awake(_st: &mut KeepAwakeState, _mode: KeepAwake) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_awake_mode_follows_live_ptys_and_the_strong_toggle() {
        assert_eq!(keep_awake_mode(false, false), KeepAwake::Off);
        assert_eq!(keep_awake_mode(false, true), KeepAwake::Off, "never pinned awake with an empty floor");
        assert_eq!(keep_awake_mode(true, false), KeepAwake::AppSuspension);
        assert_eq!(keep_awake_mode(true, true), KeepAwake::DisplaySleep);
    }
}
