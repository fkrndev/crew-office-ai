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

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let tx_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA)
            as *const std::sync::mpsc::Sender<PowerEvent>;
        let send = |e: PowerEvent| {
            if !tx_ptr.is_null() {
                let _ = (*tx_ptr).send(e);
            }
        };
        match msg {
            WM_POWERBROADCAST => {
                match wparam as u32 {
                    PBT_APMSUSPEND => send(PowerEvent::Suspend),
                    PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC => {
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
