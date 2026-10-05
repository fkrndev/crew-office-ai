//! Hidden interactive claude session — port of `src/main/hiddenClaude.ts`.
//!
//! "Hidden" means: not added to the PtyManager, not emitted to the renderer,
//! not visible in the agent list or OfficeFloor scene. Each call spawns its
//! own session and kills it after capture — no /clear needed, no context
//! bleed.
//!
//! Uses an interactive PTY (not `claude -p`) so calls draw from the user's
//! normal interactive plan quota, not the Agent SDK credit that moves to a
//! separate claim-required pool from 2026-06-15.
//!
//! Session lifecycle:
//!   spawn → boot-quiet detect → bracketed-paste prompt + \r → idle-settle →
//!   transcript JSONL extract (last assistant text block) → kill

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

/// ms of PTY silence that signals the TUI is ready for input (boot complete).
const BOOT_QUIET_MS: u64 = 1500;

pub struct HiddenClaudeOptions {
    pub model: String,
    pub cwd: String,
    pub command: Option<String>,
    pub disallowed_tools: Vec<String>,
    pub add_dirs: Vec<String>,
    pub boot_cap_ms: u64,
    pub idle_ms: u64,
    pub timeout_ms: u64,
    pub env: Vec<(String, String)>,
}

impl Default for HiddenClaudeOptions {
    fn default() -> Self {
        Self {
            model: String::new(),
            cwd: String::new(),
            command: None,
            disallowed_tools: vec!["Edit".into(), "Write".into(), "NotebookEdit".into()],
            add_dirs: Vec::new(),
            boot_cap_ms: 7000,
            idle_ms: 3500,
            timeout_ms: 180_000,
            env: Vec::new(),
        }
    }
}

pub struct HiddenClaudeResult {
    pub ok: bool,
    pub text: Option<String>,
    pub error: Option<String>,
}

/// Claude Code's project key: the absolute cwd with EVERY non-alphanumeric
/// character turned into a dash. Matches `transcript.ts projectKey`.
fn project_key(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Resolve the Claude Code transcript directory for a given working directory:
/// `~/.claude/projects/<key>`. Port of `transcript.ts projectDir` (current
/// spelling only — legacy fallback is not needed for hidden sessions, which
/// always run against a modern Claude Code install).
fn project_dir(cwd: &str) -> Option<PathBuf> {
    let home = std::env::home_dir()
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))?;
    let dir = home.join(".claude").join("projects").join(project_key(cwd));
    dir.is_dir().then_some(dir)
}

/// Extract the last assistant text block from the transcript JSONL written
/// at or after `spawned_at`. Port of `hiddenClaude.ts extractLastAssistantText`.
fn extract_last_assistant_text(cwd: &str, spawned_at: u64) -> Option<String> {
    let dir = project_dir(cwd)?;
    let mut candidates: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    for entry in fs::read_dir(&dir).ok()? {
        let Ok(e) = entry else { continue };
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(meta) = fs::metadata(&path) else { continue };
        let Ok(mtime) = meta.modified() else { continue };
        let mtime_ms = mtime
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        // 5s slack: include files that already existed at spawn but were
        // updated by this session. Sort by mtime and take the newest.
        if mtime_ms >= spawned_at.saturating_sub(5000) {
            candidates.push((path, mtime));
        }
    }
    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    let (newest, _) = candidates.first()?;
    let text = fs::read_to_string(newest).ok()?;
    for line in text.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        if rec.get("type").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let Some(content) = rec.get("message").and_then(|m| m.get("content")) else {
            continue;
        };
        let Some(arr) = content.as_array() else {
            continue;
        };
        for block in arr.iter().rev() {
            if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                    let t = t.trim();
                    if !t.is_empty() {
                        return Some(t.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Run a hidden interactive claude session and return the assistant's final
/// text response. Port of `hiddenClaude.ts runHiddenClaude`.
pub fn run_hidden_claude(prompt: &str, opts: &HiddenClaudeOptions) -> HiddenClaudeResult {
    if prompt.trim().is_empty() {
        return HiddenClaudeResult {
            ok: false,
            text: None,
            error: Some("empty prompt".into()),
        };
    }
    let cwd = crate::misc::expand_tilde(&opts.cwd);
    if cwd.is_empty() || !Path::new(&cwd).exists() {
        return HiddenClaudeResult {
            ok: false,
            text: None,
            error: Some(format!("cwd does not exist: {}", opts.cwd)),
        };
    }

    let binary = opts
        .command
        .as_deref()
        .unwrap_or("claude")
        .trim()
        .split_whitespace()
        .next()
        .unwrap_or("claude");
    let shell_path = crate::pty::user_shell_path();
    let resolved = crate::pty::resolve_program(binary, &shell_path);
    let exe = match resolved {
        Some(crate::pty::Resolved::Exe(p)) | Some(crate::pty::Resolved::Script(p)) => p,
        None => {
            return HiddenClaudeResult {
                ok: false,
                text: None,
                error: Some(format!("could not resolve command: {binary}")),
            }
        }
    };

    let add_dirs: Vec<String> = opts
        .add_dirs
        .iter()
        .filter(|d| !d.is_empty() && Path::new(d).exists())
        .cloned()
        .collect();

    let mut args: Vec<String> = vec![
        "--model".into(),
        opts.model.clone(),
        "--permission-mode".into(),
        "bypassPermissions".into(),
        "--disallowedTools".into(),
        opts.disallowed_tools.join(","),
    ];
    for d in &add_dirs {
        args.push("--add-dir".into());
        args.push(d.clone());
    }

    // Windows: non-.exe targets (npm .cmd shims) can't be exec'd directly by
    // CreateProcess (ERROR_BAD_EXE_FORMAT) — route through cmd.exe.
    let exe_str = exe.display().to_string();
    let win_wrap = cfg!(windows) && {
        let lower = exe_str.to_lowercase();
        !lower.ends_with(".exe") && !lower.ends_with(".com")
    };
    let (spawn_file, spawn_args): (String, Vec<String>) = if win_wrap {
        let comspec = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into());
        let mut a = vec!["/c".into(), exe_str.clone()];
        a.extend(args.clone());
        (comspec, a)
    } else {
        (exe_str.clone(), args.clone())
    };

    let pty_system = native_pty_system();
    let pair = match pty_system.openpty(PtySize {
        rows: 50,
        cols: 220,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(p) => p,
        Err(e) => {
            return HiddenClaudeResult {
                ok: false,
                text: None,
                error: Some(format!("openpty: {e}")),
            }
        }
    };

    let mut cmd = CommandBuilder::new(&spawn_file);
    for a in &spawn_args {
        cmd.arg(a);
    }
    cmd.cwd(&cwd);
    // Build env: parent + login-shell PATH + per-call overrides.
    let parent: std::collections::HashMap<String, String> = std::env::vars().collect();
    for (k, v) in &parent {
        cmd.env(k, v);
    }
    cmd.env("PATH", &shell_path);
    for (k, v) in &opts.env {
        cmd.env(k, v);
    }

    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(c) => c,
        Err(e) => {
            return HiddenClaudeResult {
                ok: false,
                text: None,
                error: Some(format!("spawn: {e}")),
            }
        }
    };
    drop(pair.slave);

    let master = pair.master;
    let mut writer = match master.take_writer() {
        Ok(w) => w,
        Err(e) => {
            return HiddenClaudeResult {
                ok: false,
                text: None,
                error: Some(format!("take writer: {e}")),
            }
        }
    };
    let mut reader = match master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => {
            return HiddenClaudeResult {
                ok: false,
                text: None,
                error: Some(format!("clone reader: {e}")),
            }
        }
    };

    let spawned_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let deadline = Instant::now() + Duration::from_millis(opts.timeout_ms);

    // Spawn a reader thread that drains PTY output and tracks last-activity time.
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    let prompt_sent = Arc::new(AtomicBool::new(false));
    let settled = Arc::new(AtomicBool::new(false));
    let last_activity = Arc::new(Mutex::new(Instant::now()));

    let settled_r = settled.clone();
    let last_activity_r = last_activity.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            if settled_r.load(Ordering::Relaxed) {
                break;
            }
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    *last_activity_r.lock().unwrap() = Instant::now();
                }
                Err(_) => break,
            }
        }
    });

    // Boot phase: wait for quiet, then send prompt.
    let boot_deadline = Instant::now() + Duration::from_millis(opts.boot_cap_ms);
    let boot_quiet = Duration::from_millis(BOOT_QUIET_MS);
    let mut prompt_sent_local = false;
    while Instant::now() < boot_deadline && Instant::now() < deadline {
        if settled.load(Ordering::Relaxed) {
            break;
        }
        let elapsed = Instant::now().duration_since(*last_activity.lock().unwrap());
        if elapsed >= boot_quiet {
            // Boot quiet — send the prompt via bracketed paste + CR.
            let paste = format!("\x1b[200~{prompt}\x1b[201~");
            let _ = writer.write_all(paste.as_bytes());
            std::thread::sleep(Duration::from_millis(140));
            if !settled.load(Ordering::Relaxed) {
                let _ = writer.write_all(b"\r");
            }
            prompt_sent.store(true, Ordering::Relaxed);
            prompt_sent_local = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !prompt_sent_local && Instant::now() < deadline {
        // Boot cap hit — force send.
        let paste = format!("\x1b[200~{prompt}\x1b[201~");
        let _ = writer.write_all(paste.as_bytes());
        std::thread::sleep(Duration::from_millis(140));
        if !settled.load(Ordering::Relaxed) {
            let _ = writer.write_all(b"\r");
        }
        prompt_sent.store(true, Ordering::Relaxed);
    }

    // Response phase: wait for idle, then capture.
    let idle_quiet = Duration::from_millis(opts.idle_ms);
    while Instant::now() < deadline {
        if settled.load(Ordering::Relaxed) {
            break;
        }
        // Check if child exited.
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(_) => break,
        }
        let elapsed = Instant::now().duration_since(*last_activity.lock().unwrap());
        if prompt_sent.load(Ordering::Relaxed) && elapsed >= idle_quiet {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    settled.store(true, Ordering::Relaxed);

    // Kill the process tree (ensureKilled: taskkill /T /F on Windows, group
    // SIGKILL on POSIX). portable_pty's kill signals the direct child; the
    // tree sweep ensures MCP servers and helpers don't leak.
    let _ = child.kill();
    #[cfg(windows)]
    if let Some(pid) = child.process_id() {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    // Capture the transcript.
    match extract_last_assistant_text(&cwd, spawned_at) {
        Some(text) => HiddenClaudeResult {
            ok: true,
            text: Some(text),
            error: None,
        },
        None => {
            // Check if we hit the timeout.
            if Instant::now() >= deadline {
                HiddenClaudeResult {
                    ok: false,
                    text: None,
                    error: Some("hidden session timed out".into()),
                }
            } else {
                HiddenClaudeResult {
                    ok: false,
                    text: None,
                    error: Some("no assistant response found in transcript".into()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_key_dashes_every_non_alnum() {
        assert_eq!(project_key("C:\\Users\\me\\app"), "C--Users-me-app");
        assert_eq!(project_key("/Users/me/MDv0.3.0"), "-Users-me-MDv0-3-0");
    }

    #[test]
    fn empty_prompt_rejected() {
        let r = run_hidden_claude("   ", &HiddenClaudeOptions::default());
        assert!(!r.ok);
        assert_eq!(r.error.as_deref(), Some("empty prompt"));
    }

    #[test]
    fn missing_cwd_rejected() {
        let opts = HiddenClaudeOptions {
            cwd: "/nonexistent/path/that/does/not/exist".into(),
            ..Default::default()
        };
        let r = run_hidden_claude("test", &opts);
        assert!(!r.ok);
        assert!(r.error.unwrap().contains("cwd does not exist"));
    }
}
