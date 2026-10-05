//! Ephemeral-worker watcher — port of index.ts's spawn-request loop
//! (WORKER_TICK_MS ~1.5s polling, no fs.watch).
//!
//! god drops a spawn-request JSON into `hive/spawn-requests/<id>.json`; this
//! module spins up a FRESH worker through the normal spawn path, dispatches
//! its objective via the standard inbox route, then watches each live worker
//! for a terminal `act:"done"` (success → release + settle its inbox) or
//! excessive idleness / token burn (reap). Terminal failures inform god WITH
//! the Slack coords so the request's thread hears about it.
//!
//! Intake is gated on `config.orchestrator_may_spawn` (default OFF — a spend
//! decision the operator opts into). The gate sits on INTAKE only: reaping
//! and teardown of running workers continue when the toggle is off.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::commands::{state, AppState, SpawnArgs};

/// One live god-triggered ephemeral worker (upstream WorkerRec).
pub struct WorkerRec {
    pub req_id: String,
    pub name: String,
    /// Slack thread coords the request asked for — failures are reported back
    /// here via god so the loop closes even when the worker never ran.
    pub slack: Option<(String, String)>,
    pub base_branch: String,
    /// ms-epoch spawn stamp — the stale-done guard compares `done` signals
    /// against it (a reused reqId must not release on the PRIOR worker's done).
    pub spawned_at: u64,
    /// Positive = enforce; None/absent = unlimited.
    pub token_cap: Option<u64>,
    /// One-shot guard: a release/reap decision already taken.
    pub releasing: bool,
}

/// liveWorkers — populated by the watcher; consulted by teardown paths.
pub type LiveWorkers = Mutex<HashMap<String, WorkerRec>>;

/// A worker worktree that teardown PRESERVED because it held unintegrated
/// work (upstream PreservedWorktree). The GC sweep reclaims it (+ the
/// worker's hive scratch dir) once its work lands in base — or when the
/// worktree is already gone from disk (clean teardown still registers here
/// so scratch removal is deferred past the memory miner).
pub struct PreservedWorktree {
    pub worker_id: String,
    pub wt_path: PathBuf,
    pub orig_cwd: PathBuf,
    pub base_branch: String,
    pub scratch_dir: Option<PathBuf>,
    pub slack: Option<(String, String)>,
    pub preserved_at: u64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// HIVE_ROOT/spawn-requests — the queue dir god drops requests into.
fn spawn_requests_dir(hive: &crate::hive::Hive) -> PathBuf {
    hive.root.join("spawn-requests")
}

/// Move a processed request out of the queue so it's never reprocessed.
fn archive_request(path: &Path, sub: &str) {
    let dir = path.parent().unwrap_or(Path::new(".")).join(sub);
    let _ = fs::create_dir_all(&dir);
    let dest = dir.join(path.file_name().unwrap_or_default());
    if fs::rename(path, &dest).is_err() {
        let _ = fs::remove_file(path); // poison file must not loop forever
    }
}

/// Provider id → {bin, model flag, autoMode flag}. A mirror of the renderer's
/// AGENT_PROVIDER_PRESETS — the watcher has no renderer to build commands.
fn preset_command(provider: &str, model: Option<&str>, auto_mode: bool) -> String {
    let (bin, auto_flag) = match provider {
        "codex" => ("codex", "-a never -s workspace-write"),
        "grok" => ("grok", "--permission-mode bypassPermissions"),
        "kimi" => ("kimi", "--auto"),
        "gemini" => ("gemini", "--approval-mode=yolo"),
        "antigravity" => ("agy", "--dangerously-skip-permissions"),
        "qwen" => ("qwen", "--yolo"),
        "opencode" => ("opencode", ""),
        "crush" => ("crush", "--yolo"),
        "pi" => ("pi", "--approve"),
        "copilot" => ("copilot", "-s --allow-all-tools --no-ask-user"),
        "cursor" => ("cursor-agent", "--force --trust"),
        "hermes" => ("hermes --cli", "--yolo"),
        "openclaw" => ("openclaw tui --local", ""),
        _ => ("claude", "--permission-mode bypassPermissions"),
    };
    let mut cmd = bin.to_string();
    if auto_mode && !auto_flag.is_empty() {
        cmd.push_str(&format!(" {auto_flag}"));
    }
    if let Some(m) = model.filter(|m| !m.trim().is_empty()) {
        cmd.push_str(&format!(" --model {}", m.trim()));
    }
    cmd
}

/// workerDoneAt — the newest `act:"done"` in the worker's outbox/.sent
/// authored AFTER spawnedAt (the stale-done guard: a reused reqId leaves the
/// prior worker's done in the same dir; counting it would release the new
/// worker on its first tick). mtime is the fallback timestamp; no usable
/// timestamp → not done (the idle reaper is the backstop).
fn worker_done_at(hive: &crate::hive::Hive, worker_id: &str, spawned_at: u64) -> Option<u64> {
    let base = hive.agent_dir(worker_id).join("outbox");
    let mut done_at: Option<u64> = None;
    for dir in [base.clone(), base.join(".sent")] {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Ok(t) = fs::read_to_string(&p) else {
                continue;
            };
            let Ok(m) = serde_json::from_str::<Value>(&t) else {
                continue;
            };
            if m["act"].as_str() != Some("done") {
                continue;
            }
            let ts = m["created_at"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.timestamp_millis() as u64)
                .or_else(|| {
                    fs::metadata(&p)
                        .ok()
                        .and_then(|md| md.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as u64)
                });
            if let Some(ts) = ts {
                if ts > spawned_at && done_at.map(|d| ts > d).unwrap_or(true) {
                    done_at = Some(ts);
                }
            }
        }
    }
    done_at
}

/// informGod — an inform to the god agent (the human's proxy), with the
/// request's Slack coords appended so god can close the loop in-thread.
fn inform_god(
    hive: &crate::hive::Hive,
    subject: &str,
    body: &str,
    slack: Option<&(String, String)>,
) {
    let body = match slack {
        Some((ch, ts)) => format!("{body}\n(slack: channel {ch}, thread {ts})"),
        None => body.to_string(),
    };
    let _ = hive.send_virtual("ephemeral-worker", "god", "inform", subject, &body);
}

/// Total tokens (input+output+cache) a worker has burned (upstream
/// workerTokensUsed) — live OTel → proxy ledger → transcript, same order.
fn worker_tokens_used(app: &AppHandle, worker_id: &str) -> u64 {
    let st = state(app);
    let entry = st.hive.registry().into_iter().find(|a| a.id == worker_id);
    st.telemetry
        .sample(worker_id)
        .or_else(|| st.hive.usage_sample(worker_id))
        .or_else(|| entry.and_then(|a| st.usage.sample(&a.cwd, a.session_id.as_deref())))
        .map(|s| s.input + s.output + s.cache_read + s.cache_creation)
        .unwrap_or(0)
}

/// removeWorkerScratch — delete ONLY a dead worker's hive agent dir, never a
/// live one's (path-safety: resolved dir must sit directly under agents/).
fn remove_worker_scratch(st: &AppState, worker_id: &str) {
    if st.live_workers.lock().unwrap().contains_key(worker_id) {
        return;
    }
    let dir = st.hive.agent_dir(worker_id);
    let agents_root = dir.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    if dir.parent() != Some(agents_root.as_path())
        || dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .as_deref()
            != Some(worker_id)
    {
        return;
    }
    let _ = fs::remove_dir_all(&dir);
}

/// finalizeWorkerWorktree (index.ts:528) — remove a dead worker's worktree
/// ONLY when it holds no unintegrated work; otherwise PRESERVE it and ping
/// god, the sole integrator. Either way the hive scratch dir is registered
/// for the deferred GC sweep (memory miner races an immediate delete).
fn finalize_worker_worktree(st: &AppState, wt_path: &Path, orig_cwd: &Path, rec: &WorkerRec) {
    if let Err(e) = crate::git_ops::unlink_worktree_deps(orig_cwd, wt_path) {
        log::warn!("[worktree] dependency unlink failed: {e}");
    }
    let keep = crate::git_ops::worktree_unintegrated_details(wt_path, &rec.base_branch);
    let register = |st: &AppState| {
        st.preserved_worktrees.lock().unwrap().insert(
            wt_path.to_path_buf(),
            PreservedWorktree {
                worker_id: format!("worker-{}", rec.req_id),
                wt_path: wt_path.to_path_buf(),
                orig_cwd: orig_cwd.to_path_buf(),
                base_branch: rec.base_branch.clone(),
                scratch_dir: Some(st.hive.agent_dir(&format!("worker-{}", rec.req_id))),
                slack: rec.slack.clone(),
                preserved_at: now_ms(),
            },
        );
    };
    match keep {
        Some((true, detail, branch)) => {
            log::warn!(
                "[worker] PRESERVING worktree with unintegrated work: {}",
                wt_path.display()
            );
            register(st);
            inform_god(
                &st.hive,
                &format!("[worker worktree preserved] worker-{}", rec.req_id),
                &format!(
                    "Ephemeral worker worker-{} ended but its worktree holds unintegrated work, so it was NOT auto-removed (you are the sole integrator).\nWorktree: {}\nBranch: {branch}\nState: {detail}\nReview/merge it — it will be auto-reclaimed once its work lands in {}, or remove it now with: git -C \"{}\" worktree remove \"{}\"",
                    rec.req_id,
                    wt_path.display(),
                    rec.base_branch,
                    orig_cwd.display(),
                    wt_path.display(),
                ),
                rec.slack.as_ref(),
            );
        }
        _ => {
            // Clean/integrated (or undeterminable-as-clean after unlink):
            // remove the worktree but still register the scratch for the
            // deferred GC sweep — upstream does the same so the memory miner
            // can ingest memory.md before the dir is reclaimed.
            if keep.is_none() {
                log::warn!(
                    "[worker] unintegrated check failed; keeping worktree {}",
                    wt_path.display()
                );
                register(st);
                return;
            }
            if let Err(e) = crate::git_ops::remove_worktree(wt_path) {
                log::warn!("[worker] removeWorktree failed: {e}");
            }
            register(st);
        }
    }
}

/// teardownPty step 2 — the worktree half of kill_agent: workers get the
/// safety-gated finalize; regular isolated agents keep upstream's immediate
/// force-remove. Called for EVERY teardown route (kill, done-release, reap,
/// crash) so a worktree can never orphan silently.
pub fn finalize_teardown(st: &AppState, id: &str) {
    let worker = st.live_workers.lock().unwrap().remove(id);
    if let Some((wt_path, orig_cwd)) = st.worktree_paths.lock().unwrap().remove(id) {
        match worker {
            Some(rec) => finalize_worker_worktree(st, &wt_path, &orig_cwd, &rec),
            None => {
                if let Err(e) = crate::git_ops::remove_worktree(&wt_path) {
                    log::warn!("[worktree] removeWorktree failed: {e}");
                }
            }
        }
    }
}

/// gcPreservedWorktrees — throttled sweep (GC_SWEEP_MS): reclaim a preserved
/// worktree ONLY when its work is provably integrated (or the tree is gone).
/// Any doubt KEEPS it — god is the sole integrator.
pub fn gc_preserved_worktrees(st: &AppState) {
    let keys: Vec<PathBuf> = st
        .preserved_worktrees
        .lock()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    for key in keys {
        let e = {
            let p = st.preserved_worktrees.lock().unwrap();
            match p.get(&key) {
                Some(e) => PreservedWorktree {
                    worker_id: e.worker_id.clone(),
                    wt_path: e.wt_path.clone(),
                    orig_cwd: e.orig_cwd.clone(),
                    base_branch: e.base_branch.clone(),
                    scratch_dir: e.scratch_dir.clone(),
                    slack: e.slack.clone(),
                    preserved_at: e.preserved_at,
                },
                None => continue,
            }
        };
        // A worker id that is live again (reqId reuse) → never GC out from
        // under the new run.
        if st.live_workers.lock().unwrap().contains_key(&e.worker_id) {
            continue;
        }
        // (a) Worktree already gone (clean teardown, or removed by hand) →
        //     reclaim the scratch dir + drop tracking.
        if !e.wt_path.exists() {
            remove_worker_scratch(st, &e.worker_id);
            st.preserved_worktrees.lock().unwrap().remove(&key);
            continue;
        }
        // (b) Still on disk → reclaim ONLY when provably integrated + clean.
        if let Err(err) = crate::git_ops::unlink_worktree_deps(&e.orig_cwd, &e.wt_path) {
            log::warn!("[worker gc] dependency unlink failed (keeping): {err}");
            continue;
        }
        match crate::git_ops::worktree_integrated_against(&e.wt_path, &e.base_branch) {
            Some(true) => {}
            _ => continue, // keep — fail-safe
        }
        if let Err(err) = crate::git_ops::remove_worktree(&e.wt_path) {
            log::warn!(
                "[worker gc] removeWorktree failed (keeping {}): {err}",
                e.worker_id
            );
            continue;
        }
        remove_worker_scratch(st, &e.worker_id);
        st.preserved_worktrees.lock().unwrap().remove(&key);
        inform_god(
            &st.hive,
            &format!("[worker worktree reclaimed] {}", e.worker_id),
            &format!(
                "The preserved worktree for {} is now integrated, so it and its scratch dir were garbage-collected.\nWorktree: {}",
                e.worker_id,
                e.wt_path.display(),
            ),
            e.slack.as_ref(),
        );
    }
}

/// processSpawnRequest — validate, spawn, register, dispatch. Terminal
/// failures archive to .failed and inform god WITH the Slack coords.
pub fn process_spawn_request(app: &AppHandle, path: &Path) {
    let st = state(app);
    let raw: Value = match fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
    {
        Some(v) => v,
        None => {
            log::warn!("[worker] unparseable spawn-request: {}", path.display());
            inform_god(
                &st.hive,
                "[worker spawn rejected] unparseable request",
                &format!(
                    "Could not parse spawn-request {}.",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
                None,
            );
            archive_request(path, ".failed");
            return;
        }
    };
    let slack = match (
        raw.get("slack").and_then(|s| s["channel"].as_str()),
        raw.get("slack").and_then(|s| s["thread_ts"].as_str()),
    ) {
        (Some(c), Some(t)) => Some((c.to_string(), t.to_string())),
        _ => None,
    };
    let fname = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    macro_rules! fail {
        ($reason:expr) => {{
            inform_god(
                &st.hive,
                &format!("[worker spawn rejected] {}", $reason),
                &format!("Spawn-request {fname} rejected: {}.", $reason),
                slack.as_ref(),
            );
            archive_request(path, ".failed");
            return;
        }};
    }

    let objective = raw["objective"].as_str().unwrap_or("").trim().to_string();
    if objective.is_empty() {
        fail!("missing \"objective\"");
    }

    let req_id = raw["id"]
        .as_str()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| fname.trim_end_matches(".json").to_string());
    let req_id: String = req_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let worker_id = format!("worker-{req_id}");
    if st.live_workers.lock().unwrap().contains_key(&worker_id) {
        fail!(format!("worker \"{worker_id}\" already running"));
    }

    let cwd = raw["cwd"]
        .as_str()
        .map(|s| crate::misc::expand_tilde(s.trim()))
        .unwrap_or_default();
    if cwd.is_empty() || !Path::new(&cwd).is_dir() {
        fail!(format!(
            "\"cwd\" missing or not found ({})",
            if cwd.is_empty() {
                "unset".to_string()
            } else {
                cwd.clone()
            }
        ));
    }

    let provider = raw["provider"].as_str().unwrap_or("claude");
    let auto = st.config.lock().unwrap().auto_mode;
    let command = match raw["command"]
        .as_str()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        Some(c) => c.to_string(),
        None => preset_command(provider, raw["model"].as_str(), auto),
    };
    let bin = crate::misc::tokenize_command(&command)
        .into_iter()
        .next()
        .unwrap_or_default();
    // A spawn-request file is untrusted input — the bin must be a plain
    // command token or an absolute path, never a shell-reinterpretable string.
    if !crate::pty::is_safe_command_name(&bin) && !Path::new(&bin).is_absolute() {
        fail!(format!(
            "refusing spawn: engine command \"{bin}\" is not a plain command name or an absolute path"
        ));
    }
    // Missing-CLI → fail fast; a headless worker has no human to install it.
    let path_env = std::env::var("PATH").unwrap_or_default();
    if crate::pty::resolve_program(&bin, &path_env).is_none() {
        fail!(format!("engine CLI \"{bin}\" is not installed"));
    }

    let base_branch = crate::git_ops::branch(Path::new(&cwd))
        .and_then(|b| b["current"].as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "main".into());

    let name = raw["name"]
        .as_str()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("Worker {}", &req_id[..req_id.len().min(12)]));

    let spawned = crate::commands::spawn_inner(
        app.clone(),
        SpawnArgs {
            provider: provider.to_string(),
            name: name.clone(),
            role: Some("worker".into()),
            cwd: cwd.clone(),
            command: command.clone(),
            seed_prompt: None,
            isolate: raw["isolate"].as_bool().unwrap_or(true), // upstream default
            is_god: false,
            no_auto_install: false,
            cols: 120,
            rows: 32,
        },
        Some(worker_id.clone()),
        None,
    );
    if let Err(e) = spawned {
        crate::integrations::BROKER.revoke(&worker_id);
        fail!(format!("spawn failed — {e}"));
    }

    // A god-hired worker is a main-initiated spawn — broadcast the card so
    // the floor sees it (upstream 'hive:agentSpawned').
    let _ = app.emit(
        "hive:agentSpawned",
        json!({
            "id": worker_id, "name": name, "provider": provider,
            "cwd": cwd, "command": command, "role": "worker",
        }),
    );

    let token_cap = raw["tokenCap"]
        .as_f64()
        .filter(|c| c.is_finite() && *c > 0.0)
        .map(|c| c as u64);
    st.live_workers.lock().unwrap().insert(
        worker_id.clone(),
        WorkerRec {
            req_id: req_id.clone(),
            name: name.clone(),
            slack: slack.clone(),
            base_branch,
            spawned_at: now_ms(),
            token_cap,
            releasing: false,
        },
    );

    // Dispatch the objective via the standard inbox path — `from: god`, the
    // autonomous-worker preamble (Slack protocol when coords came along) and
    // the completion contract: signal done to release.
    let prefix = match &slack {
        Some((ch, ts)) => {
            let helper = crate::misc::write_slack_reply_script(&crate::misc::harness_home(app))
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "slack-reply".into());
            crate::slack::autonomous_request_protocol(ch, ts, &helper)
        }
        None => "[AUTONOMOUS WORKER TASK — no interactive human is watching. Work autonomously; do not ask interactive questions.] The task starts now: ".to_string(),
    };
    let suffix = "\n\n[WORKER COMPLETION] When finished, signal done by sending ONE outbox message to god with \"act\":\"done\" and a short result summary — that releases this ephemeral worker (terminal closed; your branch is handed to god). Do NOT push to any remote; god is the sole integrator.";
    let mut msg = crate::hive::Message::new(
        "god",
        &worker_id,
        "request",
        &name,
        &format!("{prefix}{objective}{suffix}"),
    );
    msg.conversation = format!("worker-{req_id}");
    if let Err(e) = st.hive.send_routed(&msg) {
        log::warn!("[worker] dispatch send failed: {e}");
    }
    log::warn!("[worker] spawned {worker_id} (cwd={cwd})");
    archive_request(path, ".done");
}

/// One watcher tick: (1) finish/reap live workers — a freed slot is reused
/// the same tick — then (2) pull new requests up to the concurrency cap.
pub fn tick(app: &AppHandle) {
    let st = state(app);
    let cfg = st.config.lock().unwrap().clone();
    let max_workers = cfg.max_concurrent_workers.max(1) as usize;
    let idle_timeout_ms = (cfg.worker_idle_timeout_minutes.max(1) as u64) * 60_000;
    let default_cap = cfg.default_worker_token_cap;
    let now = now_ms();

    // (1) Finish or reap. Each release tears the PTY down explicitly, like
    // every other kill site.
    let ids: Vec<String> = st.live_workers.lock().unwrap().keys().cloned().collect();
    for worker_id in ids {
        let (releasing, rec_slack, rec_req_id, rec_spawned, rec_cap) = {
            let lw = st.live_workers.lock().unwrap();
            match lw.get(&worker_id) {
                Some(r) => (
                    r.releasing,
                    r.slack.clone(),
                    r.req_id.clone(),
                    r.spawned_at,
                    r.token_cap,
                ),
                None => continue,
            }
        };
        if releasing {
            continue;
        }
        if let Some(done_at) = worker_done_at(&st.hive, &worker_id, rec_spawned) {
            // Success: settle its inbox so a re-hired incarnation doesn't boot
            // into stale orders — only mail from BEFORE the done signal is
            // filed; a follow-up that crossed it stays pending.
            if let Some(r) = st.live_workers.lock().unwrap().get_mut(&worker_id) {
                r.releasing = true;
            }
            let settled = st.hive.settle_inbox(&worker_id, done_at);
            // Its own work order (conversation `worker-<reqId>`) is what it
            // just completed — reporting that "filed unread" costs god a turn
            // for nothing; only OTHER requests/queries count.
            let unanswered: Vec<&crate::hive::UnansweredMessage> = settled
                .unanswered
                .iter()
                .filter(|m| m.conversation != format!("worker-{rec_req_id}"))
                .collect();
            if !unanswered.is_empty() {
                let one_line = |s: &str| s.replace(['\r', '\n'], " ");
                let lines = unanswered
                    .iter()
                    .map(|m| {
                        format!(
                            "- {} {} from {}: \"{}\"",
                            m.act,
                            one_line(&m.id),
                            one_line(&m.from),
                            one_line(&m.subject)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                inform_god(
                    &st.hive,
                    &format!("[worker released — mail filed unread] {worker_id}"),
                    &format!(
                        "Worker {worker_id} signaled done and was released. These messages were still unread in its inbox and were filed under inbox/.done without an answer:\n{lines}\nIf one of them is not the order it just completed, nobody is working on it — resend it to another agent or re-hire the worker."
                    ),
                    rec_slack.as_ref(),
                );
            }
            st.live_workers.lock().unwrap().remove(&worker_id);
            let _ = crate::commands::kill_agent(app.clone(), worker_id);
            continue;
        }
        // Token-cap reap (default-off plumbing).
        let cap = rec_cap.unwrap_or(default_cap);
        if cap > 0 && worker_tokens_used(app, &worker_id) > cap {
            if let Some(r) = st.live_workers.lock().unwrap().get_mut(&worker_id) {
                r.releasing = true;
            }
            let used = worker_tokens_used(app, &worker_id);
            inform_god(
                &st.hive,
                &format!("[worker reaped — token cap] {worker_id}"),
                &format!(
                    "Worker {worker_id} used {used} tokens (> its cap of {cap}) and was reaped. Any committed work on its branch is preserved for you."
                ),
                rec_slack.as_ref(),
            );
            st.live_workers.lock().unwrap().remove(&worker_id);
            let _ = crate::commands::kill_agent(app.clone(), worker_id);
            continue;
        }
        // Idle reap — no PTY output for the configured window and never
        // signaled done.
        let idle_ms = st.ptys.lock().unwrap().get(&worker_id).map(|h| {
            now.saturating_sub(h.last_output_at.load(std::sync::atomic::Ordering::Relaxed))
        });
        match idle_ms {
            None => continue, // PTY already gone — teardown paths clean up
            Some(ms) if ms > idle_timeout_ms => {
                if let Some(r) = st.live_workers.lock().unwrap().get_mut(&worker_id) {
                    r.releasing = true;
                }
                inform_god(
                    &st.hive,
                    &format!("[worker reaped — idle] {worker_id}"),
                    &format!(
                        "Worker {worker_id} produced no output for {} min (> the {} min cap) and never signaled done, so it was reaped. Any committed work on its branch is preserved for you.",
                        ms / 60_000,
                        idle_timeout_ms / 60_000,
                    ),
                    rec_slack.as_ref(),
                );
                st.live_workers.lock().unwrap().remove(&worker_id);
                let _ = crate::commands::kill_agent(app.clone(), worker_id);
            }
            _ => {}
        }
    }

    // (3) GC preserved worktrees whose work has since integrated. Throttled
    // to GC_SWEEP_MS; a no-op when nothing is preserved (the common case).
    {
        static LAST_SWEEP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let prev = LAST_SWEEP.load(std::sync::atomic::Ordering::Relaxed);
        if !st.preserved_worktrees.lock().unwrap().is_empty()
            && now.saturating_sub(prev) >= 60_000
            && LAST_SWEEP
                .compare_exchange(
                    prev,
                    now,
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                )
                .is_ok()
        {
            gc_preserved_worktrees(&st);
        }
    }

    // (2) Intake — gated on orchestratorMaySpawn (default OFF). Declining
    // also means declining to CONSUME: a request stays queued and runs when
    // the toggle turns on.
    if !cfg.orchestrator_may_spawn {
        return;
    }
    let dir = spawn_requests_dir(&st.hive);
    let _ = fs::create_dir_all(&dir);
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    for f in files {
        if st.live_workers.lock().unwrap().len() >= max_workers {
            break; // backpressure — the rest waits for a freed slot
        }
        process_spawn_request(app, &f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hive::RegistryEntry;

    fn tmp_hive(tag: &str) -> (PathBuf, crate::hive::Hive) {
        let root = std::env::temp_dir().join(format!("oa-workers-{tag}-{}", uuid::Uuid::new_v4()));
        let hive = crate::hive::Hive::ensure(&root.join("hive")).unwrap();
        hive.register_agent(&RegistryEntry {
            id: "worker-x".into(),
            name: "worker-x".into(),
            provider: "claude".into(),
            role: "worker".into(),
            cwd: root.display().to_string(),
            status: "idle".into(),
            ..Default::default()
        })
        .unwrap();
        (root, hive)
    }

    /// The stale-done guard (upstream workerDoneAt): a `done` authored BEFORE
    /// this worker spawned — left by a prior incarnation of the same reqId —
    /// must NOT release it.
    #[test]
    fn done_at_ignores_stale_and_counts_fresh() {
        let (_dir, hive) = tmp_hive("wdone");
        let outbox = hive.agent_dir("worker-x").join("outbox");
        let stale = crate::hive::Message::new("worker-x", "god", "done", "old", "b");
        let stale_at = stale
            .created_at
            .as_str()
            .parse::<chrono::DateTime<chrono::Utc>>()
            .map(|d| d.timestamp_millis() as u64 + 1)
            .unwrap_or(1);
        fs::write(
            outbox.join(format!("{}.json", stale.id)),
            serde_json::to_string(&stale).unwrap(),
        )
        .unwrap();
        // Spawned AFTER the stale done was authored → not done.
        assert_eq!(worker_done_at(&hive, "worker-x", stale_at), None);
        // Spawned BEFORE it → the fresh done is returned.
        let r = worker_done_at(&hive, "worker-x", stale_at - 5000);
        assert!(r.is_some());
        // A malformed file never wedges the scan.
        fs::write(outbox.join("junk.json"), "{not json").unwrap();
        assert!(worker_done_at(&hive, "worker-x", stale_at - 5000).is_some());
    }
}

/// workers:list — the renderer Workers tab snapshot (upstream
/// WorkerSnapshot[]; preserved worktrees are not tracked in this port yet).
pub fn list(app: &AppHandle) -> Value {
    let st = state(app);
    let cfg = st.config.lock().unwrap().clone();
    let now = now_ms();
    let live: Vec<Value> = st
        .live_workers
        .lock()
        .unwrap()
        .values()
        .map(|r| {
            let idle_ms = st
                .ptys
                .lock()
                .unwrap()
                .get(&format!("worker-{}", r.req_id))
                .map(|h| {
                    now.saturating_sub(
                        h.last_output_at
                            .load(std::sync::atomic::Ordering::Relaxed),
                    )
                });
            json!({
                "workerId": format!("worker-{}", r.req_id),
                "reqId": r.req_id,
                "name": r.name,
                "baseBranch": r.base_branch,
                "spawnedAt": r.spawned_at,
                "ageMs": now.saturating_sub(r.spawned_at),
                "idleMs": idle_ms,
                "tokensUsed": worker_tokens_used(app, &format!("worker-{}", r.req_id)),
                "tokenCap": r.token_cap.or((cfg.default_worker_token_cap > 0).then_some(cfg.default_worker_token_cap)),
                "hasSlack": r.slack.is_some(),
                "releasing": r.releasing,
                "status": if r.releasing { "releasing" } else { "working" },
            })
        })
        .collect();
    let preserved: Vec<Value> = st
        .preserved_worktrees
        .lock()
        .unwrap()
        .values()
        .map(|e| {
            json!({
                "workerId": e.worker_id,
                "wtPath": e.wt_path.display().to_string(),
                "baseBranch": e.base_branch,
                "preservedAt": e.preserved_at,
            })
        })
        .collect();
    json!({
        "live": live,
        "preserved": preserved,
        "maxWorkers": cfg.max_concurrent_workers.max(1),
    })
}
