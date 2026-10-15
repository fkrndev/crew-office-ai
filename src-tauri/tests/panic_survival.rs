//! Panic survival, end to end at the library level: the release profile
//! unwinds, so a panic must end only the unit it happened in — a background
//! loop keeps iterating, the shared `AppState` locks stay usable after a panic
//! while one is held, and the panic is recorded (message, location, thread,
//! backtrace) in `panic.log`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_office_lib::commands::AppState;
use agent_office_lib::lock_ext::LockExt;
use agent_office_lib::panic_guard;

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("oa-panic-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_loop_thread_survives_a_panic_holding_a_real_app_state_lock() {
    let home = tmp("home");
    let log = home.join("panic.log");
    panic_guard::install_panic_hook(log.clone());

    let st = Arc::new(AppState::new(home.join("harness")).unwrap());
    let iterations = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // A "router-like" loop: every tick touches shared state under a lock, and
    // iteration 3 panics WHILE HOLDING the config lock (=> poisoned).
    let loop_thread = {
        let (st, iterations, stop) = (st.clone(), iterations.clone(), stop.clone());
        std::thread::Builder::new()
            .name("test-router-loop".into())
            .spawn(move || {
                let mut tick = 0u32;
                while !stop.load(Ordering::Relaxed) {
                    tick += 1;
                    panic_guard::guarded("test-router-tick", || {
                        let cfg = st.config.lock_safe();
                        let _missions = cfg.missions.len();
                        if tick == 3 {
                            panic!("synthetic-iteration-panic-marker");
                        }
                        iterations.fetch_add(1, Ordering::SeqCst);
                    });
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
            .unwrap()
    };

    // The loop must keep making progress PAST the poisoning iteration.
    let t0 = Instant::now();
    while iterations.load(Ordering::SeqCst) < 6 && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        iterations.load(Ordering::SeqCst) >= 6,
        "loop stalled after the panic: {} good iterations",
        iterations.load(Ordering::SeqCst)
    );
    assert!(!loop_thread.is_finished(), "the loop thread must still be alive");

    // Other threads can still use the (formerly poisoned) lock.
    let st2 = st.clone();
    std::thread::spawn(move || {
        let _ = st2.config.lock_safe().missions.len();
        let _ = st2.ptys.lock_safe().len();
    })
    .join()
    .expect("a later user of the lock must not panic");

    stop.store(true, Ordering::Relaxed);
    loop_thread.join().unwrap();

    // The hook recorded the panic with its context.
    let logged = std::fs::read_to_string(&log).expect("panic.log written");
    assert!(logged.contains("synthetic-iteration-panic-marker"), "{logged}");
    assert!(logged.contains("test-router-loop"), "thread name recorded: {logged}");
    assert!(logged.contains("backtrace:"), "{logged}");
    assert!(logged.contains("panic_survival.rs"), "location recorded: {logged}");
}
