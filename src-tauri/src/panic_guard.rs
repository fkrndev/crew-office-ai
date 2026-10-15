//! Panic survival.
//!
//! The release profile unwinds (no `panic = "abort"`), so a panic ends only the
//! thread / task it happens on instead of the whole app and every agent CLI it
//! owns. This module is the other half of that bargain:
//!
//!   * [`install_panic_hook`] — records every panic (message, location, thread,
//!     backtrace) to `<app data dir>/panic.log`, then calls the previous hook.
//!     A packaged app has no console, so without this a swallowed panic would be
//!     invisible.
//!   * [`guarded`] — run one loop iteration / handler, swallow + log a panic.
//!   * [`supervise`] — run an async loop as a task and respawn it (bounded
//!     rate) if it panics.
//!   * [`diag`] — one-line diagnostics to `log` AND the panic log (the `log`
//!     crate has no logger installed in this app, so `log::warn!` alone is
//!     silent).
//!
//! Nothing here takes an `AppState` lock; the hook must work while any of them
//! is poisoned or held.

use std::any::Any;
use std::future::Future;
use std::io::Write;
use std::panic::{AssertUnwindSafe, PanicHookInfo};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Cap for `panic.log`: past this the oldest half is dropped.
pub const LOG_CAP_BYTES: usize = 1024 * 1024;

/// Entry separator — also the boundary the cap truncation cuts on.
const ENTRY_MARK: &str = "\n---- ";

/// Where the hook writes. `try_lock`ed everywhere: the hook must never block.
static LOG_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
/// Serialises appends (concurrent panics) — never held across anything but the
/// file write itself.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

pub fn set_log_path(path: PathBuf) {
    if let Ok(mut g) = LOG_PATH.lock() {
        *g = Some(path);
    }
}

fn current_log_path() -> Option<PathBuf> {
    LOG_PATH.try_lock().ok().and_then(|g| g.clone())
}

/// Best-effort text of a panic payload.
pub fn payload_str(p: &(dyn Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

fn timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

/// Append `entry` to `path`, keeping the file under `cap` bytes by dropping the
/// oldest half (cut at an entry boundary when one is found). Errors bubble up;
/// callers fall back to stderr.
pub fn append_capped(path: &Path, entry: &str, cap: usize) -> std::io::Result<()> {
    let _w = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() as usize + entry.len() > cap {
            if let Ok(old) = std::fs::read(path) {
                let mut from = old.len() / 2;
                if let Some(rel) = find(&old[from..], ENTRY_MARK.as_bytes()) {
                    from += rel + 1; // keep the entry's own leading '-'
                }
                let _ = std::fs::write(path, &old[from..]);
            }
        }
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(entry.as_bytes())?;
    f.flush()
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Write to the panic log, else stderr. Never panics, never blocks on app state.
fn emit(entry: &str) {
    let wrote = current_log_path()
        .map(|p| append_capped(&p, entry, LOG_CAP_BYTES).is_ok())
        .unwrap_or(false);
    if !wrote {
        let _ = std::io::stderr().write_all(entry.as_bytes());
    }
}

/// One-line diagnostic: `log::<level>!` plus an entry in the panic log.
pub fn diag(level: log::Level, msg: &str) {
    log::log!(level, "{msg}");
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        emit(&format!("{ENTRY_MARK}{} [{}] {msg}\n", timestamp(), level));
    }));
}

/// The full panic entry (shared by the hook and its test).
pub fn format_panic_entry(
    thread: &str,
    message: &str,
    location: &str,
    backtrace: &str,
) -> String {
    format!(
        "{ENTRY_MARK}{} [PANIC] thread '{thread}' at {location}\nmessage: {message}\nbacktrace:\n{backtrace}\n",
        timestamp()
    )
}

fn record_panic(info: &PanicHookInfo<'_>) {
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("<unnamed>");
    let thread_desc = format!("{name} ({:?})", thread.id());
    let message = payload_str(info.payload());
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown>".into());
    let bt = std::backtrace::Backtrace::force_capture().to_string();
    emit(&format_panic_entry(&thread_desc, &message, &location, &bt));
}

/// Install the process-wide panic hook (log to `path`, then chain to the
/// previous hook). Idempotent per process for the hook itself; the path can be
/// re-pointed with [`set_log_path`].
pub fn install_panic_hook(path: PathBuf) {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    set_log_path(path);
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // A panic inside a panic hook aborts the process — so nothing in
            // here is allowed to unwind.
            let _ = std::panic::catch_unwind(AssertUnwindSafe(|| record_panic(info)));
            let _ = std::panic::catch_unwind(AssertUnwindSafe(|| prev(info)));
        }));
    });
}

/// Run one unit of work (a loop iteration, a request handler). A panic is
/// caught and logged (the hook has already written the backtrace) and `None`
/// is returned so the caller's loop just continues with its next iteration.
pub fn guarded<R>(name: &str, f: impl FnOnce() -> R) -> Option<R> {
    match std::panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => Some(v),
        Err(p) => {
            diag(
                log::Level::Error,
                &format!("[guard] '{name}' panicked ({}); continuing", payload_str(&*p)),
            );
            None
        }
    }
}

/// Same as [`guarded`] but runs on a drop path where the closure's value is
/// irrelevant.
pub fn guarded_unit(name: &str, f: impl FnOnce()) {
    let _ = guarded(name, f);
}

/// Wrap the `generate_handler!` invoke handler so a panic in the SYNCHRONOUS
/// part of a command dispatch (argument extraction, or the body of a plain
/// `fn` command) rejects that one invoke instead of unwinding into the WebView2
/// IPC callback on the UI thread — an `extern "system"` boundary, where a panic
/// aborts the whole process even with an unwinding profile.
///
/// `#[tauri::command(async)]` / `async fn` bodies run in a spawned tokio task,
/// outside this call: a panic there ends only that task, and the invoke's
/// promise would then never settle (tauri 2.12 offers no hook for it, and
/// `InvokeResolver::reject` after the fact is not possible). That is what
/// [`cmd_guard`] / [`cmd_guard_async`] are for: every command body catches its
/// own panic and returns an error value. This wrapper stays as defence in depth
/// for the part they cannot see (argument extraction).
pub fn guard_invoke<R, H>(handler: H) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static
where
    R: tauri::Runtime,
    H: Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static,
{
    move |invoke| {
        let resolver = invoke.resolver.clone();
        let cmd = invoke.message.command().to_string();
        match std::panic::catch_unwind(AssertUnwindSafe(|| handler(invoke))) {
            Ok(handled) => handled,
            Err(p) => {
                let msg = format!("command `{cmd}` panicked: {}", payload_str(&*p));
                diag(log::Level::Error, &format!("[invoke] {msg}"));
                resolver.reject(msg);
                true
            }
        }
    }
}

/// What a command returns when its body panicked. Every `#[tauri::command]`
/// return type must implement this (the lint test below enforces that each
/// body goes through [`cmd_guard`] / [`cmd_guard_async`]); pick the error shape
/// the frontend already understands for that return type and never invent a
/// value that could be mistaken for a success.
pub trait PanicResponse {
    fn from_panic(cmd: &str, msg: String) -> Self;
}

/// The one message format every impl uses (same text `guard_invoke` rejects with).
pub fn panic_message(cmd: &str, msg: &str) -> String {
    format!("command `{cmd}` panicked: {msg}")
}

/// `Result<T, String>` (and any `E: From<String>`): the invoke rejects with the message.
impl<T, E: From<String>> PanicResponse for Result<T, E> {
    fn from_panic(cmd: &str, msg: String) -> Self {
        Err(E::from(panic_message(cmd, &msg)))
    }
}

/// Bare-`Value` commands answer `{ok, error}` objects; a panic is `{ok:false,error}`.
impl PanicResponse for serde_json::Value {
    fn from_panic(cmd: &str, msg: String) -> Self {
        serde_json::json!({ "ok": false, "error": panic_message(cmd, &msg) })
    }
}

/// `{ok:false, error}` — the shape `submitToPty` / Restart & Continue read.
impl PanicResponse for crate::commands::PtyOpResult {
    fn from_panic(cmd: &str, msg: String) -> Self {
        Self::err(panic_message(cmd, &msg))
    }
}

/// `{ok:false, error}` — the spawn result the renderer classifies as a spawn failure.
impl PanicResponse for crate::commands::PtySpawnResult {
    fn from_panic(cmd: &str, msg: String) -> Self {
        Self::err(panic_message(cmd, &msg))
    }
}

impl PanicResponse for bool {
    fn from_panic(_: &str, _: String) -> Self {
        false
    }
}

impl<T> PanicResponse for Option<T> {
    fn from_panic(_: &str, _: String) -> Self {
        None
    }
}

/// Empty list. Only for commands that cannot report an error; the panic log has the entry.
impl<T> PanicResponse for Vec<T> {
    fn from_panic(_: &str, _: String) -> Self {
        Vec::new()
    }
}

impl PanicResponse for () {
    fn from_panic(_: &str, _: String) -> Self {}
}

fn log_cmd_panic(name: &str, p: &(dyn Any + Send)) -> String {
    let msg = payload_str(p);
    // The hook has already written the backtrace; this line names the command.
    diag(log::Level::Error, &format!("[cmd] {}", panic_message(name, &msg)));
    msg
}

/// Run a SYNC command body. A panic is logged (command name + payload) and
/// turned into `R::from_panic` so the invoke settles with an error instead of
/// unwinding into the IPC callback.
pub fn cmd_guard<R: PanicResponse>(name: &'static str, f: impl FnOnce() -> R) -> R {
    match std::panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(p) => {
            let msg = log_cmd_panic(name, &*p);
            R::from_panic(name, msg)
        }
    }
}

/// Run an ASYNC command body. Without this a panic would end the spawned task
/// and the JS promise would never settle (tauri awaits the task before replying).
pub async fn cmd_guard_async<R: PanicResponse, F: Future<Output = R>>(name: &'static str, fut: F) -> R {
    use futures_util::FutureExt;
    match AssertUnwindSafe(fut).catch_unwind().await {
        Ok(v) => v,
        Err(p) => {
            let msg = log_cmd_panic(name, &*p);
            R::from_panic(name, msg)
        }
    }
}

/// Releases an in-flight slot (`fetch_sub(1)`) when dropped — including during
/// unwinding, so a handler that panics cannot leak its connection-cap slot and
/// slowly lock a server into permanent 503s.
pub struct CountGuard(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl CountGuard {
    pub fn new(counter: std::sync::Arc<std::sync::atomic::AtomicUsize>) -> Self {
        CountGuard(counter)
    }
}

impl Drop for CountGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Run an async loop under supervision: `make()` builds the future, it runs as
/// its own task, and if that task PANICS it is respawned after a backoff
/// (1s, 2s, 4s … capped at 30s; the streak resets once a run lasted a minute).
/// A normal return or a cancellation ends supervision. Loop-local state is
/// rebuilt by `make()` on each restart.
pub fn supervise<F, Fut>(name: &'static str, make: F)
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    supervise_with(name, make, Duration::from_secs(1), Duration::from_secs(30));
}

/// [`supervise`] for a loop that captures one value (typically an `AppHandle`,
/// or a tuple of them): `arg` is cloned for every (re)start and handed to the
/// factory, so the body reads exactly like the unsupervised
/// `spawn(async move { .. })` it replaces.
pub fn supervise_arg<T, F, Fut>(name: &'static str, arg: T, make: F)
where
    T: Clone + Send + Sync + 'static,
    F: Fn(T) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    supervise(name, move || make(arg.clone()));
}

pub(crate) fn supervise_with<F, Fut>(name: &'static str, make: F, base: Duration, max: Duration)
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tauri::async_runtime::spawn(async move {
        let mut streak: u32 = 0;
        loop {
            let started = Instant::now();
            match tokio::spawn(make()).await {
                Ok(()) => return,
                Err(e) if e.is_panic() => {
                    if started.elapsed() >= Duration::from_secs(60) {
                        streak = 0;
                    }
                    let delay = base
                        .saturating_mul(1u32 << streak.min(10))
                        .min(max);
                    streak = streak.saturating_add(1);
                    diag(
                        log::Level::Error,
                        &format!(
                            "[supervise] task '{name}' panicked ({}); restarting in {:?}",
                            payload_str(&*e.into_panic()),
                            delay
                        ),
                    );
                    tokio::time::sleep(delay).await;
                }
                Err(_) => return, // cancelled (runtime shutting down)
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("panic-guard-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("panic.log")
    }

    #[test]
    fn guarded_swallows_a_panic_and_the_loop_continues() {
        let done = AtomicUsize::new(0);
        let panics = AtomicUsize::new(0);
        for i in 0..6 {
            let r = guarded("test-iter", || {
                if i % 2 == 1 {
                    panic!("iteration {i} blows up");
                }
                done.fetch_add(1, Ordering::SeqCst);
                i
            });
            match r {
                Some(v) => assert_eq!(v, i),
                None => {
                    panics.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
        assert_eq!(done.load(Ordering::SeqCst), 3);
        assert_eq!(panics.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_loop_thread_survives_a_panicking_iteration_and_a_poisoned_lock() {
        use crate::lock_ext::LockExt;
        let state = Arc::new(Mutex::new(Vec::<u32>::new()));
        let st = state.clone();
        let h = std::thread::spawn(move || {
            for i in 0..10u32 {
                guarded("loop-iter", || {
                    let mut g = st.lock_safe();
                    g.push(i);
                    if i == 3 {
                        // Panics while the guard is held -> poisons the lock.
                        panic!("iteration 3 panics holding the lock");
                    }
                });
            }
        });
        h.join().expect("the loop thread itself must not die");
        // Every iteration ran, including the ones AFTER the poisoning panic.
        assert_eq!(*state.lock_safe(), (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn count_guard_releases_its_slot_when_the_handler_panics() {
        let n = Arc::new(AtomicUsize::new(1));
        let n2 = n.clone();
        let r = std::thread::spawn(move || {
            let _g = CountGuard::new(n2);
            panic!("handler panics");
        })
        .join();
        assert!(r.is_err());
        assert_eq!(n.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn append_capped_drops_the_oldest_half_at_an_entry_boundary() {
        let p = tmp("cap");
        let cap = 2_000;
        for i in 0..100 {
            let e = format!("{ENTRY_MARK}ts [T] entry-{i:03} {}\n", "x".repeat(60));
            append_capped(&p, &e, cap).unwrap();
        }
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(s.len() <= cap, "file stays under the cap, got {}", s.len());
        assert!(s.contains("entry-099"), "newest entry kept");
        assert!(!s.contains("entry-000"), "oldest entry dropped");
        assert!(s.starts_with("---- "), "cut lands on an entry boundary: {:?}", &s[..20]);
    }

    #[test]
    fn the_panic_hook_writes_message_location_thread_and_backtrace() {
        let p = tmp("hook");
        install_panic_hook(p.clone());
        let h = std::thread::Builder::new()
            .name("hook-test-thread".into())
            .spawn(|| panic!("unique-hook-test-marker"))
            .unwrap();
        assert!(h.join().is_err());
        let s = std::fs::read_to_string(&p).expect("panic.log written");
        assert!(s.contains("[PANIC]"), "{s}");
        assert!(s.contains("unique-hook-test-marker"), "{s}");
        assert!(s.contains("hook-test-thread"), "{s}");
        assert!(s.contains("panic_guard.rs:"), "location recorded: {s}");
        assert!(s.contains("backtrace:"), "{s}");
        // Re-point away so later tests' panics don't keep writing here.
        set_log_path(tmp("hook-after"));
    }

    // ---- PanicResponse impls ------------------------------------------------

    #[test]
    fn panic_response_result_is_a_prefixed_err() {
        let r: Result<u8, String> = PanicResponse::from_panic("my_cmd", "boom".into());
        assert_eq!(r.unwrap_err(), "command `my_cmd` panicked: boom");
        // any `E: From<String>` works too
        let r: Result<(), Box<dyn std::error::Error>> = PanicResponse::from_panic("c", "m".into());
        assert_eq!(r.unwrap_err().to_string(), "command `c` panicked: m");
    }

    #[test]
    fn panic_response_value_is_ok_false_with_error() {
        let v: serde_json::Value = PanicResponse::from_panic("shardx_status", "boom".into());
        assert_eq!(v["ok"], serde_json::json!(false));
        assert_eq!(v["error"], "command `shardx_status` panicked: boom");
    }

    #[test]
    fn panic_response_pty_results_use_their_err_constructors() {
        let r: crate::commands::PtyOpResult = PanicResponse::from_panic("pty_write", "boom".into());
        assert!(!r.ok);
        assert_eq!(r.error.as_deref(), Some("command `pty_write` panicked: boom"));
        let s: crate::commands::PtySpawnResult = PanicResponse::from_panic("spawn_agent", "boom".into());
        assert!(!s.ok && !s.resumed);
        assert_eq!(s.error.as_deref(), Some("command `spawn_agent` panicked: boom"));
        // serialises to the camelCase `{ok:false,error}` the renderer reads
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["ok"], serde_json::json!(false));
        assert!(j["error"].as_str().unwrap().contains("panicked"));
    }

    #[test]
    fn panic_response_plain_types() {
        assert!(!<bool as PanicResponse>::from_panic("c", "m".into()));
        assert_eq!(<Option<String> as PanicResponse>::from_panic("c", "m".into()), None);
        assert!(<Vec<u8> as PanicResponse>::from_panic("c", "m".into()).is_empty());
        <() as PanicResponse>::from_panic("c", "m".into());
    }

    // ---- cmd_guard / cmd_guard_async ---------------------------------------

    #[test]
    fn cmd_guard_passes_a_normal_result_through() {
        let r: Result<u32, String> = cmd_guard("ok_cmd", || Ok(7));
        assert_eq!(r, Ok(7));
        // `?` and early `return` inside the closure behave as in the fn body
        fn body(x: Option<u32>) -> Result<u32, String> {
            cmd_guard("q_cmd", || {
                if x == Some(0) {
                    return Err("zero".to_string());
                }
                let v = x.ok_or_else(|| "none".to_string())?;
                Ok(v + 1)
            })
        }
        assert_eq!(body(Some(1)), Ok(2));
        assert_eq!(body(Some(0)), Err("zero".to_string()));
        assert_eq!(body(None), Err("none".to_string()));
    }

    #[test]
    fn cmd_guard_turns_a_panic_into_the_error_value() {
        let r: Result<(), String> = cmd_guard("sync_boom", || panic!("sync-boom-marker"));
        let e = r.unwrap_err();
        assert!(e.contains("command `sync_boom` panicked: sync-boom-marker"), "{e}");
        let v: serde_json::Value = cmd_guard("sync_val", || -> serde_json::Value { panic!("x") });
        assert_eq!(v["ok"], serde_json::json!(false));
        // a non-string payload still produces a message
        let r: Result<(), String> = cmd_guard("odd", || std::panic::panic_any(42u8));
        assert!(r.unwrap_err().contains("non-string"));
    }

    #[test]
    fn cmd_guard_does_not_poison_state_for_later_commands() {
        use crate::lock_ext::LockExt;
        let state = Mutex::new(Vec::<u32>::new());
        let r: Result<(), String> = cmd_guard("poisoner", || {
            let mut g = state.lock_safe();
            g.push(1);
            panic!("panics while holding the lock");
        });
        assert!(r.unwrap_err().contains("poisoner"));
        // The next command on the same state works (lock_safe recovers the poison)...
        let r: Result<usize, String> = cmd_guard("next", || {
            let mut g = state.lock_safe();
            g.push(2);
            Ok(g.len())
        });
        assert_eq!(r, Ok(2));
        assert_eq!(*state.lock_safe(), vec![1, 2]);
        // ...and the thread itself is untouched.
        assert!(!std::thread::panicking());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cmd_guard_async_passes_through_and_catches_panics() {
        use crate::lock_ext::LockExt;
        let r: Result<u32, String> = cmd_guard_async("a_ok", async { Ok(3) }).await;
        assert_eq!(r, Ok(3));

        let state = Arc::new(Mutex::new(0u32));
        let st = state.clone();
        let r: Result<(), String> = cmd_guard_async("a_boom", async move {
            let mut g = st.lock_safe();
            *g += 1;
            tokio::task::yield_now().await; // panic after an await point, lock released by unwind
            drop(g);
            panic!("async-boom-marker");
        })
        .await;
        let e = r.unwrap_err();
        assert!(e.contains("command `a_boom` panicked: async-boom-marker"), "{e}");
        assert_eq!(*state.lock_safe(), 1);

        // Spawned the way tauri runs an async command: the task completes with the
        // error value (no JoinError), i.e. the invoke would reply instead of hang.
        let h = tokio::spawn(async {
            let r: serde_json::Value = cmd_guard_async("a_val", async { panic!("v") }).await;
            r
        });
        let v = tokio::time::timeout(Duration::from_secs(10), h).await.expect("settles").expect("task did not panic");
        assert_eq!(v["ok"], serde_json::json!(false));
    }

    // ---- lint: every #[tauri::command] body goes through cmd_guard ----------

    /// Commands whose body cannot start with the guard. Keep EMPTY; every entry
    /// needs a comment saying why wrapping is impossible.
    const LINT_ALLOWLIST: &[&str] = &[];

    #[derive(Debug)]
    struct CmdSite {
        file: String,
        name: String,
        is_async: bool,
        body_head: String,
    }

    fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                rs_files(&p, out);
            } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
                out.push(p);
            }
        }
    }

    fn is_ident(c: u8) -> bool {
        c.is_ascii_alphanumeric() || c == b'_'
    }

    /// Find every `#[tauri::command...]` attribute (alone at the start of a line,
    /// so doc comments / strings mentioning it are ignored), the `fn` that follows,
    /// and the first 80 non-whitespace bytes of its body.
    fn scan_commands(file: &str, src: &str) -> Vec<CmdSite> {
        let b = src.as_bytes();
        let mut out = Vec::new();
        let mut line_start = 0;
        while line_start < b.len() {
            let line_end = src[line_start..].find('\n').map_or(b.len(), |i| line_start + i);
            let line = src[line_start..line_end].trim();
            if line.starts_with("#[tauri::command") {
                // next `fn` keyword after the attribute line
                let mut i = line_end;
                let (mut name, mut is_async) = (String::new(), false);
                while i < b.len() {
                    if b[i..].starts_with(b"fn ") && (i == 0 || !is_ident(b[i - 1])) {
                        let before = src[..i].trim_end();
                        is_async = before.ends_with("async");
                        let rest = &src[i + 3..];
                        name = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                        i += 3 + name.len();
                        break;
                    }
                    i += 1;
                }
                // params: balanced parens
                while i < b.len() && b[i] != b'(' {
                    i += 1;
                }
                let mut depth = 0;
                while i < b.len() {
                    match b[i] {
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                // return type: up to the `{` at <>/() depth 0 (`->` is not a closer)
                let mut d = 0i32;
                i += 1;
                while i < b.len() {
                    match b[i] {
                        b'<' | b'(' | b'[' => d += 1,
                        b'>' if b[i - 1] != b'-' => d -= 1,
                        b')' | b']' => d -= 1,
                        b'{' if d == 0 => break,
                        _ => {}
                    }
                    i += 1;
                }
                let body: String = src[(i + 1).min(b.len())..]
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .take(80)
                    .collect();
                out.push(CmdSite { file: file.to_string(), name, is_async, body_head: body });
            }
            line_start = line_end + 1;
        }
        out
    }

    fn scan_all() -> Vec<CmdSite> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rs_files(&root, &mut files);
        files.sort();
        let mut all = Vec::new();
        for f in files {
            let src = std::fs::read_to_string(&f).unwrap();
            let rel = f.strip_prefix(&root).unwrap().display().to_string();
            all.extend(scan_commands(&rel, &src));
        }
        all
    }

    #[test]
    fn lint_scanner_understands_the_shapes_we_have() {
        // Built with concat! so no source line of THIS file starts with the attribute.
        let attr = concat!("#[tauri", "::command");
        let src = format!(
            "/// `{attr}]` in a doc comment
             {attr}(async)]
pub fn a(x: Vec<(u8, u8)>) -> Result<Vec<u8>, String> {{
    crate::panic_guard::cmd_guard(\"a\", || {{
}})
}}
             {attr}]
pub async fn b(s: State<'_, S>) -> Result<(), String> {{
 crate::panic_guard::cmd_guard_async(\"b\", async move {{
}}).await
}}
"
        );
        let v = scan_commands("x.rs", &src);
        assert_eq!(v.len(), 2, "{v:?}");
        assert_eq!((v[0].name.as_str(), v[0].is_async), ("a", false));
        assert_eq!((v[1].name.as_str(), v[1].is_async), ("b", true));
        assert!(v[0].body_head.starts_with("crate::panic_guard::cmd_guard(\"a\""));
        assert!(v[1].body_head.starts_with("crate::panic_guard::cmd_guard_async(\"b\""));
    }

    #[test]
    fn every_tauri_command_body_is_wrapped_in_cmd_guard() {
        let all = scan_all();
        assert!(all.len() > 200, "scanner found only {} commands — it is broken", all.len());
        let mut bad = Vec::new();
        for c in &all {
            if LINT_ALLOWLIST.contains(&c.name.as_str()) {
                continue;
            }
            let want = if c.is_async {
                format!("crate::panic_guard::cmd_guard_async(\"{}\",", c.name)
            } else {
                format!("crate::panic_guard::cmd_guard(\"{}\",", c.name)
            };
            if !c.body_head.starts_with(&want) {
                bad.push(format!("{}: fn {} must start with `{want} ..`", c.file, c.name));
            }
        }
        assert!(
            bad.is_empty(),
            "#[tauri::command] bodies not wrapped (a panic would leave the JS promise pending forever):\n{}",
            bad.join("\n")
        );
    }

    /// Bare command names listed in lib.rs `generate_handler![ .. ]`
    /// (comments and `#[cfg]` attributes dropped; build.rs does the same parse).
    fn registered_commands() -> Vec<String> {
        let src = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs")).unwrap();
        let at = src.find("tauri::generate_handler![").expect("generate_handler! in lib.rs");
        let rest = &src[at + "tauri::generate_handler![".len()..];
        let mut body = String::new();
        let (mut depth, mut chars) = (1, rest.chars().peekable());
        while let Some(c) = chars.next() {
            match c {
                '/' if chars.peek() == Some(&'/') => {
                    for c2 in chars.by_ref() {
                        if c2 == '\n' {
                            break;
                        }
                    }
                    continue;
                }
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            body.push(c);
        }
        // drop `#[...]` attributes
        let mut cleaned = String::new();
        let mut rest = body.as_str();
        while let Some(h) = rest.find("#[") {
            cleaned.push_str(&rest[..h]);
            rest = &rest[h..];
            rest = &rest[rest.find(']').expect("closed attribute") + 1..];
        }
        cleaned.push_str(rest);
        cleaned
            .split(',')
            .map(|s| s.trim().rsplit("::").next().unwrap().trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    #[test]
    fn every_registered_command_is_covered_by_the_lint_scan() {
        let scanned: Vec<String> = scan_all().into_iter().map(|c| c.name).collect();
        let reg = registered_commands();
        assert!(reg.len() > 200, "parsed only {} registered commands", reg.len());
        let missing: Vec<&String> = reg.iter().filter(|r| !scanned.contains(r)).collect();
        assert!(
            missing.is_empty(),
            "registered in lib.rs but not found by the #[tauri::command] scan (so not lint-checked): {missing:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn supervise_respawns_a_panicking_async_loop() {
        let runs = Arc::new(AtomicUsize::new(0));
        let r = runs.clone();
        supervise_with(
            "test-loop",
            move || {
                let r = r.clone();
                async move {
                    let n = r.fetch_add(1, Ordering::SeqCst);
                    if n < 2 {
                        panic!("run {n} panics");
                    }
                    // third run: survives
                    loop {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                }
            },
            Duration::from_millis(10),
            Duration::from_millis(40),
        );
        let t0 = Instant::now();
        while runs.load(Ordering::SeqCst) < 3 && t0.elapsed() < Duration::from_secs(30) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(runs.load(Ordering::SeqCst), 3, "restarted after each panic (runs after {:?})", t0.elapsed());
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(runs.load(Ordering::SeqCst), 3, "stable run is not restarted");
    }
}
