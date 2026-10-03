//! MemPalace-backed semantic memory — port of `src/main/memory.ts` +
//! `src/main/palaceReap.ts`.
//!
//! CLI-only: the harness keeps a single shared palace under harnessHome,
//! points every agent's `MEMPALACE_PALACE_PATH` at it, and mines each agent's
//! `memory.md` into its own wing so the whole team can recall by meaning via
//! `mempalace search` / `mempalace wake-up`. Degrades silently to no-op when
//! the `mempalace` CLI isn't installed — the markdown memory still works.
//!
//!   store   : mempalace mine <agentDir> --wing <id> --agent <id>
//!   recall  : mempalace search "<q>" --results N   /   mempalace wake-up
//!
//! State is a process singleton (upstream was a module-level `new MemoryManager`);
//! `instance(home)` re-creates it if the harness home ever moves.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::TimeZone;
use serde_json::{json, Value};

use crate::pty::{self, Resolved};

/// Non-memory files `mempalace mine` must not ingest (settings.json, cursor,
/// inbox/outbox JSON, a Codex worker's private CODEX_HOME). `mempalace mine`
/// honors .gitignore — kept in sync with upstream MINE_IGNORE_LINES.
const MINE_IGNORE_LINES: &[&str] = &["settings.json", "cursor.json", "inbox/", "outbox/", ".codex/"];

/// Re-mine changed memories every 10 min (upstream MINE_INTERVAL_MS).
const MINE_INTERVAL_MS: u64 = 600_000;
/// Ceiling for the quarantine backoff (upstream MINE_BACKOFF_MAX_MS).
const MINE_BACKOFF_MAX_MS: u64 = 1_800_000;
/// Hard cap per mine — first run may lazily download the embedding model.
const MINE_TIMEOUT_MS: u64 = 10 * 60_000;
/// Read commands (search / wake-up) — upstream's runCli ceiling.
const READ_TIMEOUT_MS: u64 = 120_000;

fn mempalace_device() -> Option<&'static str> {
    // A user's OWN device choice wins (upstream mempalaceDevice).
    if std::env::var("MEMPALACE_EMBEDDING_DEVICE").is_ok() {
        return None;
    }
    // Pin cpu on macOS — mempalace's "auto" picks CoreML on Apple Silicon and
    // the embeddinggemma ONNX graph produces NaN vectors there (verified
    // upstream 2026-08-16). Other platforms keep mempalace's own default.
    if cfg!(target_os = "macos") {
        Some("cpu")
    } else {
        None
    }
}

/// Idempotently ensure `<agentDir>/.gitignore` excludes the non-memory files.
/// Writes only the missing lines (append-only) so it's safe every cycle.
fn ensure_mine_ignore(agent_dir: &Path) {
    let path = agent_dir.join(".gitignore");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let have: std::collections::HashSet<&str> =
        existing.lines().map(|l| l.trim()).collect();
    let missing: Vec<&&str> = MINE_IGNORE_LINES
        .iter()
        .filter(|l| !have.contains(**l))
        .collect();
    if missing.is_empty() {
        return;
    }
    let prefix = if !existing.is_empty() && !existing.ends_with('\n') {
        format!("{existing}\n")
    } else {
        existing
    };
    let mut out = prefix;
    for l in missing {
        out.push_str(l);
        out.push('\n');
    }
    let _ = fs::write(&path, out); // best-effort
}

// ─── palaceReap.ts ──────────────────────────────────────────────────────────

/// The quarantine stamp `<uuid>.drift-|corrupt-YYYYMMDD-HHMMSS` as LOCAL epoch
/// ms (MemPalace writes datetime.now() on the same machine). None if the name
/// doesn't carry the exact suffix — live segments are bare UUIDs.
pub fn quarantine_stamp_ms(name: &str) -> Option<i64> {
    // Matches /\.(?:drift|corrupt)-(\d{4})(\d{2})(\d{2})-(\d{2})(\d{2})(\d{2})$/
    let tail = name.rsplit_once('.').map(|(_, t)| t)?;
    let stamp = tail
        .strip_prefix("drift-")
        .or_else(|| tail.strip_prefix("corrupt-"))?;
    let b = stamp.as_bytes();
    // YYYYMMDD-HHMMSS — exactly 8 digits, dash, 6 digits.
    if b.len() != 15 || b[8] != b'-' {
        return None;
    }
    let digits = |r: std::ops::Range<usize>| -> Option<i32> {
        let s = stamp.get(r)?;
        if s.bytes().all(|c| c.is_ascii_digit()) {
            s.parse().ok()
        } else {
            None
        }
    };
    let (y, mo, d, h, mi, s) = (
        digits(0..4)?,
        digits(4..6)?,
        digits(6..8)?,
        digits(9..11)?,
        digits(11..13)?,
        digits(13..15)?,
    );
    chrono::Local
        .with_ymd_and_hms(y, mo as u32, d as u32, h as u32, mi as u32, s as u32)
        .single()
        .map(|dt| dt.timestamp_millis())
}

/// Names to delete, newest-first by stamp, skipping the newest `keep` and any
/// younger than `min_age_ms` (the rename isn't atomic with a live rebuild).
pub fn quarantine_dirs_to_reap(names: &[String], now_ms: i64) -> Vec<String> {
    const KEEP: usize = 2;
    const MIN_AGE_MS: i64 = 10 * 60_000;
    let mut stamped: Vec<(String, i64)> = names
        .iter()
        .filter_map(|n| quarantine_stamp_ms(n).map(|ts| (n.clone(), ts)))
        .collect();
    // Newest first; name tie-break for two quarantines inside the same second.
    stamped.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
    stamped
        .into_iter()
        .skip(KEEP)
        .filter(|(_, ts)| now_ms - ts >= MIN_AGE_MS)
        .map(|(n, _)| n)
        .collect()
}

/// How long to wait before the next mine pass — doubles on a fresh quarantine,
/// snaps back to base on a clean one, capped at `max_ms`.
pub fn next_mine_delay_ms(current_ms: u64, base_ms: u64, max_ms: u64, quarantined: bool) -> u64 {
    if !quarantined {
        return base_ms;
    }
    (current_ms.max(base_ms) * 2).min(max_ms)
}

// ─── MemoryManager ──────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct MemorySettings {
    pub enabled: bool,
    /// 'minilm' | 'embeddinggemma'
    pub model: String,
}

/// `semanticMemory !== false` (default ON) + `embeddingModel ?? 'minilm'`, read
/// from the raw harness config.json (the Rust Config struct drops these keys —
/// a config save already discards them, same caveat as kg).
pub fn settings(harness_home: &Path) -> MemorySettings {
    let v = fs::read_to_string(harness_home.join("config.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or(Value::Null);
    let enabled = v
        .get("semanticMemory")
        .and_then(|b| b.as_bool())
        .unwrap_or(true);
    let model = match v.get("embeddingModel").and_then(|m| m.as_str()) {
        Some("embeddinggemma") => "embeddinggemma",
        _ => "minilm",
    }
    .to_string();
    MemorySettings { enabled, model }
}

struct MemInner {
    /// Tri-state: None = not yet resolved.
    bin_cache: Option<Option<PathBuf>>,
    /// True while a mine pass is in flight — serializes palace writers.
    mining: bool,
    /// agentId → memory.md mtime ms at last successful mine (skip unchanged).
    last_mined: HashMap<String, u64>,
    /// Newest quarantine stamp seen so far; a LATER one means it quarantined again.
    last_quarantine_ts: i64,
    /// Current gap between mine passes (widens while the palace quarantines).
    mine_delay_ms: u64,
    started: bool,
}

pub struct MemoryPalace {
    home: PathBuf,
    inner: Mutex<MemInner>,
    stop: AtomicBool,
}

static INSTANCE: Mutex<Option<Arc<MemoryPalace>>> = Mutex::new(None);

/// The manager singleton for `home`. A different home stops and replaces it
/// (upstream changeHome relaunches the app instead — same end state).
pub fn instance(home: &Path) -> Arc<MemoryPalace> {
    let home = home.to_path_buf();
    let mut g = INSTANCE.lock().unwrap();
    if g.as_ref().is_some_and(|m| m.home == home) {
        return g.as_ref().unwrap().clone();
    }
    if let Some(old) = g.take() {
        old.stop();
    }
    let m = Arc::new(MemoryPalace {
        home,
        inner: Mutex::new(MemInner {
            bin_cache: None,
            mining: false,
            last_mined: HashMap::new(),
            last_quarantine_ts: 0,
            mine_delay_ms: MINE_INTERVAL_MS,
            started: false,
        }),
        stop: AtomicBool::new(false),
    });
    *g = Some(m.clone());
    m
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl MemoryPalace {
    pub fn palace_path(&self) -> PathBuf {
        self.home.join("palace")
    }

    /// Resolve the mempalace CLI against the user's PATH + common uv/pip spots.
    /// `pty::resolve_program` mirrors the upstream PATH/`where` step.
    fn find_bin() -> Option<PathBuf> {
        let path_env = std::env::var("PATH").unwrap_or_default();
        if let Some(r) = pty::resolve_program("mempalace", &path_env) {
            let p = match r {
                Resolved::Exe(p) | Resolved::Script(p) => p,
            };
            if p.is_file() {
                return Some(p);
            }
        }
        // Probe common install locations (uv tool / homebrew / pip).
        let home_dir = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();
        let candidates: Vec<PathBuf> = if cfg!(windows) {
            vec![
                PathBuf::from(&home_dir).join(".local").join("bin").join("mempalace.exe"),
                PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default())
                    .join("Programs")
                    .join("Python")
                    .join("Scripts")
                    .join("mempalace.exe"),
            ]
        } else {
            vec![
                PathBuf::from(&home_dir).join(".local").join("bin").join("mempalace"),
                PathBuf::from("/opt/homebrew/bin/mempalace"),
                PathBuf::from("/usr/local/bin/mempalace"),
            ]
        };
        candidates.into_iter().find(|c| c.is_file())
    }

    /// Resolve + cache the CLI path.
    pub fn bin(&self) -> Option<PathBuf> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(cached) = &inner.bin_cache {
            return cached.clone();
        }
        let found = Self::find_bin();
        inner.bin_cache = Some(found.clone());
        found
    }

    /// Force re-resolution (e.g. after the user installs mempalace).
    pub fn reset_bin_cache(&self) {
        self.inner.lock().unwrap().bin_cache = None;
    }

    pub fn available(&self) -> bool {
        self.bin().is_some()
    }

    pub fn enabled(&self) -> bool {
        settings(&self.home).enabled
    }

    pub fn active(&self) -> bool {
        self.available() && self.enabled()
    }

    pub fn model(&self) -> String {
        settings(&self.home).model
    }

    /// memory.status() shape + the port's per-agent last-mine stamps.
    pub fn status(&self) -> Value {
        let palace = self.palace_path();
        let inner = self.inner.lock().unwrap();
        let last_mined: serde_json::Map<String, Value> = inner
            .last_mined
            .iter()
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect();
        let mine_delay = inner.mine_delay_ms;
        let mining = inner.mining;
        let loop_running = inner.started && !self.stop.load(Ordering::Relaxed);
        drop(inner);
        let active = self.active();
        json!({
            "available": self.available(),
            "enabled": self.enabled(),
            "active": active,
            "initialized": palace.exists(),
            "palacePath": palace.display().to_string(),
            "model": self.model(),
            "bin": self.bin().map(|p| p.display().to_string()),
            "lastMined": last_mined,
            "mineDelayMs": mine_delay,
            "mining": mining,
            "mineLoopRunning": loop_running,
        })
    }

    /// Env merged into each agent's spawn so its `mempalace` CLI hits the
    /// shared palace. Empty when inactive — a default install injects nothing.
    /// (Not yet wired into spawn_agent — same env the parent can merge.)
    pub fn env(&self) -> HashMap<String, String> {
        if !self.active() {
            return HashMap::new();
        }
        let mut env = HashMap::from([
            (
                "MEMPALACE_PALACE_PATH".to_string(),
                self.palace_path().display().to_string(),
            ),
            ("MEMPALACE_EMBEDDING_MODEL".to_string(), self.model()),
        ]);
        if let Some(d) = mempalace_device() {
            env.insert("MEMPALACE_EMBEDDING_DEVICE".into(), d.into());
        }
        env
    }

    fn child_env(&self) -> HashMap<String, String> {
        let mut env = self.env();
        env.entry("MEMPALACE_PALACE_PATH".into())
            .or_insert_with(|| self.palace_path().display().to_string());
        env.entry("MEMPALACE_EMBEDDING_MODEL".into())
            .or_insert_with(|| self.model());
        env
    }

    // — lifecycle —

    /// Arm the mine loop. `mempalace mine` auto-creates the palace on first run;
    /// `mempalace init` is deliberately NOT run (interactive "Mine now?" prompt
    /// that --yes doesn't cover — a spawned child would hang forever).
    pub fn start(self: &Arc<Self>) {
        if !self.active() {
            return;
        }
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.started {
                return;
            }
            inner.started = true;
        }
        self.reap_palace(); // boot sweep before the first mine
        let me = Arc::clone(self);
        std::thread::spawn(move || loop {
            if me.stop.load(Ordering::Relaxed) {
                return;
            }
            me.mine_pass();
            // Self-scheduling with a variable gap (quarantine backoff).
            let delay = me.inner.lock().unwrap().mine_delay_ms;
            let deadline = Instant::now() + Duration::from_millis(delay);
            while Instant::now() < deadline {
                if me.stop.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        });
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Re-resolve the CLI, arm the mine loop if it is only now possible, and
    /// report — the status poll is the one thing that reliably notices a
    /// post-boot `mempalace` install (upstream refresh()).
    pub fn refresh(self: &Arc<Self>) -> Value {
        self.reset_bin_cache();
        self.start();
        self.status()
    }

    // — mining (store) —

    /// Kick a mine pass on a background thread (upstream `hive:mineNow` is
    /// fire-and-forget). `agent` = Some(id) mines just that one dir.
    pub fn mine_now(self: &Arc<Self>, agent: Option<String>) {
        if !self.active() {
            return;
        }
        let me = Arc::clone(self);
        std::thread::spawn(move || match agent {
            Some(id) => me.mine_one(&id),
            None => me.mine_pass(),
        });
    }

    /// Mine every agent whose memory.md changed since last time, one at a
    /// time — the palace permits a single writer so mines are serialized via
    /// the `mining` guard.
    fn mine_pass(&self) {
        if !self.active() {
            return;
        }
        let agents_dir = self.home.join("hive").join("agents");
        let Ok(rd) = fs::read_dir(&agents_dir) else {
            return;
        };
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.mining {
                return; // a previous pass is still running — let it finish
            }
            inner.mining = true;
        }
        let dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        for agent_dir in dirs {
            let Some(id) = agent_dir
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
            else {
                continue;
            };
            let _ = self.mine_agent_if_changed(&agent_dir, &id);
        }
        self.inner.lock().unwrap().mining = false;
        // Whether this pass left another quarantined copy decides the next gap.
        let quarantined = self.reap_palace();
        let mut inner = self.inner.lock().unwrap();
        inner.mine_delay_ms = next_mine_delay_ms(
            inner.mine_delay_ms,
            MINE_INTERVAL_MS,
            MINE_BACKOFF_MAX_MS,
            quarantined,
        );
    }

    /// Explicit single-agent mine (hive:mineNow with an id — upstream only had
    /// the all-agents pass). Returns a per-call report, still non-blocking for
    /// the caller (this runs on the mine_now thread).
    fn mine_one(&self, id: &str) {
        let agent_dir = self.home.join("hive").join("agents").join(id);
        let mut inner = self.inner.lock().unwrap();
        if inner.mining {
            return;
        }
        inner.mining = true;
        drop(inner);
        self.mine_agent_if_changed(&agent_dir, id);
        self.inner.lock().unwrap().mining = false;
        let quarantined = self.reap_palace();
        let mut inner = self.inner.lock().unwrap();
        inner.mine_delay_ms = next_mine_delay_ms(
            inner.mine_delay_ms,
            MINE_INTERVAL_MS,
            MINE_BACKOFF_MAX_MS,
            quarantined,
        );
    }

    /// Skip-unchanged gate + one serialized `mempalace mine` for a single dir.
    fn mine_agent_if_changed(&self, agent_dir: &Path, id: &str) -> bool {
        let mem = agent_dir.join("memory.md");
        if !mem.exists() {
            return false;
        }
        let mtime = fs::metadata(&mem)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        {
            let inner = self.inner.lock().unwrap();
            if inner.last_mined.get(id) == Some(&mtime) {
                return false; // unchanged — skip the model load
            }
        }
        let ok = self.mine_agent(agent_dir, id);
        let mut inner = self.inner.lock().unwrap();
        if ok {
            inner.last_mined.insert(id.to_string(), mtime);
        } else {
            inner.last_mined.remove(id); // let the next pass retry
        }
        ok
    }

    /// `mempalace mine <agentDir> --wing <id> --agent <id>` — stdin closed
    /// (mempalace can prompt); re-mining is safe (mempalace dedups).
    fn mine_agent(&self, agent_dir: &Path, id: &str) -> bool {
        ensure_mine_ignore(agent_dir); // keep settings/cursor/messages out of the index
        let args = vec![
            "mine".to_string(),
            agent_dir.display().to_string(),
            "--wing".to_string(),
            id.to_string(),
            "--agent".to_string(),
            id.to_string(),
        ];
        let r = self.run_cli(&args, Duration::from_millis(MINE_TIMEOUT_MS));
        if !r.ok {
            eprintln!("[memory] mine {id} failed: {}", r.error.unwrap_or_default());
        }
        r.ok
    }

    /**
     * Delete quarantined segment copies MemPalace renamed aside and never
     * removed (palaceReap.ts port — the 100GB-in-8-hours rename loop). Returns
     * whether a FRESH quarantine was seen this pass (drives the mine backoff).
     * Best-effort — disk hygiene, never a correctness path.
     */
    fn reap_palace(&self) -> bool {
        let palace = self.palace_path();
        if !palace.exists() {
            return false;
        }
        let Ok(rd) = fs::read_dir(&palace) else {
            return false;
        };
        let names: Vec<String> = rd
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
            .collect();

        let mut newest: i64 = 0;
        for n in &names {
            if let Some(ts) = quarantine_stamp_ms(n) {
                newest = newest.max(ts);
            }
        }
        let mut inner = self.inner.lock().unwrap();
        let fresh = inner.last_quarantine_ts > 0 && newest > inner.last_quarantine_ts;
        if newest > inner.last_quarantine_ts {
            inner.last_quarantine_ts = newest;
        }
        drop(inner);

        let doomed = quarantine_dirs_to_reap(&names, now_ms());
        let mut removed = 0usize;
        for name in &doomed {
            if fs::remove_dir_all(palace.join(name)).is_ok() {
                removed += 1;
            }
        }
        if removed > 0 {
            eprintln!("[memory] reaped {removed} quarantined palace segment(s)");
        }
        fresh
    }

    // — recall (read) —

    /// Spawn the CLI with the shared-palace env, drain stdout+stderr, enforce
    /// the timeout by killing a wedged child.
    pub fn run_cli(&self, args: &[String], timeout: Duration) -> CliResult {
        let Some(bin) = self.bin() else {
            return CliResult {
                ok: false,
                output: String::new(),
                error: Some("semantic memory not active".into()),
            };
        };
        if !self.active() {
            return CliResult {
                ok: false,
                output: String::new(),
                error: Some("semantic memory not active".into()),
            };
        }
        let env = self.child_env();
        // .cmd/.bat shims need cmd /d /s /c (same shell-wrap as pty spawn).
        let mut cmd = if cfg!(windows)
            && bin
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "bat" | "cmd"))
        {
            let line = std::iter::once(bin.display().to_string())
                .chain(args.iter().cloned())
                .map(|a| pty::cmd_quote(&a))
                .collect::<Vec<_>>()
                .join(" ");
            let mut c = Command::new("cmd.exe");
            c.args(["/d", "/s", "/c", &line]);
            c
        } else {
            let mut c = Command::new(&bin);
            c.args(args);
            c
        };
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(env);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return CliResult {
                    ok: false,
                    output: String::new(),
                    error: Some(e.to_string()),
                }
            }
        };
        let mut out_pipe = child.stdout.take().unwrap();
        let mut err_pipe = child.stderr.take().unwrap();
        let out_t = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = std::io::Read::read_to_end(&mut out_pipe, &mut b);
            b
        });
        let err_t = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = std::io::Read::read_to_end(&mut err_pipe, &mut b);
            b
        });
        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) => {
                    if Instant::now() > deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    return CliResult {
                        ok: false,
                        output: String::new(),
                        error: Some(e.to_string()),
                    }
                }
            }
        };
        let out = String::from_utf8_lossy(&out_t.join().unwrap_or_default()).to_string();
        let err = String::from_utf8_lossy(&err_t.join().unwrap_or_default()).to_string();
        match status {
            None => CliResult {
                ok: false,
                output: out,
                error: Some("timed out".into()),
            },
            Some(s) if s.success() => CliResult {
                ok: true,
                output: out,
                error: None,
            },
            Some(s) => CliResult {
                ok: false,
                output: out,
                error: Some(if err.trim().is_empty() {
                    format!("exited {}", s.code().unwrap_or(-1))
                } else {
                    err.trim().to_string()
                }),
            },
        }
    }

    /// Public shell-out wrapper: `mempalace <args>` → JSON result. When the CLI
    /// is absent/inactive this still resolves to a result object, never Err.
    pub fn run_mempalace(&self, args: &[String]) -> Result<Value, String> {
        let r = self.run_cli(args, Duration::from_millis(READ_TIMEOUT_MS));
        Ok(cli_result_json(r))
    }

    /// Semantic search across the shared palace. Returns the CLI's text output.
    pub fn search(&self, query: &str, wing: Option<&str>, results: u32) -> Value {
        let mut args = vec![
            "search".to_string(),
            query.to_string(),
            "--results".to_string(),
            results.to_string(),
        ];
        if let Some(w) = wing.filter(|w| !w.is_empty()) {
            args.push("--wing".to_string());
            args.push(w.to_string());
        }
        cli_result_json(self.run_cli(&args, Duration::from_millis(READ_TIMEOUT_MS)))
    }

    /// Session-start digest (~600-900 tokens).
    pub fn wake_up(&self, wing: Option<&str>) -> Value {
        let mut args = vec!["wake-up".to_string()];
        if let Some(w) = wing.filter(|w| !w.is_empty()) {
            args.push("--wing".to_string());
            args.push(w.to_string());
        }
        cli_result_json(self.run_cli(&args, Duration::from_millis(READ_TIMEOUT_MS)))
    }
}

pub struct CliResult {
    pub ok: bool,
    pub output: String,
    pub error: Option<String>,
}

fn cli_result_json(r: CliResult) -> Value {
    match r.error {
        Some(e) => json!({ "ok": r.ok, "output": r.output, "error": e }),
        None => json!({ "ok": r.ok, "output": r.output }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarantine_stamp_parses_exact_suffix() {
        let ts = quarantine_stamp_ms("abc-uuid.drift-20260915-142330");
        assert!(ts.is_some());
        assert!(quarantine_stamp_ms("abc-uuid.corrupt-20260915-142330").is_some());
        // Not a quarantine suffix:
        assert!(quarantine_stamp_ms("plain-uuid").is_none());
        assert!(quarantine_stamp_ms("x.drift-2026").is_none());
        assert!(quarantine_stamp_ms("x.corrupt-20260915-14233").is_none());
        // must be anchored at end
        assert!(quarantine_stamp_ms("x.drift-20260915-142330.more").is_none());
    }

    #[test]
    fn reap_keeps_newest_and_respects_age() {
        let now = chrono::Local::now().timestamp_millis();
        let recent = {
            let t = chrono::Local::now() - chrono::Duration::minutes(1);
            format!("seg.drift-{}", t.format("%Y%m%d-%H%M%S"))
        };
        let mk = |mins_ago: i64, tag: &str| {
            let t = chrono::Local::now() - chrono::Duration::minutes(mins_ago);
            format!("seg-{tag}.corrupt-{}", t.format("%Y%m%d-%H%M%S"))
        };
        let names = vec![
            recent.clone(),
            mk(30, "a"),
            mk(40, "b"),
            mk(50, "c"),
        ];
        let doomed = quarantine_dirs_to_reap(&names, now);
        // newest 2 kept → only the oldest two are candidates (a is #3, kept
        // since keep=2? — sorted newest-first: recent, a, b, c → b,c doomed)
        assert_eq!(doomed.len(), 2);
        assert!(doomed.iter().all(|d| d != &recent));
    }

    #[test]
    fn mine_delay_doubles_then_snaps_back() {
        assert_eq!(next_mine_delay_ms(MINE_INTERVAL_MS, MINE_INTERVAL_MS, MINE_BACKOFF_MAX_MS, false), MINE_INTERVAL_MS);
        let doubled = next_mine_delay_ms(MINE_INTERVAL_MS, MINE_INTERVAL_MS, MINE_BACKOFF_MAX_MS, true);
        assert_eq!(doubled, 2 * MINE_INTERVAL_MS);
        let capped = next_mine_delay_ms(MINE_BACKOFF_MAX_MS, MINE_INTERVAL_MS, MINE_BACKOFF_MAX_MS, true);
        assert_eq!(capped, MINE_BACKOFF_MAX_MS);
    }

    #[test]
    fn settings_default_enabled_minilm() {
        let dir = std::env::temp_dir().join(format!("memcfg-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let s = settings(&dir);
        assert!(s.enabled);
        assert_eq!(s.model, "minilm");
        fs::write(dir.join("config.json"), r#"{"semanticMemory":false,"embeddingModel":"embeddinggemma"}"#).unwrap();
        let s = settings(&dir);
        assert!(!s.enabled);
        assert_eq!(s.model, "embeddinggemma");
        let _ = fs::remove_dir_all(&dir);
    }
}
