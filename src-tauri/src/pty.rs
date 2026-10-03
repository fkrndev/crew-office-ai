//! PTY manager — port of `src/main/pty.ts` (node-pty) onto `portable-pty`.
//!
//! Each agent CLI runs as a real process in a pseudo-terminal. Output is read
//! on a dedicated thread per PTY and forwarded to the frontend as
//! `pty-output` events; input flows back via `pty_write`. Semantics mirror the
//! Electron version byte-for-byte where the crates allow it.

use std::collections::HashMap;
use std::io::{Read, Write};

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

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

    let mut reader = pair.master.try_clone_reader().context("clone reader")?;
    let writer = pair.master.take_writer().context("take writer")?;

    // Reader thread → frontend event stream. Lossy UTF-8 matches node-pty's
    // string output; xterm.js tolerates the occasional split sequence.
    let event_id = id.to_string();
    std::thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let data = String::from_utf8_lossy(&buf[..n]).to_string();
                    let _ = app.emit(
                        "pty-output",
                        PtyOutputPayload {
                            id: event_id.clone(),
                            data,
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
        master: pair.master,
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
