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
use tauri::{AppHandle, Emitter};

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
    let exts: Vec<&str> = if cfg!(windows) {
        vec![".com", ".exe", ".bat", ".cmd"]
    } else {
        vec![""]
    };
    for dir in path_env.split(if cfg!(windows) { ';' } else { ':' }) {
        for ext in &exts {
            let cand = PathBuf::from(dir).join(format!("{prog}{ext}"));
            if cand.is_file() {
                return Some(classify(cand));
            }
        }
    }
    None
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

/// Quote one argument for a `cmd /d /s /c` line: wrap in quotes when it has
/// spaces/quotes, doubling inner quotes (cmd's escaping).
pub fn cmd_quote(arg: &str) -> String {
    if arg.contains(' ') || arg.contains('"') || arg.is_empty() {
        format!("\"{}\"", arg.replace('"', "\"\""))
    } else {
        arg.to_string()
    }
}

/// Make a value safe inside a double-quoted cmd arg: single line, no double
/// quotes (an --append-system-prompt payload travels this way on script spawns).
pub fn cmd_sanitize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").replace('"', "'")
}

pub struct PtyHandle {
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
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
    child: Box<dyn Child + Send + Sync>,
    app: AppHandle,
) -> Result<PtyHandle> {
    let mut reader = master.try_clone_reader().context("clone reader")?;
    let writer = master.take_writer().context("take writer")?;
    let event_id = id.to_string();
    std::thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let _ = app.emit(
                        "pty-output",
                        PtyOutputPayload {
                            id: event_id.clone(),
                            data: String::from_utf8_lossy(&buf[..n]).to_string(),
                        },
                    );
                }
                Err(_) => break,
            }
        }
        let _ = app.emit(
            "pty-exit",
            PtyExitPayload {
                id: event_id,
                code: None,
            },
        );
    });
    Ok(PtyHandle {
        writer,
        child,
        master,
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

    pub fn kill(&mut self) -> Result<()> {
        self.child.kill()?;
        let _ = self.child.wait(); // reap
        Ok(())
    }
}
