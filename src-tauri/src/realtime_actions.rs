//! Voice action dispatcher — port of `src/main/realtimeActions.ts` (885 lines).
//!
//! Three commands back the renderer's `realtime:action*` channels:
//!   `realtime_action`         — propose/execute one verb (soft runs now,
//!                               destructive stages a single-slot pending)
//!   `realtime_action_confirm` — commits the pending iff the phrase carries
//!                               the distinct confirm token (bare "yes"
//!                               never authorizes)
//!   `realtime_action_cancel`  — drops the pending
//!
//! Every exec maps onto the same backend fns the renderer buttons call
//! (hive send / task mutations / spawn_inner / control.* / missions save /
//! kill_agent) — thin wrappers only, exactly like upstream's deps design.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::Serialize;
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter};

use crate::commands::state;
use crate::commands::{self, SpawnArgs};
use crate::hive::{PartialMessage, RegistryEntry};
use crate::missions::ScheduledMission;

const VOICE_ACTOR: &str = "michael-voice";
const PENDING_TTL_MS: u64 = 120_000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionResult {
    pub ok: bool,
    pub spoken: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_confirm: Option<bool>,
}

fn ok(spoken: impl Into<String>) -> ActionResult {
    ActionResult {
        ok: true,
        spoken: spoken.into(),
        needs_confirm: None,
    }
}
fn err(spoken: impl Into<String>) -> ActionResult {
    ActionResult {
        ok: false,
        spoken: spoken.into(),
        needs_confirm: None,
    }
}
fn pending_result(spoken: impl Into<String>) -> ActionResult {
    ActionResult {
        ok: true,
        spoken: spoken.into(),
        needs_confirm: Some(true),
    }
}

struct VerbSpec {
    destructive: bool,
    confirm_word: &'static str,
    agent_targeted: bool,
}

fn verb_spec(verb: &str) -> Option<VerbSpec> {
    // (destructive, confirm_word, agent_targeted) — upstream VERBS table.
    let (d, w, t): (bool, &'static str, bool) = match verb {
        "ping" => (false, "ping", true),
        "create_task" => (false, "create", false),
        "assign_task" => (false, "assign", false),
        "update_task" => (false, "update", false),
        "dispatch" => (false, "dispatch", true),
        "steer" => (false, "steer", true),
        "spawn" => (true, "spawn", false),
        "kill" => (true, "kill", true),
        "pause" => (true, "pause", true),
        "halt" => (true, "halt", true),
        "edit_schedule" => (true, "schedule", false),
        "resume" => (false, "resume", true),
        "auto_delivery" => (false, "delivery", true),
        "gate_tool" => (false, "gate", true),
        "delete_task" => (false, "delete", false),
        "unarchive" => (false, "unarchive", true),
        "clear_context" => (true, "clear", true),
        "archive" => (true, "archive", true),
        "create_schedule" => (true, "schedule", false),
        "update_setting" => (true, "setting", false),
        _ => return None,
    };
    Some(VerbSpec {
        destructive: d,
        confirm_word: w,
        agent_targeted: t,
    })
}

struct SettingPolicy {
    confirm: bool,
    ty: &'static str, // 'boolean' | 'number' | 'string'
    min: Option<f64>,
    max: Option<f64>,
    values: Option<&'static [&'static str]>,
}

fn setting_policy(key: &str) -> Option<SettingPolicy> {
    // Upstream SETTING_POLICY — the ONLY settings voice may touch.
    type P = SettingPolicy;
    let soft = |ty: &'static str| P {
        confirm: false,
        ty,
        min: None,
        max: None,
        values: None,
    };
    let conf = |ty: &'static str| P {
        confirm: true,
        ty,
        min: None,
        max: None,
        values: None,
    };
    Some(match key {
        "notifications" => soft("boolean"),
        "tvShowOffices" => soft("boolean"),
        "officeTheme" => P {
            confirm: false,
            ty: "string",
            min: None,
            max: None,
            values: Some(&[
                "office",
                "friends",
                "brooklyn99",
                "siliconvalley",
                "got",
                "hogwarts",
            ]),
        },
        "terminalTheme" => P {
            confirm: false,
            ty: "string",
            min: None,
            max: None,
            values: Some(&["light", "dark"]),
        },
        "freeflowEnabled" => soft("boolean"),
        "strongKeepalive" => soft("boolean"),
        "autoUpdate" => soft("boolean"),
        "realtimeIdleDisconnectMs" => P {
            confirm: false,
            ty: "number",
            min: Some(30_000.0),
            max: Some(3_600_000.0),
            values: None,
        },
        "autoMode" => conf("boolean"),
        "defaultModel" => conf("string"),
        "godProvider" => conf("string"),
        "godModel" => conf("string"),
        "maxConcurrentWorkers" => P {
            confirm: true,
            ty: "number",
            min: Some(1.0),
            max: Some(16.0),
            values: None,
        },
        "costCapTokens" => P {
            confirm: true,
            ty: "number",
            min: Some(0.0),
            max: Some(1_000_000_000.0),
            values: None,
        },
        "maxTurns" => P {
            confirm: true,
            ty: "number",
            min: Some(1.0),
            max: Some(1000.0),
            values: None,
        },
        "slackEnabled" => conf("boolean"),
        "webhookEnabled" => conf("boolean"),
        "semanticMemory" => conf("boolean"),
        "multiWindow" => conf("boolean"),
        _ => return None,
    })
}

const BARE_AFFIRMATIONS: &[&str] = &[
    "yes",
    "yeah",
    "yep",
    "yup",
    "ya",
    "ok",
    "okay",
    "k",
    "sure",
    "go",
    "go ahead",
    "do it",
    "please",
    "fine",
    "affirmative",
    "uh huh",
    "mhm",
    "mm hmm",
    "right",
    "correct",
];

fn provider_command(provider: &str) -> &'static str {
    match provider {
        "codex" => "codex",
        "antigravity" => "antigravity",
        "gemini" => "gemini",
        "opencode" => "opencode",
        "crush" => "crush",
        "pi" => "pi",
        "qwen" => "qwen",
        "copilot" => "copilot",
        "cursor" => "cursor-agent",
        _ => "claude",
    }
}

/// Per-provider context-clear verbs — upstream `contextCommandsForProvider(.).clear`.
/// `None` = nothing safe to type (copilot/cursor/crush/custom), NOT unchecked.
/// clearCommandForProvider — single source is misc::context_commands
/// (shared/providerAutomation.ts ported to backend); the empty message falls
/// back to the table verb.
fn clear_command(provider: &str) -> Option<String> {
    crate::misc::clear_command(provider, "")
}

fn normalize_provider(v: Option<&str>) -> Option<String> {
    const KNOWN: &[&str] = &[
        "claude",
        "codex",
        "grok",
        "kimi",
        "gemini",
        "antigravity",
        "qwen",
        "opencode",
        "crush",
        "pi",
        "copilot",
        "cursor",
        "custom",
    ];
    v.and_then(|p| {
        let p = p.trim().to_lowercase();
        KNOWN.contains(&p.as_str()).then(|| p)
    })
}

/// `inferAgentProvider(undefined, explicit)` — explicit registry field wins;
/// no command to sniff from in the voice path.
fn infer_provider(explicit: Option<&str>) -> String {
    normalize_provider(explicit).unwrap_or_else(|| "claude".to_string())
}

// ─── helpers ────────────────────────────────────────────────────────────────

fn strv(a: &Map<String, Value>, key: &str) -> String {
    a.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}
fn first_str(a: &Map<String, Value>, keys: &[&str]) -> String {
    keys.iter()
        .map(|k| strv(a, k))
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

fn norm(s: &str) -> String {
    let lower: String = s
        .to_lowercase()
        .chars()
        .map(|c| {
            if matches!(c, '.' | '!' | '?' | ',' | ';' | ':' | '\'' | '"') {
                ' '
            } else {
                c
            }
        })
        .collect();
    lower.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn norm_match(s: &str) -> String {
    let lower: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect();
    lower.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn toks_match(s: &str) -> Vec<String> {
    norm_match(s)
        .split(' ')
        .filter(|w| !w.is_empty())
        .map(|w| w.to_string())
        .collect()
}

fn short_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Voice cannot target 'everyone'/'all agents' with a destructive verb.
fn is_mass_target(target: &str) -> bool {
    let t = norm(target);
    matches!(
        t.as_str(),
        "all"
            | "everyone"
            | "everybody"
            | "all agents"
            | "every agent"
            | "the whole office"
            | "whole office"
            | "the office"
            | "all of them"
            | "every one"
    )
}

struct ResolvedAgent {
    id: String,
    name: String,
    is_god: bool,
}

fn resolve_agent(
    target: &str,
    god_id: &Option<String>,
    agents: &[RegistryEntry],
) -> Result<ResolvedAgent, String> {
    let t = norm(target);
    if t.is_empty() {
        return Err("no agent was named".into());
    }
    let mk = |e: &RegistryEntry, is_god: bool| ResolvedAgent {
        id: e.id.clone(),
        name: if e.name.is_empty() {
            e.id.clone()
        } else {
            e.name.clone()
        },
        is_god: is_god || e.is_god,
    };
    // exact id
    if let Some(e) = agents.iter().find(|e| e.id.to_lowercase() == t) {
        let g = god_id.as_deref() == Some(e.id.as_str());
        return Ok(mk(e, g));
    }
    // 'god' / 'michael' alias for the orchestrator
    if matches!(t.as_str(), "god" | "michael" | "the god") {
        if let Some(gid) = god_id {
            let e = agents.iter().find(|e| &e.id == gid);
            return Ok(match e {
                Some(e) => mk(e, true),
                None => ResolvedAgent {
                    id: gid.clone(),
                    name: gid.clone(),
                    is_god: true,
                },
            });
        }
    }
    // exact name, prefer live
    let by_name: Vec<&RegistryEntry> = agents
        .iter()
        .filter(|e| e.name.to_lowercase() == t)
        .collect();
    let live_name: Vec<&RegistryEntry> = by_name.iter().copied().filter(|e| !e.archived).collect();
    let name_pick: &[&RegistryEntry] = if !live_name.is_empty() {
        &live_name
    } else {
        &by_name
    };
    if name_pick.len() == 1 {
        let e = name_pick[0];
        let g = god_id.as_deref() == Some(e.id.as_str());
        return Ok(mk(e, g));
    }
    if name_pick.len() > 1 {
        return Err(format!(
            "{} agents are named {} — say the exact agent id",
            name_pick.len(),
            target
        ));
    }
    // partial contains, live only
    let partial: Vec<&RegistryEntry> = agents
        .iter()
        .filter(|e| {
            !e.archived && (e.id.to_lowercase().contains(&t) || e.name.to_lowercase().contains(&t))
        })
        .collect();
    if partial.len() == 1 {
        let e = partial[0];
        let g = god_id.as_deref() == Some(e.id.as_str());
        return Ok(mk(e, g));
    }
    if partial.len() > 1 {
        return Err(format!(
            "several agents match \"{}\" — be more specific or say an id",
            target
        ));
    }
    Err(format!("I don't see an agent matching \"{}\"", target))
}

/// Distinct-token confirm: the phrase must carry the verb word or 'confirm';
/// a bare affirmation never authorizes a destructive op.
fn confirm_accepted(phrase: &str, confirm_word: &str) -> bool {
    let p = norm(phrase);
    if p.is_empty() || BARE_AFFIRMATIONS.contains(&p.as_str()) {
        return false;
    }
    let toks: Vec<&str> = p.split(' ').collect();
    toks.iter()
        .any(|t| t.starts_with("confirm") || *t == confirm_word)
}

// ─── pending (single-slot two-phase confirm) ────────────────────────────────

/// What a confirm commits — kept as data (not a closure) so the pending slot
/// lives in a plain Mutex.
#[derive(Debug, Clone)]
enum CommitSpec {
    Kill {
        id: String,
        name: String,
    },
    Pause {
        id: String,
        name: String,
    },
    Halt {
        id: String,
        name: String,
    },
    ClearContext {
        id: String,
        name: String,
    },
    Archive {
        id: String,
        name: String,
    },
    Spawn {
        spec: VoiceSpawn,
        label: String,
    },
    EditSchedule {
        mission: ScheduledMission,
        action: String,
    },
    CreateSchedule {
        mission: ScheduledMission,
        minutes: u64,
    },
    UpdateSetting {
        key: String,
        value: Value,
    },
}

#[derive(Debug, Clone, Serialize)]
struct VoiceSpawn {
    id: String,
    cwd: String,
    command: String,
    provider: String,
    name: String,
    role: Option<String>,
}

struct Pending {
    verb: String,
    confirm_word: String,
    target_label: String,
    created_at: u64,
    commit: CommitSpec,
}

static PENDING: std::sync::LazyLock<Mutex<Option<Pending>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

fn pending_fresh() -> Option<Pending> {
    let mut g = PENDING.lock().unwrap();
    if let Some(p) = g.as_ref() {
        if now_ms().saturating_sub(p.created_at) > PENDING_TTL_MS {
            *g = None;
        }
    }
    g.as_ref().map(|p| Pending {
        verb: p.verb.clone(),
        confirm_word: p.confirm_word.clone(),
        target_label: p.target_label.clone(),
        created_at: p.created_at,
        commit: p.commit.clone(),
    })
}

fn stage(p: Pending) {
    *PENDING.lock().unwrap() = Some(p);
}

// ─── deps (the upstream dep-table, as small fns over AppState) ─────────────

fn hive_enabled(app: &AppHandle) -> bool {
    state(app)
        .hive
        .root
        .parent()
        .map(|p| p.exists())
        .unwrap_or(false)
        || state(app).hive.root.exists()
}

fn hive_send(
    app: &AppHandle,
    to: &str,
    act: &str,
    subject: &str,
    body: &str,
    requires_reply: bool,
) -> Result<Value> {
    let partial = PartialMessage {
        to: Some(to.to_string()),
        act: Some(act.to_string()),
        subject: Some(subject.to_string()),
        body: Some(body.to_string()),
        requires_reply: requires_reply.then_some(true),
        ..Default::default()
    };
    state(app)
        .hive
        .send_partial(&partial, VOICE_ACTOR)
        .map(|m| json!(m))
}

fn registry_doc(app: &AppHandle) -> (Option<String>, Vec<RegistryEntry>) {
    state(app).hive.read_registry_doc()
}

fn find_tasks(app: &AppHandle) -> Vec<Value> {
    state(app)
        .hive
        .read_tasks()
        .ok()
        .and_then(|v| v.get("tasks").and_then(|t| t.as_array().cloned()))
        .unwrap_or_default()
}

fn god_display_name(app: &AppHandle) -> String {
    let (god_id, agents) = registry_doc(app);
    god_id
        .and_then(|g| agents.iter().find(|e| e.id == g).map(|e| e.name.clone()))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Michael".into())
}

/// voice-action attribution: hive log + cross-notify the god PTY, both
/// best-effort (upstream try/catch twins).
fn attribute(app: &AppHandle, verb: &str, target: &str, extra: Map<String, Value>) {
    let mut ev =
        json!({ "kind": "voice_action", "actor": VOICE_ACTOR, "verb": verb, "target": target });
    if let Some(dst) = ev.as_object_mut() {
        for (k, v) in &extra {
            dst.insert(k.clone(), v.clone());
        }
    }
    let _ = state(app).hive.log(&ev);
    let detail = extra
        .get("objective")
        .or_else(|| extra.get("text"))
        .or_else(|| extra.get("title"))
        .and_then(|v| v.as_str())
        .map(|s| format!(": {}", s))
        .or_else(|| {
            extra
                .get("status")
                .and_then(|v| v.as_str())
                .map(|s| format!(" → {}", s))
        })
        .or_else(|| {
            extra
                .get("action")
                .and_then(|v| v.as_str())
                .map(|s| format!(" ({})", s))
        })
        .unwrap_or_default();
    let god = god_display_name(app);
    let _ = hive_send(
        app,
        "god",
        "inform",
        &format!("voice action: {} {}", verb, target),
        &format!(
            "{} (voice orchestrator, {}) just did: {} on {}{}. Heads-up so we don't duplicate — the board is the single source of truth.",
            god, VOICE_ACTOR, verb, target, detail
        ),
        false,
    );
}

fn log_action_failure(app: &AppHandle, channel: &str, verb: &str, msg: &str) {
    let _ = state(app).hive.log(&json!({
        "kind": "voice_action_error",
        "actor": VOICE_ACTOR,
        "channel": channel,
        "verb": verb,
        "error": msg,
    }));
}

// ─── task card matching (port of scoreCard/findCard) ───────────────────────

const AMBIGUOUS_MARGIN: f64 = 0.08;

fn score_card(ref_norm: &str, ref_toks: &[String], title: &str, id: &str) -> f64 {
    if ref_norm.is_empty() {
        return 0.0;
    }
    let title_n = norm_match(title);
    let id_n = norm_match(id);
    if id_n == ref_norm || title_n == ref_norm {
        return 1.0;
    }
    if !title_n.is_empty() && (title_n.starts_with(ref_norm) || ref_norm.starts_with(&title_n)) {
        return 0.92;
    }
    if !id_n.is_empty() && id_n.starts_with(ref_norm) {
        return 0.9;
    }
    let mut hay: Vec<String> = toks_match(title);
    hay.extend(toks_match(id));
    let coverage = if ref_toks.is_empty() {
        0.0
    } else {
        ref_toks
            .iter()
            .filter(|w| hay.iter().any(|h| h == *w))
            .count() as f64
            / ref_toks.len() as f64
    };
    let prefix_cov = if ref_toks.is_empty() {
        0.0
    } else {
        ref_toks
            .iter()
            .filter(|w| {
                hay.iter()
                    .any(|h| h.starts_with(w.as_str()) || w.starts_with(h.as_str()))
            })
            .count() as f64
            / ref_toks.len() as f64
    };
    if coverage == 1.0 {
        return 0.85;
    }
    if title_n.contains(ref_norm) || id_n.contains(ref_norm) {
        return coverage.max(0.7);
    }
    if prefix_cov == 1.0 {
        return 0.78;
    }
    coverage.max(prefix_cov) * 0.7
}

struct CardFind {
    card: Option<Value>,
    ambiguous: Vec<Value>,
}

fn find_card(app: &AppHandle, refr: &str) -> CardFind {
    let tasks = find_tasks(app);
    let ref_norm = norm_match(refr);
    let ref_toks = toks_match(refr);
    let mut scored: Vec<(f64, Value)> = tasks
        .into_iter()
        .map(|c| {
            let s = score_card(
                &ref_norm,
                &ref_toks,
                c.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                c.get("id").and_then(|v| v.as_str()).unwrap_or(""),
            );
            (s, c)
        })
        .filter(|(s, _)| *s >= 0.45)
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    if scored.is_empty() {
        return CardFind {
            card: None,
            ambiguous: vec![],
        };
    }
    let top = scored[0].0;
    let close: Vec<&(f64, Value)> = scored
        .iter()
        .filter(|(s, _)| *s >= top - AMBIGUOUS_MARGIN)
        .collect();
    if close.len() > 1 {
        return CardFind {
            card: None,
            ambiguous: close.iter().take(3).map(|(_, c)| c.clone()).collect(),
        };
    }
    CardFind {
        card: Some(scored[0].1.clone()),
        ambiguous: vec![],
    }
}

fn ambiguous_spoken(ambiguous: &[Value]) -> String {
    let names: Vec<String> = ambiguous
        .iter()
        .map(|c| {
            format!(
                "\"{}\"",
                c.get("title").and_then(|v| v.as_str()).unwrap_or("?")
            )
        })
        .collect();
    format!("Which one — {}?", names.join(", or "))
}

// ─── soft executors ─────────────────────────────────────────────────────────

fn target_of(a: &Map<String, Value>) -> String {
    first_str(a, &["agentId", "target", "name"])
}

fn exec_ping(app: &AppHandle, a: &Map<String, Value>) -> Result<ActionResult> {
    let (g, agents) = registry_doc(app);
    let r = resolve_agent(&target_of(a), &g, &agents).map_err(|e| anyhow::anyhow!(e))?;
    let message = {
        let m = first_str(a, &["message", "text"]);
        if m.is_empty() {
            "Checking in.".to_string()
        } else {
            m
        }
    };
    let god = god_display_name(app);
    hive_send(
        app,
        &r.id,
        "inform",
        &format!("Voice ping from {}", god),
        &message,
        false,
    )?;
    attribute(app, "ping", &r.id, Map::new());
    Ok(ok(format!("Pinged {}.", r.name)))
}

fn exec_dispatch(app: &AppHandle, a: &Map<String, Value>) -> Result<ActionResult> {
    let (g, agents) = registry_doc(app);
    let r = resolve_agent(&target_of(a), &g, &agents).map_err(|e| anyhow::anyhow!(e))?;
    let objective = first_str(a, &["objective", "task", "message"]);
    if objective.is_empty() {
        return Ok(err("What should I dispatch? I need an objective."));
    }
    let ctx = {
        let v = strv(a, "context");
        if v.is_empty() {
            "(none given)".to_string()
        } else {
            v
        }
    };
    let constraints = {
        let v = strv(a, "constraints");
        if v.is_empty() {
            "(use your judgement; respect the guardrails)".to_string()
        } else {
            v
        }
    };
    let done = {
        let v = first_str(a, &["doneWhen", "done"]);
        if v.is_empty() {
            "you report the outcome back to god".to_string()
        } else {
            v
        }
    };
    let body = format!(
        "OBJECTIVE: {}\nCONTEXT: {}\nCONSTRAINTS: {}\nDONE WHEN: {}",
        objective, ctx, constraints, done
    );
    let short: String = objective.chars().take(60).collect();
    let msg = hive_send(
        app,
        &r.id,
        "request",
        &format!("Voice dispatch: {}", short),
        &body,
        true,
    )?;
    let mut extra = Map::new();
    extra.insert(
        "objective".into(),
        json!(objective.chars().take(120).collect::<String>()),
    );
    attribute(app, "dispatch", &r.id, extra);
    // rt-12: register with the completion watcher.
    if let Some(cid) = msg.get("id").and_then(|v| v.as_str()) {
        crate::realtime::WATCHER
            .lock()
            .unwrap()
            .track(crate::realtime::PendingDispatch {
                correlation_id: cid.to_string(),
                kind: crate::realtime::PendingKind::Dispatch,
                target_agent_id: r.id.clone(),
                task_id: None,
                objective: Some(objective.clone()),
                dispatched_at: now_ms(),
                dispatch_message_id: Some(cid.to_string()),
            });
    }
    Ok(ok(format!(
        "Dispatched to {}: {}.",
        r.name,
        objective.chars().take(80).collect::<String>()
    )))
}

fn exec_steer(app: &AppHandle, a: &Map<String, Value>) -> Result<ActionResult> {
    let (g, agents) = registry_doc(app);
    let r = resolve_agent(&target_of(a), &g, &agents).map_err(|e| anyhow::anyhow!(e))?;
    let text = first_str(a, &["text", "message", "steer"]);
    if text.is_empty() {
        return Ok(err("What guidance should I steer them with?"));
    }
    state(app)
        .control
        .lock()
        .unwrap()
        .steer(&r.id, &format!("[{}] {}", VOICE_ACTOR, text));
    let mut extra = Map::new();
    extra.insert(
        "text".into(),
        json!(text.chars().take(120).collect::<String>()),
    );
    attribute(app, "steer", &r.id, extra);
    Ok(ok(format!(
        "Steering {}: {}.",
        r.name,
        text.chars().take(80).collect::<String>()
    )))
}

fn exec_create_task(app: &AppHandle, a: &Map<String, Value>) -> Result<ActionResult> {
    let title = first_str(a, &["title", "task", "name"]);
    if title.is_empty() {
        return Ok(err("What should the task be titled?"));
    }
    let id = format!("{}-{}", slug(&title), short_id());
    let mut card = json!({
        "id": id,
        "title": title,
        "status": "todo",
        "dependsOn": [],
        "createdAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });
    let obj = card.as_object_mut().unwrap();
    let desc = strv(a, "description");
    if !desc.is_empty() {
        obj.insert("description".into(), json!(desc));
    }
    let assignee = strv(a, "assignee");
    if !assignee.is_empty() {
        obj.insert("assignee".into(), json!(assignee));
    }
    obj.insert(
        "priority".into(),
        a.get("priority")
            .and_then(|v| v.as_f64())
            .map(|n| json!(n as i64))
            .unwrap_or(json!(5)),
    );
    if crate::hive_admin::add_task(&state(app).hive, &card).is_err() {
        return Ok(err("I can't create that task because it already exists."));
    }
    let mut extra = Map::new();
    extra.insert(
        "title".into(),
        json!(title.chars().take(120).collect::<String>()),
    );
    extra.insert("assignee".into(), json!(assignee));
    attribute(app, "create_task", &id, extra);
    Ok(ok(format!(
        "Created task \"{}\"{}.",
        title,
        if assignee.is_empty() {
            "".into()
        } else {
            format!(", assigned to {}", assignee)
        }
    )))
}

fn exec_assign_task(app: &AppHandle, a: &Map<String, Value>) -> Result<ActionResult> {
    let refr = first_str(a, &["taskId", "task", "title"]);
    let assignee = first_str(a, &["assignee", "to", "agentId"]);
    if refr.is_empty() || assignee.is_empty() {
        return Ok(err("I need both a task and who to assign it to."));
    }
    let found = find_card(app, &refr);
    if !found.ambiguous.is_empty() {
        return Ok(err(ambiguous_spoken(&found.ambiguous)));
    }
    let Some(card) = found.card else {
        return Ok(err(format!(
            "I couldn't find a task matching \"{}\".",
            refr
        )));
    };
    let cid = card
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let title = card
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut patch = Map::new();
    patch.insert("assignee".into(), json!(assignee));
    if state(app).hive.update_task(&cid, &patch).is_err() {
        return Ok(err(format!("I couldn't update \"{}\" right now.", title)));
    }
    let mut extra = Map::new();
    extra.insert("assignee".into(), json!(assignee));
    attribute(app, "assign_task", &cid, extra);
    Ok(ok(format!("Assigned \"{}\" to {}.", title, assignee)))
}

fn exec_update_task(app: &AppHandle, a: &Map<String, Value>) -> Result<ActionResult> {
    let refr = first_str(a, &["taskId", "task", "title"]);
    if refr.is_empty() {
        return Ok(err("Which task should I update?"));
    }
    let found = find_card(app, &refr);
    if !found.ambiguous.is_empty() {
        return Ok(err(ambiguous_spoken(&found.ambiguous)));
    }
    let Some(card) = found.card else {
        return Ok(err(format!(
            "I couldn't find a task matching \"{}\".",
            refr
        )));
    };
    let status = strv(a, "status");
    const VALID: &[&str] = &["todo", "doing", "blocked", "done"];
    if !status.is_empty() && !VALID.contains(&status.as_str()) {
        return Ok(err(format!("\"{}\" isn't a valid status.", status)));
    }
    let mut patch = Map::new();
    if !status.is_empty() {
        patch.insert("status".into(), json!(status));
    }
    let result = strv(a, "result");
    if !result.is_empty() {
        patch.insert("result".into(), json!(result));
    }
    let assignee = strv(a, "assignee");
    if !assignee.is_empty() {
        patch.insert("assignee".into(), json!(assignee));
    }
    let cid = card
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let title = card
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if state(app).hive.update_task(&cid, &patch).is_err() {
        return Ok(err(format!("I couldn't update \"{}\" right now.", title)));
    }
    let mut extra = Map::new();
    extra.insert("status".into(), json!(status));
    attribute(app, "update_task", &cid, extra);
    Ok(ok(format!(
        "Updated \"{}\"{}.",
        title,
        if status.is_empty() {
            "".into()
        } else {
            format!(" to {}", status)
        }
    )))
}

fn exec_resume(app: &AppHandle, a: &Map<String, Value>) -> Result<ActionResult> {
    let (g, agents) = registry_doc(app);
    let r = resolve_agent(&target_of(a), &g, &agents).map_err(|e| anyhow::anyhow!(e))?;
    state(app).control.lock().unwrap().resume(&r.id);
    attribute(app, "resume", &r.id, Map::new());
    Ok(ok(format!("Resumed {} — tools flow again.", r.name)))
}

fn exec_auto_delivery(app: &AppHandle, a: &Map<String, Value>) -> Result<ActionResult> {
    let (g, agents) = registry_doc(app);
    let r = resolve_agent(&target_of(a), &g, &agents).map_err(|e| anyhow::anyhow!(e))?;
    let raw = {
        let v = first_str(a, &["state", "action"]);
        if !v.is_empty() {
            norm(&v)
        } else {
            match a.get("paused").and_then(|v| v.as_bool()) {
                Some(true) => "pause".to_string(),
                Some(false) => "resume".to_string(),
                None => String::new(),
            }
        }
    };
    if raw.is_empty() {
        return Ok(err("Should I pause or resume message delivery?"));
    }
    let paused = raw.contains("pause")
        || raw.contains("off")
        || raw.contains("hold")
        || raw.contains("stop");
    state(app)
        .control
        .lock()
        .unwrap()
        .pause_auto_delivery(&r.id, paused);
    let mut extra = Map::new();
    extra.insert(
        "action".into(),
        json!(if paused { "paused" } else { "resumed" }),
    );
    attribute(app, "auto_delivery", &r.id, extra);
    Ok(ok(if paused {
        format!(
            "Paused automatic delivery to {} — queued messages will wait.",
            r.name
        )
    } else {
        format!("Resumed automatic delivery to {}.", r.name)
    }))
}

fn exec_gate_tool(app: &AppHandle, a: &Map<String, Value>) -> Result<ActionResult> {
    let (g, agents) = registry_doc(app);
    let r = resolve_agent(&target_of(a), &g, &agents).map_err(|e| anyhow::anyhow!(e))?;
    let tool = first_str(a, &["tool", "toolName"]);
    let valid_name = {
        let mut cs = tool.chars();
        matches!(cs.next(), Some(c) if c.is_ascii_alphabetic())
            && tool.len() <= 64
            && tool
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };
    if tool.is_empty() || !valid_name {
        return Ok(err(
            "Which tool should I gate? Give me its exact name, like Bash or WebFetch.",
        ));
    }
    let raw = norm(&first_str(a, &["state", "action"]));
    let off = raw.contains("off")
        || raw.contains("allow")
        || raw.contains("ungate")
        || raw.contains("unblock")
        || raw.contains("enable");
    let on = !off; // default: gate it
    state(app)
        .control
        .lock()
        .unwrap()
        .gate_tool(&r.id, &tool, on);
    let mut extra = Map::new();
    extra.insert(
        "action".into(),
        json!(format!("{} {}", if on { "gated" } else { "ungated" }, tool)),
    );
    attribute(app, "gate_tool", &r.id, extra);
    Ok(ok(format!(
        "{} the {} tool for {}.",
        if on { "Gated" } else { "Un-gated" },
        tool,
        r.name
    )))
}

fn exec_delete_task(app: &AppHandle, a: &Map<String, Value>) -> Result<ActionResult> {
    let refr = first_str(a, &["taskId", "task", "title"]);
    if refr.is_empty() {
        return Ok(err("Which task should I delete?"));
    }
    let found = find_card(app, &refr);
    if !found.ambiguous.is_empty() {
        return Ok(err(ambiguous_spoken(&found.ambiguous)));
    }
    let Some(card) = found.card else {
        return Ok(err(format!(
            "I couldn't find a task matching \"{}\".",
            refr
        )));
    };
    let cid = card
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let title = card
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if crate::hive_admin::delete_task(&state(app).hive, &cid).is_err() {
        return Ok(err(format!("I couldn't delete \"{}\" right now.", title)));
    }
    let mut extra = Map::new();
    extra.insert(
        "title".into(),
        json!(title.chars().take(120).collect::<String>()),
    );
    attribute(app, "delete_task", &cid, extra);
    Ok(ok(format!(
        "Deleted the task \"{}\". Recreate it any time if that was wrong.",
        title
    )))
}

fn exec_unarchive(app: &AppHandle, a: &Map<String, Value>) -> Result<ActionResult> {
    let (g, agents) = registry_doc(app);
    let r = resolve_agent(&target_of(a), &g, &agents).map_err(|e| anyhow::anyhow!(e))?;
    let res = crate::hive_admin::set_archived(&state(app).hive, &r.id, false);
    // Upstream setArchived pushes hive:agentSpawned on an unarchive — the
    // floor re-adds the card.
    let _ = app.emit("hive:agentSpawned", json!({ "id": r.id }));
    attribute(app, "unarchive", &r.id, Map::new());
    Ok(match res {
        Result::Ok(_) => ok(format!("Brought {} back from the archive.", r.name)),
        Result::Err(e) => err(format!("Couldn't unarchive {}: {}.", r.name, e)),
    })
}

// ─── destructive: propose + commit ─────────────────────────────────────────

fn propose_destructive(
    app: &AppHandle,
    verb: &str,
    a: &Map<String, Value>,
    spec: &VerbSpec,
) -> Result<ActionResult> {
    let (g, agents) = registry_doc(app);

    if spec.agent_targeted {
        let raw_target = target_of(a);
        if is_mass_target(&raw_target) {
            return Ok(err(format!(
                "{} on all agents at once is voice-forbidden. Do it agent by agent, or use the UI.",
                verb
            )));
        }
        let r = resolve_agent(&raw_target, &g, &agents).map_err(|e| anyhow::anyhow!(e))?;
        // kill/pause/halt/archive on god stay voice-forbidden; clear_context
        // is allowed behind confirm (recoverable — sessions resume).
        if r.is_god && verb != "clear_context" {
            return Ok(err(format!(
                "{} on the god orchestrator is voice-forbidden. That has to be done in the UI.",
                verb
            )));
        }
        let commit = match verb {
            "kill" => CommitSpec::Kill {
                id: r.id.clone(),
                name: r.name.clone(),
            },
            "pause" => CommitSpec::Pause {
                id: r.id.clone(),
                name: r.name.clone(),
            },
            "halt" => CommitSpec::Halt {
                id: r.id.clone(),
                name: r.name.clone(),
            },
            "clear_context" => CommitSpec::ClearContext {
                id: r.id.clone(),
                name: r.name.clone(),
            },
            _ => CommitSpec::Archive {
                id: r.id.clone(),
                name: r.name.clone(),
            },
        };
        let snap = state(app).control.lock().unwrap().snapshot(&r.id);
        let note = if snap.halted {
            " (note: already halted)"
        } else if snap.paused {
            " (note: already paused)"
        } else {
            ""
        };
        stage(Pending {
            verb: verb.to_string(),
            confirm_word: spec.confirm_word.to_string(),
            target_label: r.name.clone(),
            created_at: now_ms(),
            commit,
        });
        let consequence = match verb {
            "clear_context" => format!(
                "That wipes {}'s working memory of the current conversation.",
                r.name
            ),
            "archive" => format!("That takes {} off the floor (history kept).", r.name),
            _ => "That's destructive.".to_string(),
        };
        return Ok(pending_result(format!(
            "You asked me to {} {}{}. {} To go ahead, say \"confirm\" or \"{}\". Say \"cancel\" to stop.",
            verb.replace('_', " "), r.name, note, consequence, spec.confirm_word
        )));
    }

    if verb == "spawn" {
        let provider = {
            let p = strv(a, "provider");
            if p.is_empty() {
                "claude".to_string()
            } else {
                p.to_lowercase()
            }
        };
        let role = first_str(a, &["role", "job"]);
        let name = {
            let n = strv(a, "name");
            if !n.is_empty() {
                n
            } else if !role.is_empty() {
                // role.replace(/\b\w/g, c=>c.toUpperCase())
                let mut out = String::new();
                let mut cap = true;
                for c in role.chars() {
                    if cap && c.is_alphabetic() {
                        out.extend(c.to_uppercase());
                        cap = false;
                    } else {
                        out.push(c);
                        cap = !c.is_alphanumeric();
                    }
                }
                out
            } else if !provider.is_empty() {
                provider.clone()
            } else {
                "Worker".to_string()
            }
        };
        let god_cwd = g.as_ref().and_then(|gid| {
            agents
                .iter()
                .find(|e| &e.id == gid)
                .map(|e| e.cwd.clone())
                .filter(|c| !c.is_empty())
        });
        let cwd = {
            let v = strv(a, "cwd");
            if !v.is_empty() {
                v
            } else if let Some(c) = god_cwd {
                c
            } else {
                agents
                    .iter()
                    .find(|e| !e.cwd.is_empty())
                    .map(|e| e.cwd.clone())
                    .unwrap_or_default()
            }
        };
        if cwd.is_empty() {
            return Ok(err(
                "I need a working directory to hire into — none is configured.",
            ));
        }
        let command = {
            let v = strv(a, "command");
            if v.is_empty() {
                provider_command(&provider).to_string()
            } else {
                v
            }
        };
        let id = format!("{}-{}", slug(&name), short_id());
        stage(Pending {
            verb: verb.to_string(),
            confirm_word: "spawn".to_string(),
            target_label: name.clone(),
            created_at: now_ms(),
            commit: CommitSpec::Spawn {
                spec: VoiceSpawn {
                    id,
                    cwd,
                    command,
                    provider: provider.clone(),
                    name: name.clone(),
                    role: if role.is_empty() {
                        None
                    } else {
                        Some(role.clone())
                    },
                },
                label: format!("{} on {}", name, provider),
            },
        });
        return Ok(pending_result(format!(
            "You want to hire a new {} agent{}, named {}. To hire, say \"confirm\" or \"spawn\". Say \"cancel\" to stop.",
            provider,
            if role.is_empty() { "".into() } else { format!(" as {}", role) },
            name
        )));
    }

    if verb == "edit_schedule" {
        let missions = {
            let st = state(app);
            let cfg = st.config.lock().unwrap();
            cfg.missions.clone()
        };
        if missions.is_empty() {
            return Ok(err("There are no scheduled missions to edit."));
        }
        let refr = norm(&first_str(a, &["missionId", "schedule", "label", "target"]));
        let m = missions
            .iter()
            .find(|x| x.id.to_lowercase() == refr)
            .or_else(|| missions.iter().find(|x| x.label.to_lowercase() == refr))
            .or_else(|| {
                missions.iter().find(|x| {
                    x.label.to_lowercase().contains(&refr) || x.id.to_lowercase().contains(&refr)
                })
            });
        let Some(m) = m else {
            return Ok(err(if refr.is_empty() {
                "Which schedule should I edit?".to_string()
            } else {
                format!("I couldn't find a schedule matching \"{}\".", refr)
            }));
        };
        let raw = norm(&first_str(a, &["action", "op"]));
        let action = if raw.contains("delete") || raw.contains("remove") {
            "delete"
        } else if raw.contains("disable") || raw.contains("off") || raw.contains("pause") {
            "disable"
        } else {
            "enable"
        };
        stage(Pending {
            verb: verb.to_string(),
            confirm_word: "schedule".to_string(),
            target_label: m.label.clone(),
            created_at: now_ms(),
            commit: CommitSpec::EditSchedule {
                mission: m.clone(),
                action: action.to_string(),
            },
        });
        return Ok(pending_result(format!(
            "You want to {} the \"{}\" schedule. To go ahead, say \"confirm\" or \"schedule\". Say \"cancel\" to stop.",
            action, m.label
        )));
    }

    if verb == "create_schedule" {
        let label = first_str(a, &["label", "name", "title"]);
        let body = first_str(a, &["prompt", "body", "message"]);
        if label.is_empty() || body.is_empty() {
            return Ok(err(
                "I need a name for the schedule and what it should tell the agent.",
            ));
        }
        let minutes = a
            .get("intervalMinutes")
            .and_then(|v| v.as_f64())
            .filter(|n| n.is_finite())
            .map(|n| (n.round() as i64).clamp(5, 7 * 24 * 60) as u64)
            .unwrap_or(60);
        let to = {
            let v = first_str(a, &["to", "agentId"]);
            if v.is_empty() {
                "god".to_string()
            } else {
                v
            }
        };
        let target = resolve_agent(&to, &g, &agents)
            .map(|r| r.id)
            .unwrap_or_else(|_| "god".into());
        let mission = ScheduledMission {
            id: format!("voice-{}-{}", slug(&label), short_id()),
            label: label.clone(),
            interval_ms: minutes * 60_000,
            to: target.clone(),
            body: body.clone(),
            enabled: true,
            ..Default::default()
        };
        stage(Pending {
            verb: verb.to_string(),
            confirm_word: "schedule".to_string(),
            target_label: label.clone(),
            created_at: now_ms(),
            commit: CommitSpec::CreateSchedule { mission, minutes },
        });
        return Ok(pending_result(format!(
            "You want a new schedule \"{}\", every {} minutes, messaging {}. To create it, say \"confirm\" or \"schedule\". Say \"cancel\" to stop.",
            label, minutes, target
        )));
    }

    if verb == "update_setting" {
        let key = first_str(a, &["key", "setting", "name"]);
        if key.is_empty() {
            return Ok(err("Which setting should I change?"));
        }
        let Some(policy) = setting_policy(&key) else {
            return Ok(err(format!(
                "The \"{}\" setting can't be changed by voice — use the Settings screen for that one.",
                key
            )));
        };
        // Coerce + validate against the declared type.
        let raw = a.get("value").cloned().unwrap_or(Value::Null);
        let value: Value = match policy.ty {
            "boolean" => {
                let b = match &raw {
                    Value::Bool(b) => Some(*b),
                    Value::String(s) => Some(matches!(
                        s.trim().to_lowercase().as_str(),
                        "true" | "on" | "yes" | "enable" | "enabled" | "1"
                    )),
                    _ => None,
                };
                match b {
                    Some(b) => json!(b),
                    None => return Ok(err(format!("Should {} be on or off?", key))),
                }
            }
            "number" => {
                let n = match &raw {
                    Value::Number(n) => n.as_f64(),
                    Value::String(s) => s.trim().parse::<f64>().ok(),
                    _ => None,
                };
                let Some(n) = n.filter(|n| n.is_finite()) else {
                    return Ok(err(format!("What number should {} be?", key)));
                };
                if let Some(min) = policy.min {
                    if n < min {
                        return Ok(err(format!("{} can't go below {}.", key, min as i64)));
                    }
                }
                if let Some(max) = policy.max {
                    if n > max {
                        return Ok(err(format!("{} can't go above {}.", key, max as i64)));
                    }
                }
                json!(n.round() as i64)
            }
            _ => {
                let Some(s) = raw.as_str().map(|s| s.trim().to_string()) else {
                    return Ok(err(format!("What should {} be set to?", key)));
                };
                if s.is_empty() || s.len() > 200 {
                    return Ok(err(format!("What should {} be set to?", key)));
                }
                if let Some(values) = policy.values {
                    if !values.contains(&s.as_str()) {
                        return Ok(err(format!(
                            "{} must be one of: {}.",
                            key,
                            values.join(", ")
                        )));
                    }
                }
                json!(s)
            }
        };
        let old_value = state(app)
            .config_store
            .raw_value()
            .get(&key)
            .cloned()
            .unwrap_or(Value::Null);
        let describe = |v: &Value| -> String {
            match v {
                Value::Bool(b) => {
                    if *b {
                        "on".into()
                    } else {
                        "off".into()
                    }
                }
                Value::Null => "unset".into(),
                _ => v.to_string().trim_matches('"').to_string(),
            }
        };
        if describe(&old_value) == describe(&value) {
            return Ok(ok(format!(
                "{} is already {} — nothing to change.",
                key,
                describe(&value)
            )));
        }
        let apply = |app: &AppHandle| -> Result<String> {
            let mut patch = Map::new();
            patch.insert(key.clone(), value.clone());
            let cfg = state(app).config_store.patch_raw(&patch)?;
            *state(app).config.lock().unwrap() = cfg;
            let _ = app.emit("config:changed", json!({}));
            let mut extra = Map::new();
            extra.insert(
                "action".into(),
                json!(format!("{} → {}", describe(&old_value), describe(&value))),
            );
            attribute(app, "update_setting", &key, extra);
            Ok(format!(
                "Done — {} is now {} (was {}).",
                key,
                describe(&value),
                describe(&old_value)
            ))
        };
        if !policy.confirm {
            return Ok(ok(apply(app)?));
        }
        stage(Pending {
            verb: verb.to_string(),
            confirm_word: "setting".to_string(),
            target_label: key.clone(),
            created_at: now_ms(),
            commit: CommitSpec::UpdateSetting {
                key: key.clone(),
                value: value.clone(),
            },
        });
        return Ok(pending_result(format!(
            "{} is {}; you want it {}. To change it, say \"confirm\" or \"setting\". Say \"cancel\" to stop.",
            key, describe(&old_value), describe(&value)
        )));
    }

    Ok(err(format!("I don't know how to {}.", verb)))
}

fn commit_pending(app: &AppHandle, commit: &CommitSpec) -> Result<String> {
    match commit {
        CommitSpec::Kill { id, name } => {
            // `id` is an AGENT id — kill_agent resolves either namespace.
            let res = commands::kill_agent(app.clone(), id.clone());
            // Voice kill is main-initiated — the renderer never removed the
            // card itself (unlike a UI kill), so tell the floor to archive it
            // (upstream killAgent dep → 'hive:agentArchived').
            if res.ok {
                let _ = app.emit("hive:agentArchived", json!({ "id": id }));
            }
            attribute(app, "kill", id, Map::new());
            Ok(if res.ok {
                format!("Killed {}.", name)
            } else {
                format!(
                    "Couldn't kill {}: {}.",
                    name,
                    res.error.unwrap_or_else(|| "unknown".into())
                )
            })
        }
        CommitSpec::Pause { id, name } => {
            state(app).control.lock().unwrap().pause(id, true);
            attribute(app, "pause", id, Map::new());
            Ok(format!("Paused {}.", name))
        }
        CommitSpec::Halt { id, name } => {
            state(app).control.lock().unwrap().halt(id);
            attribute(app, "halt", id, Map::new());
            Ok(format!("Halted {}.", name))
        }
        CommitSpec::ClearContext { id, name } => {
            let provider = {
                let (_, agents) = registry_doc(app);
                agents
                    .iter()
                    .find(|e| &e.id == id)
                    .map(|e| e.provider.clone())
            };
            let provider = infer_provider(provider.as_deref());
            let Some(cmd) = clear_command(&provider) else {
                return Ok(format!(
                    "{} runs on {}, which has no context-clear command I can type. Clear it from their terminal.",
                    name, provider
                ));
            };
            // Upstream enqueues through the renderer queue — emit the event the
            // queue listens on so delivery inherits every safety gate.
            let _ = app.emit("realtime:enqueue", json!({ "agentId": id, "text": cmd }));
            attribute(app, "clear_context", id, Map::new());
            Ok(format!(
                "Queued a context clear for {} — it lands the moment they're idle.",
                name
            ))
        }
        CommitSpec::Archive { id, name } => {
            let res = crate::hive_admin::set_archived(&state(app).hive, id, true);
            if res.is_ok() {
                let _ = app.emit("hive:agentArchived", json!({ "id": id }));
            }
            attribute(app, "archive", id, Map::new());
            Ok(match res {
                Result::Ok(_) => format!(
                    "Archived {} — off the floor, history kept. Say unarchive to bring them back.",
                    name
                ),
                Result::Err(e) => format!("Couldn't archive {}: {}.", name, e),
            })
        }
        CommitSpec::Spawn { spec, label } => {
            let args = SpawnArgs {
                provider: spec.provider.clone(),
                name: spec.name.clone(),
                role: spec.role.clone(),
                cwd: spec.cwd.clone(),
                command: spec.command.clone(),
                seed_prompt: None,
                isolate: false,
                is_god: false,
                is_assistant: false,
                capabilities: Vec::new(),
                no_auto_install: false,
                mcp_enable: Vec::new(),
                mcp_disable: Vec::new(),
                model: None,
                cols: 120,
                rows: 30,
            };
            let res = commands::spawn_inner(
                app.clone(),
                args,
                commands::SpawnPlan {
                    // Voice hires keep the collapsed upstream namespace —
                    // spec.id is BOTH the pty id and the agent id (the card
                    // broadcast below keys ptyId on it).
                    pty_id: Some(spec.id.clone()),
                    agent_id: Some(spec.id.clone()),
                    resume: None,
                    require_resume: false,
                    // MAIN-initiated: nobody consumes seedPrompt — the backend
                    // types the hive-protocol seed itself.
                    auto_type_seed: true,
                },
            );
            // Main-initiated spawn — the floor adds the card from this
            // descriptor since it didn't initiate the hire (upstream
            // 'hive:agentSpawned' broadcast).
            if res.is_ok() {
                let _ = app.emit(
                    "hive:agentSpawned",
                    json!({
                        "id": spec.id, "name": spec.name, "provider": spec.provider,
                        "cwd": spec.cwd, "command": spec.command, "role": spec.role,
                    }),
                );
            }
            let mut extra = Map::new();
            extra.insert("provider".into(), json!(spec.provider));
            if let Some(r) = &spec.role {
                extra.insert("role".into(), json!(r));
            }
            attribute(app, "spawn", &spec.id, extra);
            Ok(match res {
                Result::Ok(_) => format!("Hired {}.", label),
                Result::Err(e) => format!("Couldn't hire {}: {}.", label, e),
            })
        }
        CommitSpec::EditSchedule { mission, action } => {
            let all = {
                let st = state(app);
                let cfg = st.config.lock().unwrap();
                cfg.missions.clone()
            };
            let next: Vec<ScheduledMission> = if action == "delete" {
                all.into_iter().filter(|m| m.id != mission.id).collect()
            } else {
                all.into_iter()
                    .map(|m| {
                        if m.id == mission.id {
                            let mut m = m;
                            m.enabled = action == "enable";
                            m
                        } else {
                            m
                        }
                    })
                    .collect()
            };
            commands::missions_save(app.clone(), next).map_err(|e| anyhow::anyhow!(e))?;
            let mut extra = Map::new();
            extra.insert("action".into(), json!(action));
            attribute(app, "edit_schedule", &mission.id, extra);
            Ok(format!(
                "{} the \"{}\" schedule.",
                match action.as_str() {
                    "delete" => "Deleted",
                    "enable" => "Enabled",
                    _ => "Disabled",
                },
                mission.label
            ))
        }
        CommitSpec::CreateSchedule { mission, minutes } => {
            let mut all = {
                let st = state(app);
                let cfg = st.config.lock().unwrap();
                cfg.missions.clone()
            };
            all.push(mission.clone());
            commands::missions_save(app.clone(), all).map_err(|e| anyhow::anyhow!(e))?;
            let mut extra = Map::new();
            extra.insert(
                "title".into(),
                json!(mission.label.chars().take(120).collect::<String>()),
            );
            attribute(app, "create_schedule", &mission.id, extra);
            Ok(format!(
                "Created the \"{}\" schedule — every {} minutes to {}.",
                mission.label, minutes, mission.to
            ))
        }
        CommitSpec::UpdateSetting { key, value } => {
            let old = state(app)
                .config_store
                .raw_value()
                .get(key)
                .cloned()
                .unwrap_or(Value::Null);
            let describe = |v: &Value| -> String {
                match v {
                    Value::Bool(b) => {
                        if *b {
                            "on".into()
                        } else {
                            "off".into()
                        }
                    }
                    Value::Null => "unset".into(),
                    _ => v.to_string().trim_matches('"').to_string(),
                }
            };
            let mut patch = Map::new();
            patch.insert(key.clone(), value.clone());
            let cfg = state(app).config_store.patch_raw(&patch)?;
            *state(app).config.lock().unwrap() = cfg;
            let _ = app.emit("config:changed", json!({}));
            let mut extra = Map::new();
            extra.insert(
                "action".into(),
                json!(format!("{} → {}", describe(&old), describe(value))),
            );
            attribute(app, "update_setting", key, extra);
            Ok(format!(
                "Done — {} is now {} (was {}).",
                key,
                describe(value),
                describe(&old)
            ))
        }
    }
}

fn run_action(app: &AppHandle, verb: &str, a: &Map<String, Value>) -> Result<ActionResult> {
    if !hive_enabled(app) {
        return Ok(err(
            "The hive is not configured, so I can't take that action.",
        ));
    }
    let Some(spec) = verb_spec(verb) else {
        return Ok(err(format!("I don't have an action called \"{}\".", verb)));
    };
    // Any new proposal supersedes a stale pending.
    *PENDING.lock().unwrap() = None;
    if !spec.destructive {
        return match verb {
            "ping" => exec_ping(app, a),
            "dispatch" => exec_dispatch(app, a),
            "steer" => exec_steer(app, a),
            "create_task" => exec_create_task(app, a),
            "assign_task" => exec_assign_task(app, a),
            "update_task" => exec_update_task(app, a),
            "resume" => exec_resume(app, a),
            "auto_delivery" => exec_auto_delivery(app, a),
            "gate_tool" => exec_gate_tool(app, a),
            "delete_task" => exec_delete_task(app, a),
            "unarchive" => exec_unarchive(app, a),
            _ => Ok(err(format!("I don't know how to {}.", verb))),
        };
    }
    propose_destructive(app, verb, a, &spec)
}

// ─── commands (upstream's ipcMain.handle triple) ───────────────────────────

#[tauri::command(async)]
pub async fn realtime_action(
    app: AppHandle,
    payload: Option<Value>,
) -> Result<ActionResult, String> {
    let p = payload.unwrap_or_else(|| json!({}));
    let obj = p.as_object().cloned().unwrap_or_default();
    let verb = norm(&strv(&obj, "verb")).replace(' ', "_");
    match run_action(&app, &verb, &obj) {
        Result::Ok(res) => Ok(res),
        Result::Err(e) => {
            log_action_failure(&app, "realtime:action", &verb, &e.to_string());
            Ok(err(format!("That action failed: {}.", e)))
        }
    }
}

#[tauri::command(async)]
pub async fn realtime_action_confirm(
    app: AppHandle,
    payload: Option<Value>,
) -> Result<ActionResult, String> {
    let p = payload.unwrap_or_else(|| json!({}));
    let obj = p.as_object().cloned().unwrap_or_default();
    let cur = pending_fresh();
    let Some(cur) = cur else {
        return Ok(err("There's nothing waiting to confirm."));
    };
    let phrase = first_str(&obj, &["phrase", "confirm", "text"]);
    if !confirm_accepted(&phrase, &cur.confirm_word) {
        return Ok(err(format!(
            "I won't {} {} on that — for safety I need you to say \"confirm\" or \"{}\", not just yes. Say it clearly, or say cancel.",
            cur.verb, cur.target_label, cur.confirm_word
        )));
    }
    // Consume before running so a failure can't be re-confirmed.
    *PENDING.lock().unwrap() = None;
    match commit_pending(&app, &cur.commit) {
        Result::Ok(spoken) => Ok(ok(spoken)),
        Result::Err(e) => {
            log_action_failure(&app, "realtime:action:confirm", &cur.verb, &e.to_string());
            Ok(err(format!("That action failed: {}.", e)))
        }
    }
}

#[tauri::command(async)]
pub async fn realtime_action_cancel() -> Result<ActionResult, String> {
    let had = pending_fresh();
    *PENDING.lock().unwrap() = None;
    Ok(ok(had
        .map(|p| format!("Cancelled the {}.", p.verb))
        .unwrap_or_else(|| "Nothing to cancel.".into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_affirmations_never_confirm() {
        for phrase in ["yes", "Yeah!", "ok", "sure", "go ahead", "please", "right"] {
            assert!(
                !confirm_accepted(phrase, "kill"),
                "phrase {phrase:?} must NOT confirm"
            );
        }
    }

    #[test]
    fn confirm_word_or_literal_confirm_accepts() {
        assert!(confirm_accepted("confirm", "kill"));
        assert!(confirm_accepted("yes, confirm it", "kill"));
        assert!(confirm_accepted("kill it", "kill"));
        assert!(confirm_accepted("schedule", "schedule"));
        assert!(confirm_accepted("confirmed", "spawn"));
        // The WRONG verb word doesn't count.
        assert!(!confirm_accepted("kill", "pause"));
        assert!(!confirm_accepted("go", "kill"));
        // 'confirmed' contains 'confirm' as a token prefix — accepted.
        assert!(confirm_accepted("i confirm the kill", "kill"));
    }

    #[test]
    fn mass_targets_rejected() {
        for t in [
            "all",
            "everyone",
            "all agents",
            "the whole office",
            "every agent",
        ] {
            assert!(is_mass_target(t), "{t:?} should read as mass");
        }
        assert!(!is_mass_target("oscar"));
        assert!(!is_mass_target("all right"));
    }

    #[test]
    fn score_card_tiers() {
        let toks = toks_match("message visibility");
        // exact title/id (score_card takes the ref pre-normalized)
        assert_eq!(
            score_card(
                &norm_match("message-visibility"),
                &toks_match("message-visibility"),
                "x",
                "message-visibility"
            ),
            1.0
        );
        // every spoken word present
        assert_eq!(
            score_card(
                &norm_match("message visibility"),
                &toks,
                "The message visibility task",
                "t1"
            ),
            0.85
        );
        // truncation
        assert_eq!(
            score_card(
                &norm_match("message vis"),
                &toks_match("message vis"),
                "message visibility overhaul",
                "t1"
            ),
            0.92
        );
        // unrelated
        assert!(
            score_card(
                &norm_match("unrelated words"),
                &toks_match("unrelated words"),
                "message visibility",
                "t1"
            ) < 0.45
        );
    }

    #[test]
    fn setting_policy_gates() {
        assert!(setting_policy("autoMode").unwrap().confirm);
        assert!(!setting_policy("notifications").unwrap().confirm);
        assert!(setting_policy("harnessHome").is_none());
        assert!(setting_policy("openaiApiKey").is_none());
        assert!(setting_policy("officeTheme").unwrap().values.is_some());
    }

    #[test]
    fn provider_clear_table() {
        assert_eq!(clear_command("claude").as_deref(), Some("/clear"));
        assert_eq!(clear_command("grok").as_deref(), Some("/new"));
        assert_eq!(clear_command("opencode").as_deref(), Some("/new"));
        assert_eq!(clear_command("copilot"), None);
        assert_eq!(clear_command("crush"), None);
        assert_eq!(clear_command("custom"), None);
    }
}
