//! PTY manager — port of `src/main/pty.ts` (node-pty) onto `portable-pty`.
//!
//! Each agent CLI runs as a real process in a pseudo-terminal. Output is read
//! on a dedicated thread per PTY and forwarded to the frontend as
//! `pty-output` events; input flows back via `pty_write`. Semantics mirror the
//! Electron version byte-for-byte where the crates allow it.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

/// How a program resolved on PATH must be launched.
pub enum Resolved {
    /// Native binary — spawn argv directly (newlines/quotes in args are safe).
    Exe(PathBuf),
    /// .bat/.cmd shim — must go through `cmd /d /s /c` (npm-installed CLIs).
    Script(PathBuf),
}

/// PATH + PATHEXT lookup, so `claude` resolves to `claude.exe`/`claude.cmd`
/// exactly like typing it in a terminal. The Electron version got this for
/// free from node-pty.
pub fn resolve_program(prog: &str, path_env: &str) -> Option<Resolved> {
    let prog = prog.trim().trim_matches('"');
    if prog.is_empty() {
        return None;
    }
    // An explicit path (contains a separator or drive) is used as-is.
    if prog.contains(['/', '\\']) || prog.contains(':') {
        let p = PathBuf::from(prog);
        return p.exists().then(|| classify(p));
    }
    // Only a plain command name is resolved (shellEnv isSafeCommandName).
    if !is_safe_command_name(prog) {
        return None;
    }
    let exts: Vec<String> = if cfg!(windows) {
        // PATHEXT-eligible extensions — upstream reads the env var and also
        // requires it on `where` hits; a custom PATHEXT (.ps1 etc.) resolves.
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|e| !e.trim().is_empty())
            .map(|e| {
                let e = e.trim().to_lowercase();
                if e.starts_with('.') {
                    e
                } else {
                    format!(".{e}")
                }
            })
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in path_env.split(if cfg!(windows) { ';' } else { ':' }) {
        for ext in &exts {
            let cand = PathBuf::from(dir).join(format!("{prog}{ext}"));
            if cand.is_file() {
                return Some(classify(cand));
            }
        }
    }
    // PATH missed — the established install locations
    // (shellEnv.ts windowsFallbackCandidates: npm global shims, the Claude
    // installer dir, provider-specific homes).
    for cand in fallback_candidates(prog) {
        if cand.is_file() {
            return Some(classify(cand));
        }
    }
    None
}

/// Where a provider CLI lands when it is installed but NOT on the spawning
/// process's PATH — a GUI app launched from the tray inherits no shell PATH
/// fixups, so npm-globals and installer dirs are probed by name.
fn fallback_candidates(prog: &str) -> Vec<PathBuf> {
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    let mut out = vec![
        PathBuf::from(format!(r"{appdata}\npm\{prog}.cmd")),
        PathBuf::from(format!(r"{appdata}\npm\{prog}")),
        PathBuf::from(format!(r"{appdata}\npm\{prog}.exe")),
        PathBuf::from(format!(r"{local}\Programs\claude\{prog}.exe")),
        PathBuf::from(format!(r"{home}\.claude\local\{prog}.cmd")),
        PathBuf::from(format!(r"{home}\.claude\local\{prog}")),
        PathBuf::from(format!(r"{home}\.local\bin\{prog}")),
        PathBuf::from(format!(r"{home}\.volta\bin\{prog}.cmd")),
        PathBuf::from(format!(r"{local}\Programs\{prog}\{prog}.exe")),
    ];
    // Provider-specific installs (WINDOWS_FALLBACK_LOCATIONS): agy.exe under
    // %LOCALAPPDATA%\agy\bin.
    if prog.eq_ignore_ascii_case("agy") {
        out.push(PathBuf::from(format!(r"{local}\agy\bin\agy.exe")));
    }
    out
}

fn classify(p: PathBuf) -> Resolved {
    let script = p
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "bat" | "cmd"));
    if script {
        Resolved::Script(p)
    } else {
        Resolved::Exe(p)
    }
}

/// What an npm-style Windows `.cmd` shim actually runs, decoded from its text
/// — port of `parseNpmCmdShim` (pty.ts). Routing a shim through
/// `cmd.exe /d /s /c` re-enters cmd's parser, which cannot carry a newline and
/// reads `(`/`)` as block delimiters: the multi-line hive protocol prompt was
/// silently truncated there for every npm-installed CLI. Decoding the shim to
/// its real interpreter + script lets us spawn with an argv array instead —
/// no shell parser at all.
pub enum ShimTarget {
    /// `node/bun/deno <script> %*` — spawn interpreter with `[script, ...args]`.
    Interpreted { interpreter: String, script: PathBuf },
    /// Direct-executable shim (`"<exe>" %*` — opencode-ai's bin shape): the
    /// target IS the program, no interpreter needed.
    DirectExe { exe: PathBuf },
}

const SHIM_INTERPRETERS: &[&str] = &["node", "bun", "deno"];

/// Windows-path-only expand of `%~dp0`/`%dp0%` (the shim's own dir, WITH a
/// trailing backslash). Any other surviving `%` is a variable we do not model
/// → refuse, as upstream does.
fn shim_expand(raw: &str, shim_dir: &str) -> Option<PathBuf> {
    let mut s = raw
        .replace("%~dp0", &format!("{shim_dir}\\"))
        .replace("%dp0%", &format!("{shim_dir}\\"));
    if s.contains('%') || s.trim().is_empty() {
        return None;
    }
    // Collapse doubled separators (dir ends in `\`, target may start with `\`)
    // while keeping a leading UNC `\\`.
    let unc = s.starts_with("\\\\") || s.starts_with("//");
    let mut out = String::new();
    let mut last_sep = false;
    for c in s.chars() {
        let is_sep = c == '\\' || c == '/';
        if is_sep {
            if last_sep {
                continue;
            }
            out.push('\\');
        } else {
            out.push(c);
        }
        last_sep = is_sep;
    }
    s = out;
    if unc && !s.starts_with("\\\\") {
        s = format!("\\{s}");
    }
    let p = PathBuf::from(&s);
    (p.is_absolute()).then_some(p)
}

/// Decode an npm-generated Windows `.cmd` shim into the interpreter + script
/// (or direct exe) it would have run. Returns `None` — "fall back to the
/// cmd.exe path" — for ANY shape not fully understood: unknown interpreter,
/// unexpanded `%VAR%`, a relative target, extra interpreter flags, a non-JS
/// target, a file too large to be a shim, or garbage. Pure string work so it
/// is testable on any host (upstream tests it on macOS/Linux).
pub fn parse_npm_cmd_shim(shim_path: &str, content: &str) -> Option<ShimTarget> {
    if shim_path.is_empty() || content.is_empty() {
        return None;
    }
    // A cmd-shim is ~1KB; anything larger is a real batch script.
    if content.len() > 8192 || content.contains('\0') {
        return None;
    }
    let shim_dir = std::path::Path::new(shim_path)
        .parent()
        .map(|p| p.display().to_string())?;
    if shim_dir.is_empty() || shim_dir == "." {
        return None;
    }

    // Every `SET "_prog=…"` the shim performs (IF-EXISTS arm + PATH arm) —
    // they can appear MID-line (`IF EXIST … (SET "_prog=…") ELSE (SET …)`).
    let mut prog_values: Vec<String> = Vec::new();
    {
        let mut rest = content;
        while let Some(pos) = rest.find("SET") {
            rest = &rest[pos + 3..];
            let l = rest.trim_start().trim_start_matches('"');
            if let Some(v) = l.strip_prefix("_prog=") {
                let end = v.find(['"', '\r', '\n', ')']).unwrap_or(v.len());
                prog_values.push(v[..end].to_string());
            }
        }
    }

    // The exec line is the LAST line carrying `%*` (forward-all-args).
    let exec_line = content.lines().rev().find(|l| l.contains("%*"))?;
    let head = &exec_line[..exec_line.rfind("%*")?];

    // Quoted tokens in the exec head: the LAST is the script/exe target; the
    // one before it (if any) is the program — with nothing between them.
    let mut quoted: Vec<(usize, usize, &str)> = Vec::new(); // (start,end,text)
    let bytes = head.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            if let Some(end) = head[i + 1..].find('"') {
                quoted.push((i, i + 1 + end, &head[i + 1..i + 1 + end]));
                i += end + 2;
                continue;
            }
        }
        i += 1;
    }
    let &(s_start, _s_end, script_tok) = quoted.last()?;
    let prog_raw: String;
    if quoted.len() >= 2 {
        let &(_, p_end, p_tok) = &quoted[quoted.len() - 2];
        // Interpreter flags baked between program and script must not be
        // silently dropped → bail to the cmd.exe fallback.
        if head[p_end + 1..s_start].trim() != "" {
            return None;
        }
        prog_raw = p_tok.to_string();
    } else {
        // Two one-token shapes share this branch:
        let before = head[..s_start].trim().trim_start_matches('@').trim();
        if before.is_empty() {
            // DIRECT-EXECUTABLE shim — `"%dp0%\..\bin\x.exe" %*`.
            let exe = shim_expand(script_tok, &shim_dir)?;
            let lower = exe.display().to_string().to_lowercase();
            if !(lower.ends_with(".exe") || lower.ends_with(".com")) {
                return None;
            }
            return Some(ShimTarget::DirectExe { exe });
        }
        // Classic ELSE arm: `node  "<script>" %*` — program is a bare word.
        let word = before.split_whitespace().last()?;
        if word != before {
            return None;
        }
        prog_raw = word.to_string();
    }

    // `"%_prog%"` → the value the shim SET. Prefer the bare-name arm.
    let mut prog_raw = prog_raw;
    if prog_raw.eq_ignore_ascii_case("%_prog%") {
        prog_raw = prog_values
            .iter()
            .find(|v| !v.contains('%') && !v.contains('\\') && !v.contains('/'))
            .cloned()
            .or_else(|| prog_values.last().cloned())?;
    }
    let interpreter = prog_raw
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .trim_end_matches(".exe")
        .trim_end_matches(".EXE")
        .to_lowercase();
    if !SHIM_INTERPRETERS.contains(&interpreter.as_str()) {
        return None;
    }
    let script = shim_expand(script_tok, &shim_dir)?;
    let sc = script.display().to_string().to_lowercase();
    if !(sc.ends_with(".js") || sc.ends_with(".cjs") || sc.ends_with(".mjs")) {
        return None;
    }
    Some(ShimTarget::Interpreted { interpreter, script })
}

/// resolveWindowsShimSpawn (pty.ts): decode the resolved shim to a spawnable
/// target — interpreter exe + script argv-head, or the exe itself. `resolve`
/// finds `node`/`bun`/`deno` the same way the caller resolved the shim (PATH +
/// fallbacks) and must return an EXE — a `.cmd` node would hit the exact
/// CreateProcess limit we are routing around. Every miss returns `None` =
/// "use the cmd.exe path", never worse than before.
pub fn resolve_windows_shim_spawn<F>(resolved: &PathBuf, resolve: F) -> Option<(PathBuf, Option<PathBuf>)>
where
    F: Fn(&str) -> Option<Resolved>,
{
    if !cfg!(windows) {
        return None;
    }
    let lower = resolved.display().to_string().to_lowercase();
    let shim_path = if lower.ends_with(".cmd") || lower.ends_with(".bat") {
        resolved.clone()
    } else {
        // An extensionless npm shim is a POSIX sh script cmd can't run — its
        // `.cmd` sibling names the same target.
        let with_ext = PathBuf::from(format!("{}.cmd", resolved.display()));
        with_ext.is_file().then_some(with_ext)?
    };
    let meta = std::fs::metadata(&shim_path).ok()?;
    if !meta.is_file() || meta.len() > 8192 {
        return None;
    }
    let content = std::fs::read_to_string(&shim_path).ok()?;
    match parse_npm_cmd_shim(&shim_path.display().to_string(), &content)? {
        ShimTarget::DirectExe { exe } => {
            exe.is_file().then_some((exe, None))
        }
        ShimTarget::Interpreted { interpreter, script } => {
            if !script.is_file() {
                return None;
            }
            // The interpreter itself must resolve to a REAL exe — a `.cmd`
            // node would hit the same CreateProcess limit we route around.
            match resolve(&interpreter) {
                Some(Resolved::Exe(exe)) => Some((exe, Some(script))),
                _ => None,
            }
        }
    }
}

/// Quote one argument for a `cmd /d /s /c` line — port of upstream
/// `buildCmdCommandLine`'s quoteToken: quote on whitespace/quote AND on
/// cmd.exe metacharacters (`& | ^ < > ( ) % !`); an unquoted metachar would
/// let one token chain a second command once cmd re-parses the line.
/// Embedded `"` becomes `\"` exactly as upstream emits (the escape only works
/// because the FINAL program's CRT un-escapes it — cmd itself never sees this
/// path carrying prompts: shim-decoded spawns use argv).
///
/// NOTE: tokens produced here are meant to be passed as SEPARATE argv entries
/// to `spawn_cmd` — portable-pty CRT-quotes each arg correctly, so a token we
/// pre-quote must be handed to it with the quotes intact (they round-trip).
pub fn cmd_quote(arg: &str) -> String {
    if arg.is_empty() || arg.chars().any(|c| " \t\"&|^<>()%!".contains(c)) {
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        arg.to_string()
    }
}

/// A bare command name — the only shape resolved against PATH + fallback
/// dirs (shellEnv.ts isSafeCommandName). Anything else (metachars, separators
/// handled by the caller first) is refused rather than probed.
pub fn is_safe_command_name(command: &str) -> bool {
    !command.is_empty()
        && command
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
}

/// Spawn `cmd.exe /d /s /c` with each token as a SEPARATE argv entry.
/// portable-pty's append_quoted applies MSDN/CRT quoting per arg — tokens
/// containing spaces or quotes arrive wrapped in `"..."` with inner quotes
/// backslash-escaped, which is precisely the shape upstream's
/// buildCmdCommandLine emits verbatim. Passing the whole line as ONE arg (the
/// old path) made portable-pty wrap + escape it a SECOND time, so cmd saw
/// literal `\"` sequences and corrupted quoted args.
pub fn spawn_cmd(
    id: &str,
    tokens: &[String],
    cwd: &str,
    env: &HashMap<String, String>,
    cols: u16,
    rows: u16,
    app: AppHandle,
) -> Result<PtyHandle> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("openpty")?;

    let mut cmd = if cfg!(windows) {
        let mut c = CommandBuilder::new("cmd.exe");
        c.args(["/d", "/s", "/c"]);
        for t in tokens {
            // Tokens needing quoting (space/quote) are CRT-quoted by
            // portable-pty — cmd metachars inside "..." are inert. A bare
            // token with `&`/`|`/`^`/etc. would chain a second command, so
            // `^`-escape those (upstream neutralizes them by quoting; `^` is
            // the equivalent once the token reaches cmd unquoted).
            if t.chars().any(|c| c == ' ' || c == '\t' || c == '"') {
                c.arg(t.as_str());
            } else {
                let mut esc = String::with_capacity(t.len() + 4);
                for ch in t.chars() {
                    if "&|^<>()%!".contains(ch) {
                        esc.push('^');
                    }
                    esc.push(ch);
                }
                c.arg(esc);
            }
        }
        c
    } else {
        let mut c = CommandBuilder::new("sh");
        c.arg("-c");
        let line = tokens
            .iter()
            .map(|t| {
                if t.chars().any(|c| c == ' ' || c == '\t' || c == '"' || c == '\'') {
                    format!("'{}'", t.replace('\'', "'\\''"))
                } else {
                    t.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        c.arg(line);
        c
    };
    cmd.cwd(cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .with_context(|| format!("spawn {}", tokens.join(" ")))?;
    drop(pair.slave); // keep only the master; the slave side belongs to the child
    finish(id, pair.master, child, app)
}

/// Make a value safe inside a double-quoted cmd arg: single line, no double
/// quotes (an --append-system-prompt payload travels this way on script spawns).
pub fn cmd_sanitize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").replace('"', "'")
}

/// How much trailing PTY output to retain per session for crash diagnostics
/// and `pty:redraw` — upstream TAIL_MAX.
const TAIL_MAX: usize = 8192;

pub struct PtyHandle {
    writer: Box<dyn Write + Send>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    /// Root process id — ensureKilled's taskkill target on Windows.
    pub pid: Option<u32>,
    master: Box<dyn MasterPty + Send>,
    /// Epoch-ms of the last byte read — the heartbeat's quiet-floor signal
    /// (an agent printing/thinking counts as activity, same as upstream).
    pub last_output_at: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Trailing output buffer (TAIL_MAX cap) — pty:redraw replays this into a
    /// freshly-mounted xterm; the exit handler also ships it for diagnostics.
    tail: std::sync::Arc<std::sync::Mutex<String>>,
}

#[derive(Clone, Serialize)]
pub struct PtyOutputPayload {
    pub id: String,
    pub data: String,
}

#[derive(Clone, Serialize)]
pub struct PtyExitPayload {
    pub id: String,
    pub code: Option<u32>,
}

/// Spawn `command_line` inside a PTY. The command line is run through the
/// platform shell (`cmd /d /s /c` on Windows, `sh -c` elsewhere) so bare CLI
/// names like `claude` / `codex` resolve via PATH + PATHEXT exactly as they do
/// when the user types them — the Electron version relied on node-pty for this.
pub fn spawn(
    id: &str,
    command_line: &str,
    cwd: &str,
    env: &HashMap<String, String>,
    cols: u16,
    rows: u16,
    app: AppHandle,
) -> Result<PtyHandle> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("openpty")?;

    let mut cmd = if cfg!(windows) {
        let mut c = CommandBuilder::new("cmd.exe");
        c.args(["/d", "/s", "/c", command_line]);
        c
    } else {
        let mut c = CommandBuilder::new("sh");
        c.args(["-c", command_line]);
        c
    };
    cmd.cwd(cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .with_context(|| format!("spawn {command_line}"))?;
    drop(pair.slave); // keep only the master; the slave side belongs to the child
    finish(id, pair.master, child, app)
}

/// Direct argv spawn (resolved .exe) — the node-pty equivalent: no shell
/// between us and the child, so args like `--append-system-prompt <multiline
/// text>` arrive byte-exact.
pub fn spawn_argv(
    id: &str,
    program: &std::path::Path,
    args: &[String],
    cwd: &str,
    env: &HashMap<String, String>,
    cols: u16,
    rows: u16,
    app: AppHandle,
) -> Result<PtyHandle> {
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut cmd = CommandBuilder::new(program);
    cmd.args(args);
    cmd.cwd(cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);
    finish(id, pair.master, child, app)
}

fn finish(
    id: &str,
    master: Box<dyn MasterPty + Send>,
    mut child: Box<dyn Child + Send + Sync>,
    app: AppHandle,
) -> Result<PtyHandle> {
    let mut reader = master.try_clone_reader().context("clone reader")?;
    let writer = master.take_writer().context("take writer")?;
    let last_output_at = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    ));
    let clock = last_output_at.clone();
    let tail = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let tail_reader = tail.clone();
    let event_id = id.to_string();
    let app_reader = app.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    clock.store(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0),
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    let data = String::from_utf8_lossy(&buf[..n]).to_string();
                    {
                        let mut t = tail_reader.lock().unwrap();
                        t.push_str(&data);
                        if t.len() > TAIL_MAX {
                            let cut = t.len() - TAIL_MAX;
                            // Trim on a char boundary so the tail stays valid UTF-8.
                            let mut idx = cut;
                            while idx < t.len() && !t.is_char_boundary(idx) {
                                idx += 1;
                            }
                            t.drain(..idx);
                        }
                    }
                    let _ = app_reader.emit(
                        "pty-output",
                        PtyOutputPayload {
                            id: event_id.clone(),
                            data: data.clone(),
                        },
                    );
                    // Upstream's per-id channel (`pty:data:<id>`) — the
                    // preload payload there is the raw string.
                    let _ = app_reader.emit(
                        &format!("pty:data:{event_id}"),
                        serde_json::json!(data),
                    );
                }
                Err(_) => break,
            }
        }
    });
    // Separate waiter thread: the reader exits on PTY EOF but the process may
    // outlive it briefly — upstream's onExit reports the real exitCode, which
    // a "clean exit 0 vs crash" distinction (auto-install restart) needs. The
    // killer handle is cloned out first so PtyHandle::kill still works while
    // the waiter owns the child.
    let killer = child.clone_killer();
    let pid = child.process_id();
    let event_id2 = id.to_string();
    std::thread::spawn(move || {
        let code = match child.wait() {
            Ok(status) => Some(status.exit_code()),
            Err(_) => None,
        };
        let _ = app.emit(
            "pty-exit",
            PtyExitPayload {
                id: event_id2.clone(),
                code,
            },
        );
        // Upstream's per-id channel (`pty:exit:<id>`) — payload {exitCode,
        // signal} there; code maps to exitCode, signal is always null here.
        let _ = app.emit(
            &format!("pty:exit:{event_id2}"),
            serde_json::json!({ "exitCode": code, "signal": null }),
        );
        // Natural-exit teardown (upstream's exitHandler): a child that exits
        // on its own gets the same lifecycle cleanup as an explicit kill —
        // session removed, broker caps revoked, status stamped — so respawn
        // never reports "already running" for a dead PTY.
        if let Some(st) = app.try_state::<crate::AppState>() {
            st.ptys.lock().unwrap().remove(&event_id2);
            crate::integrations::BROKER.revoke(&event_id2);
            let _ = st.hive.set_status(&event_id2, "stopped");
            let _ = app.emit(
                "agent-status",
                serde_json::json!({ "id": event_id2, "status": "stopped" }),
            );
        }
        // pendingInstallRelaunch: this was an install PTY — a clean exit
        // re-arms the renderer's pooled terminal (`pty:relaunch:<id>`) and
        // re-runs the agent spawn, which now finds the installed binary.
        // A non-zero exit leaves the installer's output on screen.
        if code == Some(0) {
            if let Some(retry) = crate::commands::PENDING_INSTALLS
                .lock()
                .unwrap()
                .remove(&event_id2)
            {
                let _ = app.emit(&format!("pty:relaunch:{event_id2}"), serde_json::json!({}));
                let app3 = app.clone();
                let id3 = event_id2.clone();
                let _ = crate::commands::spawn_inner(app3, retry, Some(id3), None);
            }
        } else {
            crate::commands::PENDING_INSTALLS.lock().unwrap().remove(&event_id2);
        }
    });
    Ok(PtyHandle {
        writer,
        killer,
        pid,
        master,
        last_output_at,
        tail,
    })
}

impl PtyHandle {
    pub fn write(&mut self, data: &str) -> Result<()> {
        self.writer.write_all(data.as_bytes())?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    /// Trailing output buffer for `pty:redraw` (upstream PtyManager.redraw
    /// returns {ok, tail} so a remount replays the last TAIL_MAX bytes).
    pub fn tail(&self) -> String {
        self.tail.lock().unwrap().clone()
    }

    pub fn kill(&mut self) -> Result<()> {
        // The waiter thread reaps; kill only signals. ensureKilled
        // (procKill.ts): the ConPTY root dies first, then the process TREE is
        // swept — MCP servers and helper daemons the agent spawned would
        // otherwise be orphaned.
        let _ = self.killer.kill();
        #[cfg(windows)]
        if let Some(pid) = self.pid {
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
        Ok(())
    }
}

#[cfg(test)]
mod shim_tests {
    use super::*;

    const MODERN: &str = "@SET dp0=%~dp0\r\nIF EXIST \"%dp0%\\node.exe\" (SET \"_prog=%dp0%\\node.exe\") ELSE (SET \"_prog=node\")\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\" \"%dp0%\\node_modules\\@anthropic-ai\\claude-code\\cli.js\" %*\r\n";

    const CLASSIC: &str = "@IF EXIST \"%~dp0\\node.exe\" (\r\n  \"%~dp0\\node.exe\" \"%~dp0\\node_modules\\pkg\\bin\\cli.js\" %*\r\n) ELSE (\r\n  node \"%~dp0\\node_modules\\pkg\\bin\\cli.js\" %*\r\n)\r\n";

    const DIRECT_EXE: &str = "@\"%~dp0\\..\\opencode-ai\\bin\\opencode.exe\" %*\r\n";

    #[test]
    fn modern_shim_decodes_to_node_plus_cli() {
        let t = parse_npm_cmd_shim(r"C:\Users\u\AppData\Roaming\npm\claude.cmd", MODERN).unwrap();
        match t {
            ShimTarget::Interpreted { interpreter, script } => {
                assert_eq!(interpreter, "node");
                assert!(script.display().to_string().replace('/', "\\").ends_with(
                    r"npm\node_modules\@anthropic-ai\claude-code\cli.js"
                ));
            }
            _ => panic!("expected interpreted"),
        }
    }

    #[test]
    fn classic_if_else_decodes() {
        let t = parse_npm_cmd_shim(r"C:\npm\codex.cmd", CLASSIC).unwrap();
        match t {
            ShimTarget::Interpreted { interpreter, script } => {
                assert_eq!(interpreter, "node");
                assert!(script.display().to_string().ends_with(r"bin\cli.js"));
            }
            _ => panic!("expected interpreted"),
        }
    }

    #[test]
    fn direct_exe_shim_needs_no_interpreter() {
        let t = parse_npm_cmd_shim(r"C:\npm\opencode.cmd", DIRECT_EXE).unwrap();
        match t {
            ShimTarget::DirectExe { exe } => {
                assert!(exe.display().to_string().ends_with("opencode.exe"));
            }
            _ => panic!("expected direct exe"),
        }
    }

    #[test]
    fn garbage_and_oversize_refused() {
        assert!(parse_npm_cmd_shim(r"C:\x.cmd", "echo hello").is_none());
        assert!(parse_npm_cmd_shim(r"C:\x.cmd", &"x".repeat(9000)).is_none());
        assert!(parse_npm_cmd_shim(r"C:\x.cmd", "").is_none());
    }

    #[test]
    fn interpreter_flags_between_prog_and_script_refused() {
        let s = "@rem shim\nnode --inspect \"%~dp0\\cli.js\" %*\n";
        assert!(parse_npm_cmd_shim(r"C:\npm\x.cmd", s).is_none());
    }

    #[test]
    fn unknown_interpreter_refused() {
        let s = "@rem\npython \"%~dp0\\cli.py\" %*\n";
        assert!(parse_npm_cmd_shim(r"C:\npm\x.cmd", s).is_none());
    }
}
