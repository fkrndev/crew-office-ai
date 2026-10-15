//! Spawn-time policy ported from upstream `spawnAgentCore` (index.ts) and
//! `config.ts`, kept as small pure helpers so `commands::spawn_inner_impl`
//! stays readable and each rule is unit-testable without a PTY:
//!
//!   * per-engine resume (preset `resumeFlag` / `resumeSubcommand`) and the
//!     Codex "which agent home owns this session" lookup;
//!   * Claude guardrails — auto-mode flag, default/role model, Remote Control
//!     session-name prefix, `--max-turns`;
//!   * per-engine env (non-interactive suppression, COLORFGBG, OpenCode's
//!     `OPENCODE_CONFIG_CONTENT`, least-privilege BYOK scoping);
//!   * Claude's first-run permission pre-accept (`ensureClaudePermissionsAccepted`);
//!   * the bundled knowledge-graph CLI written to `<harness>/bin/`.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

// ─── resume ─────────────────────────────────────────────────────────────────

/// A preset's resume mechanism (agentProvider.ts `resumeFlag` /
/// `resumeSubcommand`): a flag taking the session id, or a leading subcommand
/// (`codex resume <id>`). Neither = the engine can't resume, spawn fresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResumeSpec {
    pub flag: Option<&'static str>,
    pub sub: Option<&'static str>,
}

pub fn resume_spec(provider: &str) -> ResumeSpec {
    let (flag, sub) = match provider {
        "claude" | "grok" | "gemini" | "copilot" | "cursor" | "hermes" => (Some("--resume"), None),
        "antigravity" => (Some("--conversation"), None),
        "crush" | "pi" => (Some("--session"), None),
        "codex" => (None, Some("resume")),
        // kimi, qwen, opencode, openclaw, custom: no resume support.
        _ => (None, None),
    };
    ResumeSpec { flag, sub }
}

/// Does `hay` contain `needle` as a byte substring? (state_5.sqlite probing.)
fn contains_bytes(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    let first = needle[0];
    let last_start = hay.len() - needle.len();
    let mut i = 0;
    while i <= last_start {
        match hay[i..=last_start].iter().position(|b| *b == first) {
            None => return false,
            Some(p) => {
                i += p;
                if &hay[i..i + needle.len()] == needle {
                    return true;
                }
                i += 1;
            }
        }
    }
    false
}

/// Does any `*<sid>*.jsonl` rollout exist under `sessions` (any depth)?
fn has_rollout(sessions: &Path, sid: &str) -> bool {
    let mut stack = vec![sessions.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            // metadata() follows the junction/symlink the exposed sessions dir
            // may be (expose_codex_data).
            let Ok(md) = std::fs::metadata(&p) else { continue };
            if md.is_dir() {
                stack.push(p);
            } else if md.is_file() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.ends_with(".jsonl") && name.contains(sid) {
                    return true;
                }
            }
        }
    }
    false
}

/// Codex keeps rollouts under a PER-AGENT CODEX_HOME
/// (`<agents>/<id>/.codex/sessions/...`) and indexes them in `state_5.sqlite`,
/// so a freshly added agent's empty home can't `codex resume <sid>` — it
/// silently opens a blank session. Find the sibling agent home that OWNS the
/// rollout, preferring one whose state DB (or its WAL) actually indexes the id
/// (upstream findCodexHomeForSession). The caller points CODEX_HOME at it.
pub fn find_codex_home_for_session(session_id: &str, agents_root: &Path) -> Option<PathBuf> {
    let ok_shape = session_id.len() >= 16
        && session_id.chars().next().is_some_and(|c| c.is_ascii_hexdigit())
        && session_id.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    if !ok_shape {
        return None;
    }
    let mut fallback: Option<PathBuf> = None;
    for e in std::fs::read_dir(agents_root).ok()?.flatten() {
        if !e.path().is_dir() {
            continue;
        }
        let home = e.path().join(".codex");
        let sessions = home.join("sessions");
        if !sessions.exists() || !has_rollout(&sessions, session_id) {
            continue;
        }
        let indexed = ["state_5.sqlite", "state_5.sqlite-wal"].iter().any(|db| {
            std::fs::read(home.join(db))
                .map(|b| contains_bytes(&b, session_id.as_bytes()))
                .unwrap_or(false)
        });
        if indexed {
            return Some(home);
        }
        if fallback.is_none() {
            fallback = Some(home);
        }
    }
    fallback
}

/// The resume argv for a non-Claude engine, or None to start fresh.
/// `tokens` = the command's own argv (minus the program): a flag already
/// present means the caller stated its own resume and nothing is added.
/// Flag form → `[flag, sid]` (appended); subcommand form → `[sub, sid]`,
/// which the caller must place AFTER the command's own flags but BEFORE its
/// first positional (see `first_positional_index` / `splice_before_positional`):
/// Codex reads `[OPTIONS] resume [SESSION_ID] [PROMPT]`, so a prompt ahead of
/// the subcommand would be taken as the prompt and `resume` as part of it.
pub fn resume_args(provider: &str, sid: &str, tokens: &[String]) -> Option<Vec<String>> {
    // `proxy-…` is the synthetic id the proxy-tier bridges stamp on their
    // payloads (no real CLI session behind it) — never a resume target.
    if sid.starts_with("proxy-") {
        return None;
    }
    let spec = resume_spec(provider);
    if let Some(flag) = spec.flag {
        if tokens.iter().any(|t| t == flag) {
            return None;
        }
        return Some(vec![flag.to_string(), sid.to_string()]);
    }
    if let Some(sub) = spec.sub {
        if tokens.get(first_positional_index(tokens)).map(String::as_str) == Some(sub) {
            return None;
        }
        return Some(vec![sub.to_string(), sid.to_string()]);
    }
    None
}

/// Codex flags that take the NEXT token as their value (`--flag=value` carries
/// it inline and is not listed). Needed to tell a flag's value from the first
/// positional (the prompt / a subcommand). `-i/--image` is variadic in Codex;
/// only its first value is skipped here.
const CODEX_VALUE_FLAGS: &[&str] = &[
    "-c", "--config", "--enable", "--disable", "--remote", "--remote-auth-token-env", "-i",
    "--image", "-m", "--model", "--local-provider", "-p", "--profile", "-s", "--sandbox", "-a",
    "--ask-for-approval", "-C", "--cd", "--add-dir", "--color", "-o", "--output-last-message",
    "--output-schema",
];

/// Index of the first positional token of a Codex argv (the prompt or a
/// subcommand), skipping flags and their values; `tokens.len()` when there is
/// none. A bare `--` ends the flags, so it counts as the boundary itself.
pub fn first_positional_index(tokens: &[String]) -> usize {
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i].as_str();
        if t == "--" || !t.starts_with('-') || t == "-" {
            return i;
        }
        i += if !t.contains('=') && CODEX_VALUE_FLAGS.contains(&t) { 2 } else { 1 };
    }
    tokens.len()
}

/// `cmd_toks` with `insert` spliced in just before its first positional (or at
/// the end when it has none — then it is a plain append).
pub fn splice_before_positional(mut cmd_toks: Vec<String>, insert: &[String]) -> Vec<String> {
    let at = first_positional_index(&cmd_toks);
    cmd_toks.splice(at..at, insert.iter().cloned());
    cmd_toks
}

// ─── Claude guardrails ──────────────────────────────────────────────────────

pub const MODEL_GOD: &str = "claude-opus-4-8";
pub const MODEL_WORKER: &str = "claude-sonnet-4-6";
pub const MODEL_HELPER: &str = "claude-haiku-4-5-20251001";
/// config.ts DEFAULTS.defaultModel / godModel — what readConfig deep-fills.
pub const DEFAULT_MODEL: &str = "claude-fable-5";
pub const DEFAULT_GOD_MODEL: &str = MODEL_GOD;

/// modelForRole (config.ts): Opus for the god, Haiku for narrow helpers
/// (triage / routing / verification / formatting), Sonnet for everything else.
/// `god_model`: the configured godModel (None = default).
pub fn model_for_role(
    is_god: bool,
    role: &str,
    capabilities: &[String],
    god_model: Option<&str>,
) -> String {
    if is_god {
        return god_model
            .filter(|m| !m.trim().is_empty())
            .unwrap_or(DEFAULT_GOD_MODEL)
            .to_string();
    }
    let hay = format!("{} {}", role, capabilities.join(" ")).to_lowercase();
    // /\b(triage|rout|verif|lint|format|summar|classif|label)/
    let helper = ["triage", "rout", "verif", "lint", "format", "summar", "classif", "label"];
    let hit = helper.iter().any(|w| {
        hay.match_indices(w).any(|(i, _)| {
            i == 0 || !hay[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_')
        })
    });
    (if hit { MODEL_HELPER } else { MODEL_WORKER }).to_string()
}

/// True when argv already states a permission posture for Claude (token
/// match, not substring). `--dangerously-skip-permissions` counts too — adding
/// `--permission-mode` on top of it would only duplicate the stance.
pub fn has_claude_auto_stance(tokens: &[String]) -> bool {
    tokens.iter().any(|t| {
        t == "--permission-mode"
            || t.starts_with("--permission-mode=")
            || t == "--dangerously-skip-permissions"
    })
}

/// Slug the friendly name (or id) into one safe token for the Remote Control
/// session-name prefix: `[^A-Za-z0-9._-]+` → `-`, trimmed of leading/trailing `-`.
pub fn rc_label(name: &str, id: &str) -> String {
    let src = if name.trim().is_empty() { id } else { name }.trim();
    let mut out = String::new();
    let mut last_dash = false;
    for c in src.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Inputs of `claude_extra_args`.
pub struct ClaudeSpawnCtx<'a> {
    /// The command's own argv (minus the program) + anything already added.
    pub tokens: &'a [String],
    pub auto_mode: bool,
    /// The raw config.json object (`defaultModel`, `godModel`, `godProvider`,
    /// `maxTurns`) — read untyped so unmodelled keys count.
    pub cfg: &'a Value,
    pub is_god: bool,
    pub role: &'a str,
    pub capabilities: &'a [String],
    /// A model the caller picked outside the command string (SpawnArgs.model).
    pub explicit_model: Option<&'a str>,
    pub name: &'a str,
    pub id: &'a str,
}

fn has_flag(tokens: &[String], flag: &str) -> bool {
    tokens
        .iter()
        .any(|t| t == flag || t.strip_prefix(flag).is_some_and(|r| r.starts_with('=')))
}

/// The Claude-only spawn flags spawnAgentCore adds in main (all additive; an
/// explicit choice already in `tokens` always wins):
///   * `--permission-mode bypassPermissions` when floor autoMode is on — a
///     main-only spawn (ephemeral worker, voice hire) never went through the
///     renderer's buildSpawnCommand, so it carried no posture and deadlocked
///     on an approval nobody could grant;
///   * `--model`: explicit per-agent > (god: godModel; others: global
///     defaultModel) > role tier;
///   * `--remote-control-session-name-prefix <agent>` so RC sessions are
///     identifiable instead of carrying the hostname;
///   * `--max-turns <n>` from settings.maxTurns.
pub fn claude_extra_args(ctx: &ClaudeSpawnCtx) -> Vec<String> {
    let mut add: Vec<String> = Vec::new();
    let mut seen: Vec<String> = ctx.tokens.to_vec();
    if ctx.auto_mode && !has_claude_auto_stance(&seen) {
        add.push("--permission-mode".into());
        add.push("bypassPermissions".into());
    }
    if !has_flag(&seen, "--model") {
        let s = |k: &str| {
            ctx.cfg
                .get(k)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
        };
        let god_provider = s("godProvider").unwrap_or("claude");
        let model = if let Some(m) = ctx.explicit_model.filter(|m| !m.trim().is_empty()) {
            Some(m.to_string())
        } else if ctx.is_god {
            // The god has its own engine config; a non-Claude godProvider's
            // model id must never reach a claude spawn.
            (god_provider == "claude")
                .then(|| model_for_role(true, ctx.role, ctx.capabilities, s("godModel")))
        } else {
            Some(
                s("defaultModel")
                    .map(str::to_string)
                    .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            )
        };
        if let Some(m) = model {
            add.push("--model".into());
            add.push(m);
        }
    }
    seen.extend(add.iter().cloned());
    if !has_flag(&seen, "--remote-control-session-name-prefix") {
        let label = rc_label(ctx.name, ctx.id);
        if !label.is_empty() {
            add.push("--remote-control-session-name-prefix".into());
            add.push(label);
        }
    }
    if !has_flag(&seen, "--max-turns") {
        if let Some(n) = ctx
            .cfg
            .get("maxTurns")
            .and_then(|v| v.as_f64())
            .filter(|n| *n > 0.0)
        {
            add.push("--max-turns".into());
            add.push(format!("{}", n as u64));
        }
    }
    add
}

// ─── per-engine env ─────────────────────────────────────────────────────────

/// Preset `nonInteractiveEnv`: first-run prompt / version-check / telemetry
/// suppression (Codex's directory-trust gate, pi's chatter).
pub fn non_interactive_env(provider: &str) -> Vec<(&'static str, &'static str)> {
    match provider {
        "codex" => vec![("CODEX_NON_INTERACTIVE", "1")],
        "pi" => vec![("PI_SKIP_VERSION_CHECK", "1"), ("PI_TELEMETRY", "0")],
        _ => Vec::new(),
    }
}

/// COLORFGBG ("fg;bg", the rxvt/konsole convention lipgloss/termenv fall back
/// to) so Crush/OpenCode pick a readable theme on the app's terminal theme.
pub fn colorfgbg(theme: &str) -> &'static str {
    if theme == "dark" { "15;0" } else { "0;15" }
}

/// Engines whose BYOK keys / permission env upstream materialises at spawn.
pub fn is_byok_engine(provider: &str) -> bool {
    matches!(provider, "opencode" | "crush" | "pi" | "qwen")
}

/// The `--model` slug from a command's argv (`--model X` or `--model=X`).
pub fn model_slug(tokens: &[String], explicit: Option<&str>) -> String {
    for (i, t) in tokens.iter().enumerate() {
        if t == "--model" {
            if let Some(v) = tokens.get(i + 1) {
                return v.clone();
            }
        } else if let Some(v) = t.strip_prefix("--model=") {
            return v.to_string();
        }
    }
    explicit.unwrap_or("").to_string()
}

/// `provider/model` prefix → the one BYOK backend whose key that model needs
/// (least privilege: inject only it). None = unknown prefix/no prefix → the
/// caller falls back to every stored key, as upstream does.
pub fn byok_backend_for_model(slug: &str) -> Option<&'static str> {
    let (prefix, _) = slug.split_once('/')?;
    Some(match prefix.to_lowercase().as_str() {
        "anthropic" => "anthropic",
        "openai" => "openai",
        "google" | "gemini" => "google",
        "groq" => "groq",
        "openrouter" => "openrouter",
        "deepseek" => "deepseek",
        _ => return None,
    })
}

/// Should a stored `PROVIDER_KEY_<BACKEND>` secret reach this spawn? When the
/// model names its backend only that key does; otherwise every key does.
pub fn byok_key_allowed(secret_name: &str, scoped: Option<&str>) -> bool {
    let Some(rest) = secret_name.strip_prefix("PROVIDER_KEY_") else {
        return true; // not a BYOK provider key — unaffected
    };
    match scoped {
        Some(b) => rest.eq_ignore_ascii_case(b),
        None => true,
    }
}

/// OPENCODE_CONFIG_CONTENT: autoupdate off, tool permissions `allow` ONLY in
/// floor auto mode, and — with a configured base URL — a `local` provider
/// registering the model id the user actually picked (`local/<id>`, default
/// `local`) so `--model local/llama3` resolves.
pub fn opencode_config_content(auto_mode: bool, base_url: Option<&str>, slug: &str) -> String {
    let mut oc = serde_json::Map::new();
    oc.insert("autoupdate".into(), json!(false));
    if auto_mode {
        oc.insert(
            "permission".into(),
            json!({ "edit": "allow", "bash": "allow", "webfetch": "allow" }),
        );
    }
    if let Some(url) = base_url.map(str::trim).filter(|u| !u.is_empty()) {
        let local_model = slug
            .strip_prefix("local/")
            .filter(|m| !m.is_empty())
            .unwrap_or("local");
        oc.insert(
            "provider".into(),
            json!({
                "local": {
                    "npm": "@ai-sdk/openai-compatible",
                    "name": "Local (self-hosted)",
                    "options": { "baseURL": url },
                    "models": { local_model: { "name": local_model } },
                }
            }),
        );
    }
    Value::Object(oc).to_string()
}

// ─── Claude first-run permission pre-accept ─────────────────────────────────

/// Read `path` as a JSON OBJECT. Ok(None) = missing/empty. Err = present but
/// unparseable or not an object: the file is left alone (never overwritten).
fn read_json_object(path: &Path) -> Result<Option<serde_json::Map<String, Value>>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(m)) => Ok(Some(m)),
        Ok(_) => Err("not a JSON object".into()),
        Err(e) => Err(e.to_string()),
    }
}

fn write_json_atomic(path: &Path, obj: &serde_json::Map<String, Value>) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(obj.clone())).map_err(|e| e.to_string())?;
    crate::hive::atomic_write_file(path, text.as_bytes()).map_err(|e| e.to_string())
}

/// `~/.claude/settings.json` → skipDangerousModePermissionPrompt +
/// skipAutoPermissionPrompt (the bypass-mode warning gate). Written only when
/// not already satisfied; an unparseable file is preserved and skipped.
fn ensure_global_permissions(home: &Path) {
    let p = home.join(".claude").join("settings.json");
    let res = (|| -> Result<(), String> {
        let mut s = read_json_object(&p)?.unwrap_or_default();
        let done = |k: &str| s.get(k) == Some(&Value::Bool(true));
        if done("skipDangerousModePermissionPrompt") && done("skipAutoPermissionPrompt") {
            return Ok(());
        }
        s.insert("skipDangerousModePermissionPrompt".into(), json!(true));
        s.insert("skipAutoPermissionPrompt".into(), json!(true));
        write_json_atomic(&p, &s)
    })();
    if let Err(e) = res {
        log::warn!("[config] Could not safely update Claude config at {}: {e}", p.display());
    }
}

/// `~/.claude.json` → projects[cwd].hasTrustDialogAccepted (the per-folder
/// "do you trust the files in this folder?" dialog). Claude looks the entry up
/// under a FORWARD-slash path (its own lookup walks parents with
/// `startsWith(r + "/")`; Windows entries are keyed "C:/Users/…"), so both
/// spellings are written — the raw one keeps older builds working. On
/// macOS/Linux the two are identical and this is a single key.
fn ensure_project_trust(home: &Path, cwd: &str) {
    let p = home.join(".claude.json");
    let res = (|| -> Result<(), String> {
        let mut c = read_json_object(&p)?.unwrap_or_default();
        let mut keys = vec![cwd.replace('\\', "/")];
        if !keys.contains(&cwd.to_string()) {
            keys.push(cwd.to_string());
        }
        let trusted = |c: &serde_json::Map<String, Value>, k: &str| {
            c.get("projects")
                .and_then(|p| p.get(k))
                .and_then(|e| e.get("hasTrustDialogAccepted"))
                == Some(&Value::Bool(true))
        };
        if keys.iter().all(|k| trusted(&c, k)) {
            return Ok(());
        }
        if !c.get("projects").is_some_and(|v| v.is_object()) {
            c.insert("projects".into(), json!({}));
        }
        let projects = c.get_mut("projects").and_then(|v| v.as_object_mut()).unwrap();
        for k in keys {
            let entry = projects.entry(k).or_insert_with(|| json!({}));
            if !entry.is_object() {
                *entry = json!({});
            }
            entry["hasTrustDialogAccepted"] = json!(true);
        }
        write_json_atomic(&p, &c)
    })();
    if let Err(e) = res {
        log::warn!("[config] Could not safely update Claude config at {}: {e}", p.display());
    }
}

/// The home whose `.claude*` files spawn-time pre-accept may touch.
/// `AGENT_OFFICE_CLAUDE_HOME` overrides it (tests, portable setups); the
/// crate's own unit tests never fall through to the real user home.
pub fn claude_home() -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("AGENT_OFFICE_CLAUDE_HOME").filter(|h| !h.is_empty()) {
        return Some(PathBuf::from(h));
    }
    if cfg!(test) {
        return None;
    }
    dirs::home_dir()
}

/// ensureClaudePermissionsAccepted — idempotently pre-accept Claude Code's
/// first-run prompts so an agent spawned with `--permission-mode
/// bypassPermissions` starts cleanly instead of stalling on a dialog the PTY
/// can't answer (exit code 1). Each file is an independent best-effort
/// boundary: unsafe existing contents are preserved without blocking the
/// other. `home` is injectable so tests never touch the real user files.
pub fn ensure_claude_permissions_accepted(home: &Path, cwd: Option<&str>) {
    if home.as_os_str().is_empty() {
        return;
    }
    ensure_global_permissions(home);
    if let Some(cwd) = cwd.filter(|c| !c.is_empty()) {
        ensure_project_trust(home, cwd);
    }
}

// ─── bundled knowledge-graph CLI ────────────────────────────────────────────

const KG_CLI_JS: &str = include_str!("../resources/kg.cjs");
const KG_CORE_JS: &str = include_str!("../resources/kg-core.cjs");

/// Write the bundled kg CLI + core to `<bin_dir>/kg.cjs` / `kg-core.cjs`
/// (only when their content differs) and return `(cli, core)`. Agents run
/// `node <bin>/kg.cjs search "…"` with KG_ROOT / KG_CORE in their env.
pub fn install_kg_cli(bin_dir: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(bin_dir)?;
    let put = |name: &str, body: &str| -> std::io::Result<PathBuf> {
        let p = bin_dir.join(name);
        if std::fs::read_to_string(&p).map(|t| t != body).unwrap_or(true) {
            std::fs::write(&p, body)?;
        }
        Ok(p)
    };
    Ok((put("kg.cjs", KG_CLI_JS)?, put("kg-core.cjs", KG_CORE_JS)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oa-spawnext-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn toks(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn resume_specs_match_presets() {
        assert_eq!(resume_spec("antigravity").flag, Some("--conversation"));
        assert_eq!(resume_spec("crush").flag, Some("--session"));
        assert_eq!(resume_spec("pi").flag, Some("--session"));
        assert_eq!(resume_spec("codex"), ResumeSpec { flag: None, sub: Some("resume") });
        assert_eq!(resume_spec("grok").flag, Some("--resume"));
        for p in ["kimi", "qwen", "opencode", "openclaw", "custom"] {
            assert_eq!(resume_spec(p), ResumeSpec { flag: None, sub: None }, "{p}");
        }
    }

    #[test]
    fn resume_args_flag_subcommand_and_dedupe() {
        assert_eq!(
            resume_args("antigravity", "c-1", &toks("--yolo")),
            Some(toks("--conversation c-1"))
        );
        assert_eq!(resume_args("crush", "s1", &toks("--yolo")), Some(toks("--session s1")));
        // the id comes straight after the subcommand
        assert_eq!(resume_args("codex", "abc", &toks("-a never")), Some(toks("resume abc")));
        // already resuming / unsupported → nothing
        assert_eq!(resume_args("codex", "abc", &toks("resume zzz")), None);
        assert_eq!(resume_args("codex", "abc", &toks("--model gpt-5 resume zzz")), None);
        assert_eq!(resume_args("grok", "abc", &toks("--resume abc")), None);
        assert_eq!(resume_args("qwen", "abc", &[]), None);
        assert_eq!(resume_args("crush", "proxy-dev-1a2b3c", &[]), None);
    }

    #[test]
    fn first_positional_skips_flags_and_their_values() {
        assert_eq!(first_positional_index(&[]), 0);
        assert_eq!(first_positional_index(&toks("--yolo --search")), 2);
        assert_eq!(first_positional_index(&toks("--model gpt-5 -a never")), 4);
        assert_eq!(first_positional_index(&toks("--model gpt-5 do the thing")), 2);
        // inline value: nothing to skip
        assert_eq!(first_positional_index(&toks("--model=gpt-5 prompt")), 1);
        // a value that looks like a prompt is still the flag's value
        assert_eq!(first_positional_index(&toks("-c k=v -s read-only go")), 4);
        // `--` ends the flags
        assert_eq!(first_positional_index(&toks("--yolo -- -weird")), 1);
    }

    #[test]
    fn codex_resume_goes_before_the_commands_positional_prompt() {
        let post = toks("--dangerously-bypass-hook-trust --add-dir D:/hive");
        let ra = resume_args("codex", "sid1", &toks("--model gpt-5 do the thing")).unwrap();
        assert_eq!(ra, toks("resume sid1"));
        let insert: Vec<String> = ra.into_iter().chain(post.iter().cloned()).collect();
        // the wrong order the old append produced: `--model gpt-5 do the thing resume sid1 ...`
        assert_eq!(
            splice_before_positional(toks("--model gpt-5 do the thing"), &insert),
            toks("--model gpt-5 resume sid1 --dangerously-bypass-hook-trust --add-dir D:/hive do the thing")
        );
        // flag-only command: unchanged behaviour (resume + app args trail the flags)
        assert_eq!(
            splice_before_positional(toks("-a never"), &insert),
            toks("-a never resume sid1 --dangerously-bypass-hook-trust --add-dir D:/hive")
        );
        assert_eq!(
            splice_before_positional(Vec::new(), &insert),
            toks("resume sid1 --dangerously-bypass-hook-trust --add-dir D:/hive")
        );
    }

    #[test]
    fn codex_home_lookup_prefers_the_indexed_owner() {
        let root = tmp("codexhome");
        let sid = "0199aaaa-bbbb-cccc-dddd-eeeeffff0001";
        // agent a: rollout only (no index) — fallback
        let a = root.join("a").join(".codex").join("sessions").join("2026").join("01");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(a.join(format!("rollout-x-{sid}.jsonl")), "{}").unwrap();
        // agent b: rollout AND an index containing the id — preferred
        let b = root.join("b").join(".codex");
        let bs = b.join("sessions").join("2026").join("02");
        std::fs::create_dir_all(&bs).unwrap();
        std::fs::write(bs.join(format!("rollout-y-{sid}.jsonl")), "{}").unwrap();
        std::fs::write(b.join("state_5.sqlite-wal"), format!("xx{sid}yy")).unwrap();
        // agent c: unrelated
        std::fs::create_dir_all(root.join("c").join(".codex").join("sessions")).unwrap();
        assert_eq!(find_codex_home_for_session(sid, &root), Some(b.clone()));
        // only-fallback case
        std::fs::remove_file(b.join("state_5.sqlite-wal")).unwrap();
        let got = find_codex_home_for_session(sid, &root).unwrap();
        assert!(got.ends_with(".codex"));
        // not found / malformed ids
        assert_eq!(find_codex_home_for_session("0199aaaa-bbbb-cccc-dddd-000000000000", &root), None);
        assert_eq!(find_codex_home_for_session("../etc", &root), None);
        assert_eq!(find_codex_home_for_session("short", &root), None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn contains_bytes_basics() {
        assert!(contains_bytes(b"abcabd", b"abd"));
        assert!(!contains_bytes(b"abcab", b"abd"));
        assert!(!contains_bytes(b"", b"a"));
        assert!(contains_bytes(b"a", b"a"));
    }

    #[test]
    fn role_tiers() {
        assert_eq!(model_for_role(true, "anything", &[], None), "claude-opus-4-8");
        assert_eq!(model_for_role(true, "x", &[], Some("claude-x")), "claude-x");
        assert_eq!(model_for_role(false, "Triage bot", &[], None), MODEL_HELPER);
        assert_eq!(model_for_role(false, "Backend dev", &["format".into()], None), MODEL_HELPER);
        // \b anchors the START of the word only: "router" matches "rout", "xlint" does not
        assert_eq!(model_for_role(false, "router", &[], None), MODEL_HELPER);
        assert_eq!(model_for_role(false, "xlint", &[], None), MODEL_WORKER);
        assert_eq!(model_for_role(false, "Backend dev", &[], None), MODEL_WORKER);
    }

    fn ctx<'a>(
        tokens: &'a [String],
        cfg: &'a Value,
        is_god: bool,
        auto: bool,
    ) -> ClaudeSpawnCtx<'a> {
        ClaudeSpawnCtx {
            tokens,
            auto_mode: auto,
            cfg,
            is_god,
            role: "worker",
            capabilities: &[],
            explicit_model: None,
            name: "Dev One",
            id: "dev1",
        }
    }

    #[test]
    fn claude_flags_default_model_wins_over_role_and_rc_prefix() {
        let cfg = json!({ "defaultModel": "claude-sonnet-5", "maxTurns": 25 });
        let a = claude_extra_args(&ctx(&[], &cfg, false, true));
        assert_eq!(
            a,
            toks(
                "--permission-mode bypassPermissions --model claude-sonnet-5 \
                 --remote-control-session-name-prefix Dev-One --max-turns 25"
            )
        );
        // no config at all → the shipped defaults
        let a = claude_extra_args(&ctx(&[], &json!({}), false, false));
        assert_eq!(a[..2], toks("--model claude-fable-5")[..]);
        assert!(!a.contains(&"--max-turns".to_string()));
        assert!(!a.contains(&"--permission-mode".to_string()));
    }

    #[test]
    fn claude_flags_god_uses_god_model_not_default() {
        let cfg = json!({ "defaultModel": "claude-sonnet-5", "godModel": "claude-opus-4-8" });
        let a = claude_extra_args(&ctx(&[], &cfg, true, false));
        assert_eq!(a[..2], toks("--model claude-opus-4-8")[..]);
        // a non-Claude god engine must not leak its model id onto a claude spawn
        let cfg = json!({ "godProvider": "codex", "godModel": "gpt-6-sol" });
        let a = claude_extra_args(&ctx(&[], &cfg, true, false));
        assert!(!a.contains(&"--model".to_string()));
    }

    #[test]
    fn claude_flags_explicit_choices_win() {
        let cfg = json!({ "defaultModel": "claude-sonnet-5", "maxTurns": 9 });
        let t = toks("--model m1 --permission-mode plan --max-turns 3 --remote-control-session-name-prefix x");
        assert!(claude_extra_args(&ctx(&t, &cfg, false, true)).is_empty());
        let t = toks("--model=m1");
        let a = claude_extra_args(&ctx(&t, &json!({}), false, false));
        assert!(!a.contains(&"--model".to_string()));
        // dangerously-skip counts as a stance
        let t = toks("--dangerously-skip-permissions");
        let a = claude_extra_args(&ctx(&t, &json!({}), false, true));
        assert!(!a.contains(&"--permission-mode".to_string()));
        // an explicit per-agent model beats the default
        let cfg2 = json!({ "defaultModel": "d" });
        let mut c = ctx(&[], &cfg2, false, false);
        c.explicit_model = Some("picked");
        assert_eq!(claude_extra_args(&c)[..2], toks("--model picked")[..]);
    }

    #[test]
    fn rc_label_slugs() {
        assert_eq!(rc_label("Dev One", "x"), "Dev-One");
        assert_eq!(rc_label("  ", "worker-1"), "worker-1");
        assert_eq!(rc_label("!!", "x"), "");
        assert_eq!(rc_label("Zoë & Co", "x"), "Zo-Co");
    }

    #[test]
    fn per_engine_env() {
        assert_eq!(non_interactive_env("codex"), vec![("CODEX_NON_INTERACTIVE", "1")]);
        assert_eq!(
            non_interactive_env("pi"),
            vec![("PI_SKIP_VERSION_CHECK", "1"), ("PI_TELEMETRY", "0")]
        );
        assert!(non_interactive_env("claude").is_empty());
        assert_eq!(colorfgbg("dark"), "15;0");
        assert_eq!(colorfgbg("light"), "0;15");
        assert_eq!(colorfgbg("whatever"), "0;15");
    }

    #[test]
    fn byok_scoping_by_model_prefix() {
        assert_eq!(byok_backend_for_model("anthropic/claude-sonnet-4-5"), Some("anthropic"));
        assert_eq!(byok_backend_for_model("Gemini/flash"), Some("google"));
        assert_eq!(byok_backend_for_model("local/llama3"), None);
        assert_eq!(byok_backend_for_model("gpt-4o"), None);
        assert!(byok_key_allowed("PROVIDER_KEY_OPENAI", Some("openai")));
        assert!(!byok_key_allowed("PROVIDER_KEY_ANTHROPIC", Some("openai")));
        assert!(byok_key_allowed("PROVIDER_KEY_ANTHROPIC", None));
        // non-provider secrets are never filtered
        assert!(byok_key_allowed("MY_TOKEN", Some("openai")));
        assert_eq!(model_slug(&toks("--yolo --model openai/gpt-4o"), None), "openai/gpt-4o");
        assert_eq!(model_slug(&toks("--model=a/b"), None), "a/b");
        assert_eq!(model_slug(&[], Some("x/y")), "x/y");
    }

    #[test]
    fn opencode_config_is_gated_on_auto_mode_and_registers_local_model() {
        let v: Value = serde_json::from_str(&opencode_config_content(false, None, "")).unwrap();
        assert_eq!(v, json!({ "autoupdate": false }));
        let v: Value = serde_json::from_str(&opencode_config_content(true, None, "")).unwrap();
        assert_eq!(v["permission"]["bash"], "allow");
        let v: Value = serde_json::from_str(&opencode_config_content(
            false,
            Some("http://localhost:11434/v1"),
            "local/llama3",
        ))
        .unwrap();
        assert_eq!(v["provider"]["local"]["options"]["baseURL"], "http://localhost:11434/v1");
        assert_eq!(v["provider"]["local"]["models"]["llama3"]["name"], "llama3");
        assert!(v.get("permission").is_none());
        let v: Value =
            serde_json::from_str(&opencode_config_content(false, Some("http://h"), "anthropic/x"))
                .unwrap();
        assert!(v["provider"]["local"]["models"]["local"].is_object());
    }

    #[test]
    fn permissions_preaccept_writes_both_files_and_is_idempotent() {
        let home = tmp("perm");
        ensure_claude_permissions_accepted(&home, Some("C:\\Users\\me\\proj"));
        let s: Value =
            serde_json::from_str(&std::fs::read_to_string(home.join(".claude/settings.json")).unwrap())
                .unwrap();
        assert_eq!(s["skipDangerousModePermissionPrompt"], true);
        assert_eq!(s["skipAutoPermissionPrompt"], true);
        let c: Value =
            serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap()).unwrap();
        // both the forward-slash key Claude reads and the raw key
        assert_eq!(c["projects"]["C:/Users/me/proj"]["hasTrustDialogAccepted"], true);
        assert_eq!(c["projects"]["C:\\Users\\me\\proj"]["hasTrustDialogAccepted"], true);
        let before = std::fs::read_to_string(home.join(".claude.json")).unwrap();
        ensure_claude_permissions_accepted(&home, Some("C:\\Users\\me\\proj"));
        assert_eq!(before, std::fs::read_to_string(home.join(".claude.json")).unwrap());
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn permissions_preaccept_preserves_existing_keys_and_malformed_files() {
        let home = tmp("permkeep");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(
            home.join(".claude/settings.json"),
            r#"{"theme":"dark","skipAutoPermissionPrompt":true}"#,
        )
        .unwrap();
        std::fs::write(
            home.join(".claude.json"),
            r#"{"numStartups":7,"projects":{"/p":{"allowedTools":["x"],"hasTrustDialogAccepted":false}}}"#,
        )
        .unwrap();
        ensure_claude_permissions_accepted(&home, Some("/p"));
        let s: Value =
            serde_json::from_str(&std::fs::read_to_string(home.join(".claude/settings.json")).unwrap())
                .unwrap();
        assert_eq!(s["theme"], "dark");
        assert_eq!(s["skipDangerousModePermissionPrompt"], true);
        let c: Value =
            serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap()).unwrap();
        assert_eq!(c["numStartups"], 7);
        assert_eq!(c["projects"]["/p"]["allowedTools"], json!(["x"]));
        assert_eq!(c["projects"]["/p"]["hasTrustDialogAccepted"], true);

        // malformed / non-object files are NEVER overwritten
        let bad = tmp("permbad");
        std::fs::create_dir_all(bad.join(".claude")).unwrap();
        std::fs::write(bad.join(".claude/settings.json"), "{ not json").unwrap();
        std::fs::write(bad.join(".claude.json"), "[1,2]").unwrap();
        ensure_claude_permissions_accepted(&bad, Some("/p"));
        assert_eq!(std::fs::read_to_string(bad.join(".claude/settings.json")).unwrap(), "{ not json");
        assert_eq!(std::fs::read_to_string(bad.join(".claude.json")).unwrap(), "[1,2]");
        // one bad file does not block the other
        std::fs::write(bad.join(".claude.json"), "{").unwrap();
        std::fs::write(bad.join(".claude/settings.json"), "{}").unwrap();
        ensure_claude_permissions_accepted(&bad, Some("/p"));
        assert!(std::fs::read_to_string(bad.join(".claude/settings.json"))
            .unwrap()
            .contains("skipDangerousModePermissionPrompt"));
        assert_eq!(std::fs::read_to_string(bad.join(".claude.json")).unwrap(), "{");
        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&bad).ok();
    }

    #[test]
    fn kg_cli_is_written_and_reads_a_rust_written_store() {
        let root = tmp("kgcli");
        let (cli, core) = install_kg_cli(&root.join("bin")).unwrap();
        assert!(cli.is_file() && core.is_file());
        // idempotent
        let (cli2, _) = install_kg_cli(&root.join("bin")).unwrap();
        assert_eq!(cli, cli2);
        // Store format parity: ingest with kg.rs, query with the JS CLI. Skipped
        // when `node` is not on PATH (CI without Node).
        let store = root.join("store");
        let node_ok = std::process::Command::new("node")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if node_ok {
            let input = crate::kg::IngestInput {
                text: Some("The quarterly zebra migration plan covers all stripes.".into()),
                title: Some("Zebra plan".into()),
                ..Default::default()
            };
            crate::kg::ingest(&store, &input).expect("ingest");
            let out = std::process::Command::new("node")
                .arg(&cli)
                .args(["search", "zebra migration"])
                .env("KG_ROOT", &store)
                .env("KG_CORE", &core)
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(text.contains("Zebra plan"), "kg.cjs output: {text} / {}", String::from_utf8_lossy(&out.stderr));
        }
        std::fs::remove_dir_all(&root).ok();
    }
}
