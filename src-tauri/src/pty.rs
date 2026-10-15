//! PTY manager — port of `src/main/pty.ts` (node-pty) onto `portable-pty`.
//!
//! Each agent CLI runs as a real process in a pseudo-terminal. Output is read
//! on a dedicated thread per PTY and forwarded to the frontend as
//! `pty-output` events; input flows back via `pty_write`. Semantics mirror the
//! Electron version byte-for-byte where the crates allow it.

use crate::lock_ext::LockExt;
use crate::panic_guard;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::{AppHandle, Manager};

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
    // The official hermes installer lands its shims under %LOCALAPPDATA%\
    // hermes\bin (with the venv under hermes-agent) — a running app holds the
    // PATH it was launched with, so the install-ladder relaunch resolves
    // through here even before a fresh PATH reaches it.
    if prog.eq_ignore_ascii_case("hermes") {
        out.push(PathBuf::from(format!(r"{local}\hermes\bin\hermes.exe")));
        out.push(PathBuf::from(format!(r"{local}\hermes\bin\hermes.cmd")));
        out.push(PathBuf::from(format!(
            r"{local}\hermes\hermes-agent\venv\Scripts\hermes.exe"
        )));
    }
    // OpenClaw's install.ps1 lays down a PRIVATE portable Node + npm prefix
    // under %LOCALAPPDATA%\OpenClaw\deps\portable-node — the openclaw.cmd shim
    // lives there, not in %APPDATA%\npm, and the running app's PATH never sees
    // the installer's registry update.
    if prog.eq_ignore_ascii_case("openclaw") {
        out.push(PathBuf::from(format!(
            r"{local}\OpenClaw\deps\portable-node\openclaw.cmd"
        )));
        out.push(PathBuf::from(format!(
            r"{local}\OpenClaw\deps\portable-node\openclaw"
        )));
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
    Interpreted {
        interpreter: String,
        script: PathBuf,
    },
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
    Some(ShimTarget::Interpreted {
        interpreter,
        script,
    })
}

/// resolveWindowsShimSpawn (pty.ts): decode the resolved shim to a spawnable
/// target — interpreter exe + script argv-head, or the exe itself. `resolve`
/// finds `node`/`bun`/`deno` the same way the caller resolved the shim (PATH +
/// fallbacks) and must return an EXE — a `.cmd` node would hit the exact
/// CreateProcess limit we are routing around. Every miss returns `None` =
/// "use the cmd.exe path", never worse than before.
pub fn resolve_windows_shim_spawn<F>(
    resolved: &PathBuf,
    resolve: F,
) -> Option<(PathBuf, Option<PathBuf>)>
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
        ShimTarget::DirectExe { exe } => exe.is_file().then_some((exe, None)),
        ShimTarget::Interpreted {
            interpreter,
            script,
        } => {
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

/// The user's interactive-shell PATH (shellEnv.ts userShellPath).
///
/// A GUI-launched app on macOS inherits no login-shell PATH, so a bare
/// `claude` installed via nvm/homebrew would ENOENT. We spawn the user's
/// interactive login shell once, capture its PATH between two fence markers
/// (rc-file chatter like "Restored session: …" would otherwise poison the
/// value), and cache the result for the session.
///
/// Windows is the OPPOSITE of a no-login-shell case: installers write the
/// registry PATH (HKLM + HKCU Environment) between spawns, and our own
/// auto-install relaunch must see the directory the installer JUST added.
/// The process PATH is fixed at process start — a cached PATH would keep
/// the freshly-installed CLI invisible — so on Windows the merged registry
/// PATH (machine + user, expanded) is rebuilt on every call.
static SHELL_PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn user_shell_path() -> String {
    if cfg!(windows) {
        return windows_registry_path();
    }
    if let Some(p) = SHELL_PATH.get() {
        return p.clone();
    }
    let path = capture_shell_path();
    let v = SHELL_PATH.get_or_init(|| path);
    v.clone()
}

/// Windows: merge the HKLM Session Manager + HKCU Environment PATH values
/// (the same order Windows composes them) with the process PATH as a tail —
/// per-spawn, never cached, so a binary installed seconds ago resolves.
/// REG_EXPAND_SZ entries carry %VAR% refs; they expand against our env map.
#[cfg(windows)]
fn windows_registry_path() -> String {
    use std::collections::HashMap as Map;
    use winreg::enums::*;
    use winreg::RegKey;

    let mut env_map: Map<String, String> = Map::new();
    for (k, v) in std::env::vars() {
        env_map.insert(k.to_uppercase(), v);
    }
    let expand = |s: &str| -> String {
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        while let Some(i) = rest.find('%') {
            out.push_str(&rest[..i]);
            let tail = &rest[i + 1..];
            match tail.find('%') {
                Some(j) => {
                    let name = tail[..j].to_uppercase();
                    match env_map.get(&name) {
                        Some(v) => out.push_str(v),
                        None => {
                            out.push('%');
                            out.push_str(&tail[..j]);
                            out.push('%');
                        }
                    }
                    rest = &tail[j + 1..];
                }
                None => {
                    out.push('%');
                    out.push_str(tail);
                    rest = "";
                }
            }
        }
        out.push_str(rest);
        out
    };

    let read_key = |root: winreg::HKEY, sub: &str| -> Option<String> {
        RegKey::predef(root)
            .open_subkey(sub)
            .ok()
            .and_then(|k| k.get_value::<String, _>("Path").ok())
    };
    let machine = read_key(
        HKEY_LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
    );
    let user = read_key(HKEY_CURRENT_USER, "Environment");

    // Dedup case-insensitively; machine first, then user, then process extras.
    let mut seen = std::collections::HashSet::new();
    let mut parts: Vec<String> = Vec::new();
    for src in [machine, user, std::env::var("PATH").ok()].into_iter().flatten() {
        for dir in expand(&src).split(';') {
            let d = dir.trim();
            if !d.is_empty() && seen.insert(d.to_lowercase()) {
                parts.push(d.to_string());
            }
        }
    }
    parts.join(";")
}

fn capture_shell_path() -> String {
    if cfg!(windows) {
        return std::env::var("PATH").unwrap_or_default();
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let mark = "__MD_SHELL_FENCE__";
    let script = format!("printf %s {mark}; printf %s \"$PATH\"; printf %s {mark}");
    let Ok(out) = std::process::Command::new(&shell)
        .args(["-ilc", &script])
        .output()
    else {
        return std::env::var("PATH").unwrap_or_default();
    };
    let s = String::from_utf8_lossy(&out.stdout);
    let start = s.find(mark);
    let end = s.rfind(mark);
    match (start, end) {
        (Some(st), Some(en)) if en > st => {
            let captured = s[st + mark.len()..en].to_string();
            // A PATH is a single colon-joined line; multi-line = rc noise.
            if !captured.is_empty() && !captured.contains('\n') {
                captured
            } else {
                std::env::var("PATH").unwrap_or_default()
            }
        }
        _ => std::env::var("PATH").unwrap_or_default(),
    }
}

/// Spawn `cmd.exe /d /s /c` with each token as a SEPARATE argv entry.
/// portable-pty's append_quoted applies MSDN/CRT quoting per arg — tokens
/// containing spaces or quotes arrive wrapped in `"..."` with inner quotes
/// backslash-escaped, which is precisely the shape upstream's
/// buildCmdCommandLine emits verbatim. Passing the whole line as ONE arg (the
/// old path) made portable-pty wrap + escape it a SECOND time, so cmd saw
/// literal `\"` sequences and corrupted quoted args.
pub fn spawn_cmd<R: tauri::Runtime>(
    id: &str,
    tokens: &[String],
    cwd: &str,
    env: &HashMap<String, String>,
    cols: u16,
    rows: u16,
    app: AppHandle<R>,
    owner: &str,
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
                if t.chars()
                    .any(|c| c == ' ' || c == '\t' || c == '"' || c == '\'')
                {
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
    finish(id, pair.master, child, cwd.to_string(), tokens.join(" "), cols, rows, app, owner)
}

/// Make a value safe inside a double-quoted cmd arg: single line, no double
/// quotes (an --append-system-prompt payload travels this way on script spawns).
pub fn cmd_sanitize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('"', "'")
}

/// How much trailing PTY output to retain per session for crash diagnostics
/// and `pty:redraw` — upstream TAIL_MAX.
const TAIL_MAX: usize = 8192;

/// Unique per spawn. The natural-exit waiter compares it against the handle
/// currently in `st.ptys` so a late waiter from a killed agent can never tear
/// down a respawned agent that reuses the same pty id.
static NEXT_SPAWN_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Shared, clonable write half of a PTY. Callers clone it out from under the
/// global `ptys` mutex and write AFTER releasing it — a ConPTY write can block
/// when the child isn't draining stdin, and must not stall every other lookup.
#[derive(Clone)]
pub struct PtyWriter(
    std::sync::Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
    CursorGate,
);

/// ConPTY (portable-pty always passes `--inheritcursor`) opens every session by
/// asking the terminal where the cursor is (`ESC[6n`) and holds the child until
/// a reply arrives. The renderer's xterm answers; with no terminal view attached
/// (background/headless spawns, a floor window not mounted yet, the gap between
/// spawn and xterm attach) nobody does and the agent hangs. So when the very
/// first output carries the query and no terminal has taken over within
/// `CPR_GRACE`, the reader answers `ESC[1;1R` ONCE. State machine, so xterm and
/// this fallback can never both answer: `Armed` until either a terminal shows up
/// (`Terminal`: a cursor report was written by anyone, or `pty_redraw` - the
/// terminal-mounted signal - ran) or the fallback fires (`Answered`, which also
/// scrubs the query from the replay tail so a late-mounting xterm does not
/// answer a second time). Plain `pty_write`/`pty_resize` deliberately do not
/// count: the renderer seeds and resizes PTYs it has no terminal view for.
const CPR_QUERY: &[u8] = b"\x1b[6n";
const CPR_REPLY: &str = "\x1b[1;1R";
const CPR_GRACE: std::time::Duration = std::time::Duration::from_millis(1500);
/// The query is ConPTY's session-start probe: only look in the first output.
const CPR_SCAN_MAX: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CursorState {
    Armed,
    Terminal,
    Answered,
}

#[derive(Clone)]
struct CursorGate(std::sync::Arc<std::sync::Mutex<CursorState>>);

impl CursorGate {
    fn new() -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(CursorState::Armed)))
    }

    #[cfg(test)]
    fn state(&self) -> CursorState {
        *self.0.lock_safe()
    }

    /// A terminal is (about to be) answering the query: the fallback stands down.
    fn terminal_attached(&self) {
        let mut s = self.0.lock_safe();
        if *s == CursorState::Armed {
            *s = CursorState::Terminal;
        }
    }

    /// Claim the single fallback answer (Armed -> Answered), running `scrub`
    /// under the same lock so a concurrent `pty_redraw` replays either the
    /// untouched tail (and its terminal answers) or the scrubbed one.
    fn claim_answer(&self, scrub: impl FnOnce()) -> bool {
        let mut s = self.0.lock_safe();
        if *s != CursorState::Armed {
            return false;
        }
        *s = CursorState::Answered;
        scrub();
        true
    }
}

/// Does `data` contain a cursor position report (`ESC[<row>;<col>R`) - the
/// answer to `ESC[6n`, however it reached the PTY?
fn contains_cursor_report(data: &str) -> bool {
    let b = data.as_bytes();
    let mut i = 0;
    while let Some(p) = b[i..].windows(2).position(|w| w == b"\x1b[") {
        let mut j = i + p + 2;
        let digits = |j: &mut usize| {
            let from = *j;
            while *j < b.len() && b[*j].is_ascii_digit() {
                *j += 1;
            }
            *j > from
        };
        if digits(&mut j) && j < b.len() && b[j] == b';' {
            j += 1;
            if digits(&mut j) && j < b.len() && b[j] == b'R' {
                return true;
            }
        }
        i += p + 2;
    }
    false
}

impl PtyWriter {
    pub fn write(&self, data: &str) -> Result<()> {
        if contains_cursor_report(data) {
            self.1.terminal_attached();
        }
        let mut w = self.0.lock_safe();
        w.write_all(data.as_bytes())?;
        w.flush()?;
        Ok(())
    }
}

/// Write `data` to the PTY `pty_id` without holding the `ptys` map lock across
/// the (potentially blocking) write. Returns false when the PTY is gone or the
/// write failed.
pub fn write_pty(st: &crate::AppState, pty_id: &str, data: &str) -> bool {
    let w = st.ptys.lock_safe().get(pty_id).map(|h| h.writer());
    w.is_some_and(|w| w.write(data).is_ok())
}

/// Carry-over UTF-8 decode for the reader thread: append `chunk` to `pending`,
/// return everything that is valid text (genuinely invalid bytes become U+FFFD)
/// and leave an INCOMPLETE trailing sequence in `pending` for the next read, so
/// a multibyte char split across two reads is not mangled into U+FFFD.
fn decode_utf8_chunk(pending: &mut Vec<u8>, chunk: &[u8]) -> String {
    pending.extend_from_slice(chunk);
    let mut out = String::new();
    let mut start = 0;
    loop {
        match std::str::from_utf8(&pending[start..]) {
            Ok(s) => {
                out.push_str(s);
                start = pending.len();
                break;
            }
            Err(e) => {
                let good = start + e.valid_up_to();
                out.push_str(&String::from_utf8_lossy(&pending[start..good]));
                match e.error_len() {
                    Some(n) => {
                        out.push('\u{FFFD}');
                        start = good + n;
                    }
                    // Incomplete sequence at the end of the buffer — wait for more.
                    None => {
                        start = good;
                        break;
                    }
                }
            }
        }
    }
    pending.drain(..start);
    out
}

pub struct PtyHandle {
    /// Label of the window that spawned this PTY (`main` or `floor-<n>`) — the
    /// ONLY window `pty:data` / `pty:exit` / `pty:relaunch` are routed to
    /// (upstream PtySession.owner). See `crate::floors`.
    pub owner: String,
    writer: std::sync::Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
    /// Cursor-query fallback state - see `CursorGate`.
    cursor: CursorGate,
    /// Spawn generation — see `NEXT_SPAWN_GEN`.
    pub spawn_gen: u64,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    /// Root process id — ensureKilled's taskkill target on Windows.
    pub pid: Option<u32>,
    master: Box<dyn MasterPty + Send>,
    /// Spawn cwd + resolved command line — upstream PtySession fields, replayed
    /// by `pty:list` for the renderer's waitForTerminalReady.
    pub cwd: String,
    pub command: String,
    /// Epoch-ms of the last byte read — the heartbeat's quiet-floor signal
    /// (an agent printing/thinking counts as activity, same as upstream).
    pub last_output_at: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// True once the reader has seen ANY byte — upstream PtySession.hasOutput,
    /// the renderer's "the TUI actually booted" signal in waitForTerminalReady.
    pub has_output: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Trailing output buffer (TAIL_MAX cap) — pty:redraw replays this into a
    /// freshly-mounted xterm; the exit handler also ships it for diagnostics.
    tail: std::sync::Arc<std::sync::Mutex<String>>,
    /// Current grid size — pty:redraw's same-size resize (upstream semantics)
    /// needs it; updated by resize().
    size: std::sync::Mutex<(u16, u16)>,
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
pub fn spawn<R: tauri::Runtime>(
    id: &str,
    command_line: &str,
    cwd: &str,
    env: &HashMap<String, String>,
    cols: u16,
    rows: u16,
    app: AppHandle<R>,
    owner: &str,
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
    finish(
        id,
        pair.master,
        child,
        cwd.to_string(),
        command_line.to_string(),
        cols,
        rows,
        app,
        owner,
    )
}

/// Direct argv spawn (resolved .exe) — the node-pty equivalent: no shell
/// between us and the child, so args like `--append-system-prompt <multiline
/// text>` arrive byte-exact.
pub fn spawn_argv<R: tauri::Runtime>(
    id: &str,
    program: &std::path::Path,
    args: &[String],
    cwd: &str,
    env: &HashMap<String, String>,
    cols: u16,
    rows: u16,
    app: AppHandle<R>,
    owner: &str,
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
    let command = std::iter::once(program.display().to_string())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    finish(id, pair.master, child, cwd.to_string(), command, cols, rows, app, owner)
}

fn finish<R: tauri::Runtime>(
    id: &str,
    master: Box<dyn MasterPty + Send>,
    mut child: Box<dyn Child + Send + Sync>,
    cwd: String,
    command: String,
    cols: u16,
    rows: u16,
    app: AppHandle<R>,
    owner: &str,
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
    let has_output = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let saw_output = has_output.clone();
    let tail = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let tail_reader = tail.clone();
    let event_id = id.to_string();
    let app_reader = app.clone();
    // Output routes ONLY to the window that spawned this PTY (see floors.rs).
    let owner_reader = owner.to_string();
    let owner_exit = owner.to_string();
    let writer = std::sync::Arc::new(std::sync::Mutex::new(writer));
    let cursor = CursorGate::new();
    let cpr_writer = PtyWriter(writer.clone(), cursor.clone());
    let cpr_gate = cursor.clone();
    let cpr_tail = tail.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        // Bytes of a multibyte char split across two reads.
        let mut pending: Vec<u8> = Vec::new();
        // First output, scanned for ConPTY's cursor query until it is found
        // (or the scan window is spent) - see `CursorGate`. Windows only.
        let mut early: Option<Vec<u8>> = cfg!(windows).then(Vec::new);
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if let Some(e) = early.as_mut() {
                        e.extend_from_slice(&buf[..n]);
                        if e.windows(CPR_QUERY.len()).any(|w| w == CPR_QUERY) {
                            early = None;
                            let (gate, w, t) = (cpr_gate.clone(), cpr_writer.clone(), cpr_tail.clone());
                            std::thread::spawn(move || {
                                std::thread::sleep(CPR_GRACE);
                                let answered = gate.claim_answer(|| {
                                    let mut t = t.lock_safe();
                                    *t = t.replace("\u{1b}[6n", "");
                                });
                                if answered {
                                    let _ = w.write(CPR_REPLY);
                                }
                            });
                        } else if e.len() >= CPR_SCAN_MAX {
                            early = None;
                        }
                    }
                    panic_guard::guarded("pty-reader-chunk", || {
                        saw_output.store(true, std::sync::atomic::Ordering::Relaxed);
                        clock.store(
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0),
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        let data = decode_utf8_chunk(&mut pending, &buf[..n]);
                        if data.is_empty() {
                            return;
                        }
                        {
                            let mut t = tail_reader.lock_safe();
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
                        crate::floors::emit_to_owner(
                            &app_reader,
                            &owner_reader,
                            "pty-output",
                            PtyOutputPayload {
                                id: event_id.clone(),
                                data: data.clone(),
                            },
                        );
                        // Upstream's per-id channel (`pty:data:<id>`) — the
                        // preload payload there is the raw string.
                        crate::floors::emit_to_owner(
                            &app_reader,
                            &owner_reader,
                            &format!("pty:data:{event_id}"),
                            serde_json::json!(data),
                        );
                    });
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
    let spawn_gen = NEXT_SPAWN_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // By-value copies for the exit handler: the handle (and its tail) is
    // dropped from the map before the handler runs.
    let tail_exit = tail.clone();
    let command_exit = command.clone();
    std::thread::spawn(move || {
        // `signal` is the killed-by-signal name (unix; always None on Windows):
        // a SIGILL/SIGSEGV death must not read as a clean finish.
        let (code, signal) = match child.wait() {
            Ok(status) => (
                Some(status.exit_code()),
                status.signal().map(|s| s.to_string()),
            ),
            Err(_) => (None, None),
        };
        // The reader thread may still be draining the panic banner the child
        // printed on its way out — give it a beat before snapshotting the tail.
        if signal.is_some() || code.is_some_and(|c| c != 0) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let trace = ExitTrace {
            signal: signal.clone(),
            tail: tail_exit.lock_safe().clone(),
            command: command_exit,
        };
        // Is the handle currently registered under this pty id OURS? A killed
        // agent's waiter can fire after the id was respawned — it must not
        // remove/unmap/finalize the NEW agent. `Some(true)` = ours (removed
        // here), `Some(false)` = a newer spawn owns the id, `None` = nothing
        // registered (an explicit kill already ran the teardown).
        let ours: Option<bool> = app.try_state::<crate::AppState>().and_then(|st| {
            let mut ptys = st.ptys.lock_safe();
            match ptys.get(&event_id2).map(|h| h.spawn_gen) {
                Some(g) if g == spawn_gen => {
                    ptys.remove(&event_id2);
                    Some(true)
                }
                Some(_) => Some(false),
                None => {
                    // Nothing registered under this id: either an explicit
                    // kill already tore it down, OR the child exited before
                    // spawn_inner inserted the handle (a ~1ms-lived child).
                    // Leave a marker — recorded under the ptys lock so it is
                    // atomic with the "nothing registered" check — so the
                    // inserter can tell the two apart (see
                    // `reap_if_already_exited`).
                    EARLY_EXITS
                        .lock_safe()
                        .insert(event_id2.clone(), (spawn_gen, code));
                    EARLY_EXIT_TRACES
                        .lock_safe()
                        .insert(event_id2.clone(), (spawn_gen, trace.clone()));
                    None
                }
            }
        });
        if ours == Some(false) {
            return;
        }
        panic_guard::guarded("pty-exit-emit-1", || {
            crate::floors::emit_to_owner(
                &app,
                &owner_exit,
                "pty-exit",
                PtyExitPayload {
                    id: event_id2.clone(),
                    code,
                },
            );
        });
        // Upstream's per-id channel (`pty:exit:<id>`) — payload {exitCode,
        // signal}; signal is the killed-by-signal name on unix, null otherwise.
        panic_guard::guarded("pty-exit-emit-2", || {
            crate::floors::emit_to_owner(
                &app,
                &owner_exit,
                &format!("pty:exit:{event_id2}"),
                serde_json::json!({ "exitCode": code, "signal": signal }),
            );
        });
        // Natural-exit teardown (upstream's exitHandler, index.ts:595): a
        // child that exits on its own gets the same lifecycle cleanup as an
        // explicit kill — session removed, the pty↔agent mapping dropped,
        // broker caps revoked, status stamped — so respawn never reports
        // "already running" for a dead PTY.
        // (An explicit kill already tore the agent down — nothing left to do.)
        if ours == Some(true) {
            panic_guard::guarded("pty-natural-exit-teardown", || {
                finish_natural_exit(&app, &event_id2, code, Some(trace));
            });
        }
    });
    Ok(PtyHandle {
        owner: owner.to_string(),
        writer,
        cursor,
        spawn_gen,
        killer,
        pid,
        master,
        cwd,
        command,
        last_output_at,
        has_output,
        tail,
        size: std::sync::Mutex::new((cols, rows)),
    })
}

/// Synchronously sweep the process TREES rooted at `pids` (Windows:
/// `taskkill /T /F`, all launched at once then waited on, so N agents cost one
/// taskkill's latency at quit). conpty's own kill is async and best-effort, so
/// this is the only reliable reaper when the app is about to exit — a deferred
/// sweep would never run. POSIX: a no-op here (closing the pty HUPs the
/// foreground process group).
pub fn kill_trees(pids: &[u32]) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let children: Vec<std::process::Child> = pids
            .iter()
            .filter_map(|pid| {
                std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .ok()
            })
            .collect();
        for mut c in children {
            let _ = c.wait();
        }
    }
    #[cfg(not(windows))]
    let _ = pids;
}

/// Live PTYs owned by the window labelled `owner` — a floor's close
/// confirmation is scoped to ITS terminals, not the whole app's (upstream
/// PtyManager.countByOwner).
pub fn count_by_owner(ptys: &HashMap<String, PtyHandle>, owner: &str) -> usize {
    ptys.values().filter(|h| h.owner == owner).count()
}

/// Kill every PTY owned by `owner` (upstream PtyManager.killByOwner) so a
/// closed floor never leaves its terminals running headless. Like upstream it
/// only signals: the handles stay in the map and each PTY's waiter thread
/// removes it and runs the normal natural-exit teardown (archive + worktree
/// cleanup). Process trees are swept first, in one parallel taskkill batch
/// and outside the map lock. Returns how many PTYs were signalled.
pub fn kill_by_owner(
    ptys: &std::sync::Mutex<HashMap<String, PtyHandle>>,
    owner: &str,
) -> usize {
    let pids: Vec<u32> = ptys
        .lock_safe()
        .values()
        .filter(|h| h.owner == owner)
        .filter_map(|h| h.pid)
        .collect();
    kill_trees(&pids);
    let mut n = 0;
    {
        let mut m = ptys.lock_safe();
        for h in m.values_mut().filter(|h| h.owner == owner) {
            let _ = h.kill_root();
            n += 1;
        }
    }
    n
}

/// Exits whose waiter found no handle registered, keyed by pty id →
/// `(spawn_gen, exit code)`. Tiny (one entry per pty id at most); consumed by
/// `reap_if_already_exited`, and stale entries from explicit kills never match
/// a later spawn because the generation differs.
pub(crate) static EARLY_EXITS: std::sync::LazyLock<
    std::sync::Mutex<HashMap<String, (u64, Option<u32>)>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// What the exit handler is told about a process that just died (upstream
/// PtyExitInfo). Passed by value: the handle is already out of the map.
#[derive(Clone, Default)]
pub(crate) struct ExitTrace {
    /// Killed-by-signal name (unix only).
    pub signal: Option<String>,
    /// Trailing PTY output (TAIL_MAX).
    pub tail: String,
    pub command: String,
}

/// The `ExitTrace` twin of `EARLY_EXITS` (same key + generation) — kept as a
/// separate map so EARLY_EXITS' shape stays `(gen, code)`.
static EARLY_EXIT_TRACES: std::sync::LazyLock<
    std::sync::Mutex<HashMap<String, (u64, ExitTrace)>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// The lifecycle cleanup for a child that exited on its own (shared by the
/// waiter thread and the late-insert path):
///
/// pendingInstallRelaunch: this was an install PTY — a clean exit re-arms the
/// renderer's pooled terminal (`pty:relaunch:<id>`) and re-runs the SAME spawn
/// (same agent id, same resume plan), which now finds the installed binary.
/// An install PTY carries no pty→agent mapping, so teardown is skipped — same
/// early return as upstream. Otherwise teardownPty(id): agent-keyed cleanup
/// resolves the owning agent through the map — `pty_id` is a PTY id.
pub(crate) fn finish_natural_exit<R: tauri::Runtime>(
    app: &AppHandle<R>,
    pty_id: &str,
    code: Option<u32>,
    trace: Option<ExitTrace>,
) {
    // Record an ABNORMAL death BEFORE teardown (upstream index.ts exitHandler):
    // teardown drops the pty→agent mapping, after which we can no longer say
    // whose process died. A clean exit writes nothing; install PTYs carry no
    // mapping and are skipped.
    if let (Some(t), Some(st)) = (&trace, app.try_state::<crate::AppState>()) {
        if let Some(agent_id) = st.agent_for_pty(pty_id) {
            st.hive.record_agent_exit(
                &agent_id,
                code,
                t.signal.as_deref(),
                &t.tail,
                &t.command,
            );
        }
    }
    if code == Some(0) {
        if let Some(pending) = crate::commands::PENDING_INSTALLS
            .lock_safe()
            .remove(pty_id)
        {
            crate::floors::emit_to_owner(
                app,
                pending.plan.owner.as_deref().unwrap_or(crate::floors::MAIN_LABEL),
                &format!("pty:relaunch:{pty_id}"),
                serde_json::json!({}),
            );
            let mut plan = pending.plan;
            plan.pty_id = Some(pty_id.to_string());
            let _ = crate::commands::spawn_inner(app.clone(), pending.args, plan);
            return;
        }
    } else {
        crate::commands::PENDING_INSTALLS
            .lock_safe()
            .remove(pty_id);
    }
    crate::commands::teardown_pty_core(app, pty_id, "stopped");
}

/// Close the spawn-vs-waiter race: `spawn` starts the waiter thread before
/// the caller can insert the handle into `AppState.ptys` and map it to its
/// agent. A child that exits within ~1ms runs its teardown first (nothing
/// registered yet / no mapping yet), and the insert then leaves a dead handle
/// — respawn would say "already running" forever. Callers invoke this AFTER
/// the handle is inserted and mapped; if the child already exited, the dead
/// handle is removed and the same natural-exit cleanup runs now.
pub(crate) fn reap_if_already_exited<R: tauri::Runtime>(
    app: &AppHandle<R>,
    pty_id: &str,
    spawn_gen: u64,
) {
    let Some(st) = app.try_state::<crate::AppState>() else {
        return;
    };
    let early = {
        let mut m = EARLY_EXITS.lock_safe();
        match m.get(pty_id) {
            Some((g, _)) if *g == spawn_gen => m.remove(pty_id).map(|(_, c)| c),
            _ => None,
        }
    };
    let trace = {
        let mut m = EARLY_EXIT_TRACES.lock_safe();
        match m.get(pty_id) {
            Some((g, _)) if *g == spawn_gen => m.remove(pty_id).map(|(_, t)| t),
            _ => None,
        }
    };
    if let Some(code) = early {
        {
            let mut ptys = st.ptys.lock_safe();
            if ptys.get(pty_id).map(|h| h.spawn_gen) == Some(spawn_gen) {
                ptys.remove(pty_id);
            } else {
                return; // already replaced/removed by someone else
            }
        }
        finish_natural_exit(app, pty_id, code, trace);
        return;
    }
    // Narrower window: the waiter ran between the insert and `map_pty` — its
    // teardown found no mapping, and map_pty then recorded a stale one for a
    // handle that is already gone.
    let gone = !st.ptys.lock_safe().contains_key(pty_id);
    if gone && st.agent_for_pty(pty_id).is_some() {
        crate::commands::teardown_pty_core(app, pty_id, "stopped");
    }
}

impl PtyHandle {
    pub fn write(&mut self, data: &str) -> Result<()> {
        self.writer().write(data)
    }

    /// Clonable write half — see `PtyWriter` / `write_pty`.
    pub fn writer(&self) -> PtyWriter {
        PtyWriter(self.writer.clone(), self.cursor.clone())
    }

    /// A terminal view just mounted on this PTY (`pty_redraw` replays the tail
    /// into it, query included, and it will answer): stand the fallback down.
    pub fn terminal_attached(&self) {
        self.cursor.terminal_attached();
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        *self.size.lock_safe() = (cols, rows);
        Ok(())
    }

    /// Current (cols, rows) — for the same-size redraw nudge.
    pub fn size(&self) -> (u16, u16) {
        *self.size.lock_safe()
    }

    /// Trailing output buffer for `pty:redraw` (upstream PtyManager.redraw
    /// returns {ok, tail} so a remount replays the last TAIL_MAX bytes).
    pub fn tail(&self) -> String {
        self.tail.lock_safe().clone()
    }

    pub fn kill(&mut self) -> Result<()> {
        // The waiter thread reaps; kill only signals. ensureKilled
        // (procKill.ts): MCP servers and helper daemons the agent spawned would
        // otherwise be orphaned, so the process TREE is swept — and FIRST, while
        // the root is still alive: once the ConPTY root exits, taskkill can no
        // longer enumerate descendants by its PID (upstream hardKillTree runs
        // before proc.kill()). taskkill is waited on so the sweep has really
        // happened by the time this returns.
        if let Some(pid) = self.pid {
            kill_trees(&[pid]);
        }
        self.kill_root()
    }

    /// Signal ONLY the root process (the tree was swept already — see
    /// `kill_trees`). The waiter thread reaps.
    pub fn kill_root(&mut self) -> Result<()> {
        let _ = self.killer.kill();
        Ok(())
    }
}

#[cfg(test)]
mod utf8_tests {
    use super::*;

    #[test]
    fn multibyte_split_across_reads_is_not_mangled() {
        let s = "a—b漢😀c";
        let bytes = s.as_bytes();
        // Every possible split point must reassemble to the original text.
        for cut in 0..=bytes.len() {
            let mut pending = Vec::new();
            let mut got = decode_utf8_chunk(&mut pending, &bytes[..cut]);
            got.push_str(&decode_utf8_chunk(&mut pending, &bytes[cut..]));
            assert_eq!(got, s, "split at {cut}");
            assert!(pending.is_empty());
        }
    }

    #[test]
    fn invalid_bytes_become_replacement_and_do_not_stall() {
        let mut pending = Vec::new();
        let got = decode_utf8_chunk(&mut pending, b"ok\xFFgo");
        assert_eq!(got, "ok\u{FFFD}go");
        assert!(pending.is_empty());
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
            ShimTarget::Interpreted {
                interpreter,
                script,
            } => {
                assert_eq!(interpreter, "node");
                assert!(script
                    .display()
                    .to_string()
                    .replace('/', "\\")
                    .ends_with(r"npm\node_modules\@anthropic-ai\claude-code\cli.js"));
            }
            _ => panic!("expected interpreted"),
        }
    }

    #[test]
    fn classic_if_else_decodes() {
        let t = parse_npm_cmd_shim(r"C:\npm\codex.cmd", CLASSIC).unwrap();
        match t {
            ShimTarget::Interpreted {
                interpreter,
                script,
            } => {
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// Fresh registry merge: an installer that wrote PATH seconds ago must
    /// resolve -- the frozen process PATH is what hid hermes from the
    /// install-and-relaunch ladder.
    #[test]
    fn windows_registry_path_is_live() {
        let p = windows_registry_path();
        assert!(!p.is_empty());
        let sys = std::env::var("SYSTEMROOT").unwrap_or_else(|_| r"C:\Windows".into());
        let needle = format!(r"{}\System32", sys).to_lowercase();
        assert!(
            p.split(';').map(|d| d.to_lowercase()).any(|d| d == needle),
            "registry PATH must resolve System32: {p}"
        );
    }
}

#[cfg(test)]
mod cpr_tests {
    use super::*;

    /// No terminal view is attached, so nothing answers the cursor query: the
    /// child must still run (it used to stay blocked on ConPTY's ESC[6n).
    #[cfg(windows)]
    #[test]
    fn headless_conpty_child_runs_without_a_terminal_answering_the_cursor_query() {
        let app = tauri::test::mock_app();
        let cwd = std::env::temp_dir().display().to_string();
        let env: HashMap<String, String> = HashMap::new();
        let h = spawn("pty-cpr", "echo hello-cpr", &cwd, &env, 80, 24, app.handle().clone(), "main")
            .unwrap();
        let start = std::time::Instant::now();
        while !h.tail().contains("hello-cpr") {
            assert!(
                start.elapsed() < std::time::Duration::from_secs(10),
                "child never ran; tail={:?}",
                h.tail()
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        // answered exactly once, and the query is scrubbed from the replay tail
        // so a terminal mounting later does not answer it a second time
        assert_eq!(h.cursor.state(), CursorState::Answered);
        assert!(!h.tail().contains("\u{1b}[6n"), "tail={:?}", h.tail());
    }

    /// A terminal that answers the query itself (or mounts via pty_redraw)
    /// keeps the fallback from ever firing: the child stays blocked on whoever
    /// is meant to answer, and the replay tail keeps the query for it.
    #[cfg(windows)]
    #[test]
    fn an_attached_terminal_stands_the_fallback_down() {
        let app = tauri::test::mock_app();
        let cwd = std::env::temp_dir().display().to_string();
        let env: HashMap<String, String> = HashMap::new();
        let spawn_one = |id: &str| {
            spawn(id, "echo hello-term", &cwd, &env, 80, 24, app.handle().clone(), "main").unwrap()
        };
        let wait_query = |h: &PtyHandle| {
            let start = std::time::Instant::now();
            while !h.tail().contains("\u{1b}[6n") {
                assert!(start.elapsed() < std::time::Duration::from_secs(10), "no query");
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        // mounted via pty_redraw: no answer from us, so the child stays blocked
        let a = spawn_one("pty-cpr-a");
        wait_query(&a);
        a.terminal_attached();
        std::thread::sleep(CPR_GRACE + std::time::Duration::from_millis(800));
        assert_eq!(a.cursor.state(), CursorState::Terminal);
        assert_eq!(a.tail(), "\u{1b}[6n");
        // an xterm-style answer in time: the child runs, and we never answer
        let b = spawn_one("pty-cpr-b");
        wait_query(&b);
        b.writer().write("\u{1b}[24;80R").unwrap();
        let start = std::time::Instant::now();
        while !b.tail().contains("hello-term") {
            assert!(start.elapsed() < std::time::Duration::from_secs(10), "tail={:?}", b.tail());
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        std::thread::sleep(CPR_GRACE + std::time::Duration::from_millis(500));
        assert_eq!(b.cursor.state(), CursorState::Terminal);
    }

    #[test]
    fn cursor_report_detection_and_single_claim() {
        assert!(contains_cursor_report("\u{1b}[1;1R"));
        assert!(contains_cursor_report("x\u{1b}[24;80Ry"));
        assert!(contains_cursor_report("\u{1b}[A\u{1b}[3;9R"));
        for no in ["", "\u{1b}[6n", "\u{1b}[R", "\u{1b}[1R", "\u{1b}[1;R", "24;80R", "\u{1b}[1;2S"] {
            assert!(!contains_cursor_report(no), "{no:?}");
        }
        let g = CursorGate::new();
        let mut scrubbed = 0;
        assert!(g.claim_answer(|| scrubbed += 1));
        assert!(!g.claim_answer(|| scrubbed += 1));
        assert_eq!((scrubbed, g.state()), (1, CursorState::Answered));
        // a terminal that shows up first wins for good
        let g = CursorGate::new();
        g.terminal_attached();
        assert!(!g.claim_answer(|| panic!("must not scrub")));
        assert_eq!(g.state(), CursorState::Terminal);
        // and never downgrades an answered gate
        let g = CursorGate::new();
        assert!(g.claim_answer(|| {}));
        g.terminal_attached();
        assert_eq!(g.state(), CursorState::Answered);
    }
}
