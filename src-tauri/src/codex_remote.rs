//! Codex remote-control daemon — port of upstream `src/shared/codexRemote.ts`
//! + `enableCodexRemoteForSpawn` (src/main/index.ts, commits 33317a2c /
//! 1d95952b).
//!
//! Codex's remote control is DAEMON based (there is no `/remote-control` slash
//! command): inside this agent's isolated `CODEX_HOME` we run
//! `codex app-server daemon start` + `codex app-server daemon enable-remote-control`,
//! then spawn the TUI with `--remote unix://<home>/app-server-control/app-server-control.sock`
//! so the thread is visible in ChatGPT mobile. Because a unix socket path is
//! capped at ~104 bytes (`sun_path`), the real (long) agent home is reached
//! through a SHORT symlink alias `/tmp/mdc/<8 hex>`.
//!
//! PLATFORM: macOS / Linux only. OpenAI's own CLI refuses the daemon on
//! Windows ("daemon lifecycle is only supported on Unix platforms") and
//! upstream skips win32 too, so on Windows this whole module is a no-op.
//!
//! UNVERIFIED AT RUNTIME: this was written on a Windows machine and cannot be
//! compiled for, or run on, unix here. To make that honest and safe the logic
//! is split into
//!   * pure, platform-neutral functions (alias derivation, socket-fit check,
//!     argv building, the plan/alias decisions) that compile and are
//!     unit-tested on Windows,
//!   * an orchestrator (`setup`) that takes its side effects through the small
//!     `Io` struct, so the unix flow is unit-tested here with FAKE I/O,
//!   * the real I/O (`real_io`) whose only unix-only std API,
//!     `std::os::unix::fs::symlink`, is confined to a tiny
//!     `#[cfg(unix)]` / `#[cfg(not(unix))]` pair.
//! Activation is a RUNTIME `cfg!(unix)` check (not `#[cfg]` on blocks), so both
//! branches always compile. Real daemon behaviour (does the installed codex
//! support `app-server daemon`? does the socket appear?) has never been
//! exercised; every failure is non-fatal and degrades to a normal local TUI.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// Where codex binds its control socket, relative to `$CODEX_HOME`.
pub const SOCKET_RELATIVE: &str = "app-server-control/app-server-control.sock";

/// Fixed SHORT root for the alias. `$TMPDIR` cannot host it: macOS spells it
/// `/var/folders/xx/<30-char-hash>/T/` (49 bytes), which made the alias LONGER
/// than the home it was meant to shorten. With this root plus an 8-char digest
/// the whole socket path is 60 bytes.
pub const ALIAS_ROOT: &str = "/tmp/mdc";

/// Longest socket path the platform accepts (macOS `sun_path` is 104).
pub const SOCKET_MAX: usize = 104;

/// Per daemon command timeout (upstream: 20 s).
const DAEMON_TIMEOUT: Duration = Duration::from_secs(20);

/// `codex app-server daemon start`.
pub const DAEMON_START: [&str; 3] = ["app-server", "daemon", "start"];
/// `codex app-server daemon enable-remote-control`.
pub const DAEMON_ENABLE: [&str; 3] = ["app-server", "daemon", "enable-remote-control"];

// ---------------------------------------------------------------------------
// Pure helpers (platform-neutral, unit-tested on Windows)
// ---------------------------------------------------------------------------

/// `<root>/<first 8 hex of sha256("<real_home>\0<agent_id>")>` — unix-style
/// separators always (the alias only ever exists on unix).
pub fn alias_path(real_home: &str, agent_id: &str, root: &str) -> String {
    let mut h = Sha256::new();
    h.update(real_home.as_bytes());
    h.update([0u8]);
    h.update(agent_id.as_bytes());
    let digest = h.finalize();
    let hex: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("{}/{}", root.trim_end_matches('/'), hex)
}

/// `<short_home>/app-server-control/app-server-control.sock`.
pub fn socket_path(short_home: &str) -> String {
    format!("{}/{}", short_home.trim_end_matches('/'), SOCKET_RELATIVE)
}

/// Whether the socket path under `short_home` is bindable (bytes, not chars).
pub fn socket_fits(short_home: &str) -> bool {
    socket_path(short_home).len() < SOCKET_MAX
}

/// `unix://<socket path>` — the value of `--remote`.
pub fn endpoint(short_home: &str) -> String {
    format!("unix://{}", socket_path(short_home))
}

/// Global options must precede a `resume` subcommand, so the endpoint goes
/// FIRST. Already carrying `--remote` is left alone.
pub fn with_remote_args(args: Vec<String>, endpoint: &str) -> Vec<String> {
    if args.iter().any(|a| a == "--remote") {
        return args;
    }
    let mut out = Vec::with_capacity(args.len() + 2);
    out.push("--remote".to_string());
    out.push(endpoint.to_string());
    out.extend(args);
    out
}

/// Why remote control was not engaged (always non-fatal: local TUI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// Not a unix platform (Windows) — OpenAI's CLI refuses the daemon there.
    NotUnix,
    /// The spawn has no isolated CODEX_HOME (no hook shim): nothing to alias.
    NoCodexHome,
    /// Even the short alias would exceed `sun_path`.
    SocketTooLong(String),
    /// The alias path exists and is not our symlink to this home.
    AliasOccupied(String),
    /// Filesystem / daemon step failed (message for the log).
    Failed(String),
}

impl std::fmt::Display for Skip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Skip::NotUnix => write!(f, "not supported on this platform"),
            Skip::NoCodexHome => write!(f, "no isolated CODEX_HOME"),
            Skip::SocketTooLong(a) => write!(f, "socket path exceeds sun_path: {a}"),
            Skip::AliasOccupied(a) => write!(f, "short home alias is occupied: {a}"),
            Skip::Failed(e) => write!(f, "{e}"),
        }
    }
}

/// The first decision: may we even try, and where would the alias live?
pub fn plan(
    is_unix: bool,
    real_home: Option<&str>,
    agent_id: &str,
    alias_root: &str,
) -> Result<(String, String), Skip> {
    if !is_unix {
        return Err(Skip::NotUnix);
    }
    let home = real_home
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .ok_or(Skip::NoCodexHome)?;
    let alias = alias_path(home, agent_id, alias_root);
    // Bail before touching the filesystem: a daemon that starts and then dies on
    // bind would only ever show a generic readiness timeout.
    if !socket_fits(&alias) {
        return Err(Skip::SocketTooLong(alias));
    }
    Ok((home.to_string(), alias))
}

/// What currently sits at the alias path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AliasState {
    Absent,
    /// A symlink, with its raw (possibly relative) target.
    Symlink(String),
    /// A real file/dir — never ours.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AliasAction {
    Create,
    Reuse,
    Occupied,
}

/// Lexical normalisation (`.`/`..`/duplicate and trailing slashes) of a
/// unix-style path, enough to compare a symlink target with the real home.
fn normalize_unix(p: &str) -> String {
    let abs = p.starts_with('/');
    let mut stack: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            s => stack.push(s),
        }
    }
    let joined = stack.join("/");
    if abs {
        format!("/{joined}")
    } else {
        joined
    }
}

/// Reuse an existing alias only when it is a symlink that resolves to the SAME
/// real home (upstream: `resolve(dirname(alias), readlink(alias)) === resolve(realHome)`).
pub fn alias_action(state: &AliasState, alias: &str, real_home: &str) -> AliasAction {
    match state {
        AliasState::Absent => AliasAction::Create,
        AliasState::Other => AliasAction::Occupied,
        AliasState::Symlink(target) => {
            let resolved = if target.starts_with('/') {
                normalize_unix(target)
            } else {
                let dir = alias.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                normalize_unix(&format!("{dir}/{target}"))
            };
            if resolved == normalize_unix(real_home) {
                AliasAction::Reuse
            } else {
                AliasAction::Occupied
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Orchestration over injectable I/O (the unix flow is unit-tested with fakes)
// ---------------------------------------------------------------------------

/// Side effects of the setup, injected so the sequence can be tested without a
/// unix machine.
pub struct Io<'a> {
    pub alias_state: &'a dyn Fn(&str) -> AliasState,
    pub mkdir_all: &'a dyn Fn(&str) -> std::io::Result<()>,
    /// `symlink(real_home, alias)` — a directory symlink.
    pub symlink: &'a dyn Fn(&str, &str) -> std::io::Result<()>,
    /// Run `<codex> <argv…>` with `CODEX_HOME=<alias>`; Ok on exit code 0.
    pub run: &'a dyn Fn(&str, &[&str]) -> Result<(), String>,
    pub socket_exists: &'a dyn Fn(&str) -> bool,
}

/// What a successful setup hands back to the spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Engaged {
    /// The short `CODEX_HOME` to spawn the TUI with.
    pub alias: String,
    /// `unix://…` for `--remote`.
    pub endpoint: String,
}

/// plan → alias → `daemon start` → `daemon enable-remote-control` → socket
/// check. Any failure is a `Skip` (the caller starts a plain local TUI).
pub fn setup(
    is_unix: bool,
    real_home: Option<&str>,
    agent_id: &str,
    alias_root: &str,
    io: &Io,
) -> Result<Engaged, Skip> {
    let (home, alias) = plan(is_unix, real_home, agent_id, alias_root)?;
    let root = alias.rsplit_once('/').map(|(r, _)| r).unwrap_or("/");
    (io.mkdir_all)(root).map_err(|e| Skip::Failed(format!("create {root}: {e}")))?;
    match alias_action(&(io.alias_state)(&alias), &alias, &home) {
        AliasAction::Occupied => return Err(Skip::AliasOccupied(alias)),
        AliasAction::Reuse => {}
        AliasAction::Create => {
            (io.symlink)(&home, &alias)
                .map_err(|e| Skip::Failed(format!("symlink {alias} -> {home}: {e}")))?;
        }
    }
    (io.run)(&alias, &DAEMON_START)
        .map_err(|e| Skip::Failed(format!("daemon start failed: {e}")))?;
    (io.run)(&alias, &DAEMON_ENABLE)
        .map_err(|e| Skip::Failed(format!("enable-remote-control failed: {e}")))?;
    if !(io.socket_exists)(&socket_path(&alias)) {
        return Err(Skip::Failed(
            "daemon returned without a control socket".to_string(),
        ));
    }
    Ok(Engaged {
        endpoint: endpoint(&alias),
        alias,
    })
}

/// Apply an engaged setup to the spawn: short `CODEX_HOME`, `--remote` first.
pub fn apply(env: &mut HashMap<String, String>, tail: Vec<String>, e: &Engaged) -> Vec<String> {
    env.insert("CODEX_HOME".into(), e.alias.clone());
    with_remote_args(tail, &e.endpoint)
}

// ---------------------------------------------------------------------------
// Real I/O (the only place unix-only std APIs are named)
// ---------------------------------------------------------------------------

/// The one unix-only std call, confined to this cfg pair.
#[cfg(unix)]
fn symlink_dir(real_home: &str, alias: &str) -> std::io::Result<()> {
    std::os::unix::fs::symlink(real_home, alias)
}

#[cfg(not(unix))]
fn symlink_dir(_real_home: &str, _alias: &str) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "directory symlink aliases are only used on unix",
    ))
}

fn real_alias_state(alias: &str) -> AliasState {
    match std::fs::symlink_metadata(alias) {
        Err(_) => AliasState::Absent,
        Ok(m) if m.file_type().is_symlink() => match std::fs::read_link(alias) {
            Ok(t) => AliasState::Symlink(t.to_string_lossy().into_owned()),
            Err(_) => AliasState::Other,
        },
        Ok(_) => AliasState::Other,
    }
}

/// Run one daemon command to completion with a hard timeout (stderr kept for
/// the log, stdout discarded).
fn run_daemon_command(
    exe: &str,
    argv: &[&str],
    env: &HashMap<String, String>,
    timeout: Duration,
) -> Result<(), String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new(exe)
        .args(argv)
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut err = String::new();
                if let Some(mut s) = child.stderr.take() {
                    let _ = s.by_ref().take(8_000).read_to_string(&mut err);
                }
                return if status.success() {
                    Ok(())
                } else {
                    let err = err.trim();
                    Err(if err.is_empty() {
                        format!("Codex exited with {status}")
                    } else {
                        err.to_string()
                    })
                };
            }
            Ok(None) => {}
            Err(e) => return Err(e.to_string()),
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "Codex daemon command timed out after {}ms",
                timeout.as_millis()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Spawn-path entry (commands.rs): when this is a unix machine and the codex
/// spawn has an isolated `CODEX_HOME`, bring up the remote-control daemon and
/// return the tail with `--remote <endpoint>` first and `CODEX_HOME` in `env`
/// pointed at the short alias. EVERY failure returns `tail` unchanged (a normal
/// local TUI) after a log line. On Windows this returns immediately.
///
/// UNVERIFIED on unix — see the module docs.
pub fn apply_for_spawn(
    command_first_token: &str,
    path_env: &str,
    env: &mut HashMap<String, String>,
    tail: Vec<String>,
    agent_id: &str,
) -> Vec<String> {
    // Cheap early-out before resolving anything (the runtime cfg! check inside
    // `setup` is the authoritative gate; both branches always compile).
    if !cfg!(unix) {
        return tail;
    }
    let exe = match crate::pty::resolve_program(command_first_token, path_env) {
        Some(crate::pty::Resolved::Exe(p)) | Some(crate::pty::Resolved::Script(p)) => {
            p.display().to_string()
        }
        None => command_first_token.to_string(),
    };
    let base_env = env.clone();
    let run = |alias: &str, argv: &[&str]| -> Result<(), String> {
        let mut e: HashMap<String, String> = std::env::vars().collect();
        e.extend(base_env.clone());
        e.insert("CODEX_HOME".into(), alias.to_string());
        run_daemon_command(&exe, argv, &e, DAEMON_TIMEOUT)
    };
    let alias_state = |a: &str| real_alias_state(a);
    let mkdir_all = |d: &str| std::fs::create_dir_all(Path::new(d));
    let symlink = |h: &str, a: &str| symlink_dir(h, a);
    let socket_exists = |s: &str| Path::new(s).exists();
    let io = Io {
        alias_state: &alias_state,
        mkdir_all: &mkdir_all,
        symlink: &symlink,
        run: &run,
        socket_exists: &socket_exists,
    };
    let real_home = env.get("CODEX_HOME").cloned();
    match setup(cfg!(unix), real_home.as_deref(), agent_id, ALIAS_ROOT, &io) {
        Ok(engaged) => apply(env, tail, &engaged),
        Err(skip) => {
            log::warn!("[codex-remote] {skip}; starting local TUI");
            tail
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const HOME: &str = "/Users/someone/Library/Application Support/dev.fkrn.agentoffice/harness/agents/agent-1/codex-home";

    #[test]
    fn alias_is_short_stable_and_agent_specific() {
        let a = alias_path(HOME, "agent-1", ALIAS_ROOT);
        assert_eq!(a, alias_path(HOME, "agent-1", ALIAS_ROOT));
        assert_ne!(a, alias_path(HOME, "agent-2", ALIAS_ROOT));
        assert_ne!(a, alias_path("/other/home", "agent-1", ALIAS_ROOT));
        assert!(a.starts_with("/tmp/mdc/"));
        assert_eq!(a.len(), "/tmp/mdc/".len() + 8);
        assert!(a["/tmp/mdc/".len()..].chars().all(|c| c.is_ascii_hexdigit()));
        // trailing slash on the root does not double up
        assert_eq!(alias_path(HOME, "agent-1", "/tmp/mdc/"), a);
        // upstream parity: sha256("<home>\0<agent>") first 8 hex chars
        assert_eq!(alias_path("a", "b", "/r"), "/r/59b271ae");
    }

    #[test]
    fn socket_budget_matches_the_104_byte_limit() {
        let a = alias_path(HOME, "agent-1", ALIAS_ROOT);
        assert_eq!(socket_path(&a).len(), 60);
        assert!(socket_fits(&a));
        // the unshortened home would not fit
        assert!(!socket_fits(HOME));
        // boundary: exactly 103 bytes fits, 104 does not
        let pad = SOCKET_MAX - 1 - (SOCKET_RELATIVE.len() + 1);
        assert!(socket_fits(&"x".repeat(pad)));
        assert!(!socket_fits(&"x".repeat(pad + 1)));
        assert_eq!(
            endpoint("/tmp/mdc/abcd1234"),
            "unix:///tmp/mdc/abcd1234/app-server-control/app-server-control.sock"
        );
    }

    #[test]
    fn remote_flag_goes_first_and_is_never_duplicated() {
        let ep = "unix:///tmp/mdc/x/app-server-control/app-server-control.sock";
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            with_remote_args(v(&["resume", "sid", "--add-dir", "/a"]), ep),
            v(&["--remote", ep, "resume", "sid", "--add-dir", "/a"])
        );
        assert_eq!(with_remote_args(vec![], ep), v(&["--remote", ep]));
        let already = v(&["--remote", "unix:///other", "resume"]);
        assert_eq!(with_remote_args(already.clone(), ep), already);
        assert_eq!(DAEMON_START.join(" "), "app-server daemon start");
        assert_eq!(DAEMON_ENABLE.join(" "), "app-server daemon enable-remote-control");
    }

    #[test]
    fn plan_decisions() {
        // Windows (and any non-unix) never engages, even with a home.
        assert_eq!(plan(false, Some(HOME), "a", ALIAS_ROOT), Err(Skip::NotUnix));
        assert_eq!(plan(true, None, "a", ALIAS_ROOT), Err(Skip::NoCodexHome));
        assert_eq!(plan(true, Some("  "), "a", ALIAS_ROOT), Err(Skip::NoCodexHome));
        let (home, alias) = plan(true, Some(HOME), "a", ALIAS_ROOT).unwrap();
        assert_eq!(home, HOME);
        assert!(alias.starts_with("/tmp/mdc/"));
        // an over-long alias root is refused before any filesystem work
        let long_root = format!("/{}", "r".repeat(100));
        assert!(matches!(
            plan(true, Some(HOME), "a", &long_root),
            Err(Skip::SocketTooLong(_))
        ));
    }

    #[test]
    fn alias_reuse_rules() {
        let alias = "/tmp/mdc/abcd1234";
        assert_eq!(alias_action(&AliasState::Absent, alias, HOME), AliasAction::Create);
        assert_eq!(alias_action(&AliasState::Other, alias, HOME), AliasAction::Occupied);
        // our own symlink (absolute, with noise) is reused
        let noisy = format!("{HOME}/");
        assert_eq!(
            alias_action(&AliasState::Symlink(noisy), alias, HOME),
            AliasAction::Reuse
        );
        assert_eq!(
            alias_action(&AliasState::Symlink("/Users/someone/../someone/x/../Library/Application Support/dev.fkrn.agentoffice/harness/agents/agent-1/codex-home".into()), alias, HOME),
            AliasAction::Reuse
        );
        // someone else's symlink is not
        assert_eq!(
            alias_action(&AliasState::Symlink("/elsewhere".into()), alias, HOME),
            AliasAction::Occupied
        );
        // relative targets resolve against the alias's directory
        assert_eq!(
            alias_action(&AliasState::Symlink("../../home/u".into()), "/tmp/mdc/abcd1234", "/home/u"),
            AliasAction::Reuse
        );
    }

    /// Everything the fake world recorded.
    #[derive(Default)]
    struct World {
        calls: RefCell<Vec<String>>,
    }

    fn run_setup(
        w: &World,
        state: AliasState,
        fail_step: Option<&'static str>,
        socket: bool,
    ) -> Result<Engaged, Skip> {
        let alias_state = |_: &str| state.clone();
        let mkdir_all = |d: &str| {
            w.calls.borrow_mut().push(format!("mkdir {d}"));
            Ok(())
        };
        let symlink = |h: &str, a: &str| {
            w.calls.borrow_mut().push(format!("symlink {a} -> {h}"));
            Ok(())
        };
        let run = |alias: &str, argv: &[&str]| -> Result<(), String> {
            let step = argv.last().copied().unwrap_or("");
            w.calls
                .borrow_mut()
                .push(format!("run[{alias}] {}", argv.join(" ")));
            if fail_step == Some(step) {
                Err("boom".into())
            } else {
                Ok(())
            }
        };
        let socket_exists = |_: &str| socket;
        let io = Io {
            alias_state: &alias_state,
            mkdir_all: &mkdir_all,
            symlink: &symlink,
            run: &run,
            socket_exists: &socket_exists,
        };
        setup(true, Some(HOME), "agent-1", ALIAS_ROOT, &io)
    }

    #[test]
    fn unix_flow_runs_in_order_and_engages() {
        let w = World::default();
        let e = run_setup(&w, AliasState::Absent, None, true).expect("engaged");
        let alias = alias_path(HOME, "agent-1", ALIAS_ROOT);
        assert_eq!(e.alias, alias);
        assert_eq!(e.endpoint, endpoint(&alias));
        assert_eq!(
            *w.calls.borrow(),
            vec![
                "mkdir /tmp/mdc".to_string(),
                format!("symlink {alias} -> {HOME}"),
                format!("run[{alias}] app-server daemon start"),
                format!("run[{alias}] app-server daemon enable-remote-control"),
            ]
        );
        // apply: env points at the alias and --remote precedes `resume`
        let mut env = HashMap::from([("CODEX_HOME".to_string(), HOME.to_string())]);
        let tail = apply(&mut env, vec!["resume".into(), "sid".into()], &e);
        assert_eq!(env["CODEX_HOME"], alias);
        assert_eq!(tail[0], "--remote");
        assert_eq!(tail[1], e.endpoint);
        assert_eq!(&tail[2..], ["resume", "sid"]);
    }

    #[test]
    fn existing_alias_is_reused_without_a_new_symlink() {
        let w = World::default();
        run_setup(&w, AliasState::Symlink(HOME.into()), None, true).expect("engaged");
        assert!(!w.calls.borrow().iter().any(|c| c.starts_with("symlink")));
    }

    #[test]
    fn every_failure_degrades_to_local_and_stops_early() {
        let w = World::default();
        assert!(matches!(
            run_setup(&w, AliasState::Other, None, true),
            Err(Skip::AliasOccupied(_))
        ));
        assert!(!w.calls.borrow().iter().any(|c| c.starts_with("run")));

        let w = World::default();
        let err = run_setup(&w, AliasState::Absent, Some("start"), true).unwrap_err();
        assert!(matches!(&err, Skip::Failed(m) if m.contains("daemon start failed")));
        // a failed start never reaches enable-remote-control
        assert_eq!(w.calls.borrow().iter().filter(|c| c.starts_with("run")).count(), 1);

        let w = World::default();
        let err = run_setup(&w, AliasState::Absent, Some("enable-remote-control"), true).unwrap_err();
        assert!(matches!(&err, Skip::Failed(m) if m.contains("enable-remote-control failed")));

        let w = World::default();
        let err = run_setup(&w, AliasState::Absent, None, false).unwrap_err();
        assert!(matches!(&err, Skip::Failed(m) if m.contains("without a control socket")));
    }

    #[test]
    fn non_unix_gate_leaves_the_spawn_untouched() {
        // The runtime gate: on this (Windows) build cfg!(unix) is false, so the
        // real entry point must be a pure no-op. On unix CI this test is skipped
        // by the early return below (the unix flow is covered with fakes above).
        if cfg!(unix) {
            return;
        }
        let mut env = HashMap::from([("CODEX_HOME".to_string(), HOME.to_string())]);
        let before = env.clone();
        let tail = vec!["resume".to_string(), "sid".to_string()];
        let out = apply_for_spawn("codex", "", &mut env, tail.clone(), "agent-1");
        assert_eq!(out, tail);
        assert_eq!(env, before);
    }

    #[test]
    fn windows_real_io_symlink_pair_compiles_and_refuses_off_unix() {
        if cfg!(unix) {
            return;
        }
        let err = symlink_dir("/a", "/b").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
    }

    #[test]
    fn daemon_command_runner_reports_exit_status_and_timeouts() {
        // The runner itself is platform-neutral std::process code, so exercise
        // it for real with a shell that exists on the host.
        let (exe, ok, fail, slow): (&str, Vec<&str>, Vec<&str>, Vec<&str>) = if cfg!(windows) {
            (
                "cmd",
                vec!["/C", "exit 0"],
                vec!["/C", "echo nope 1>&2 & exit 3"],
                vec!["/C", "ping -n 6 127.0.0.1 >NUL"],
            )
        } else {
            (
                "sh",
                vec!["-c", "exit 0"],
                vec!["-c", "echo nope 1>&2; exit 3"],
                vec!["-c", "sleep 5"],
            )
        };
        let env: HashMap<String, String> = std::env::vars().collect();
        assert!(run_daemon_command(exe, &ok, &env, Duration::from_secs(10)).is_ok());
        let e = run_daemon_command(exe, &fail, &env, Duration::from_secs(10)).unwrap_err();
        assert!(e.contains("nope"), "{e}");
        let e = run_daemon_command(exe, &slow, &env, Duration::from_millis(300)).unwrap_err();
        assert!(e.contains("timed out"), "{e}");
        assert!(run_daemon_command("definitely-not-a-binary-xyz", &ok, &env, Duration::from_secs(1)).is_err());
    }
}
