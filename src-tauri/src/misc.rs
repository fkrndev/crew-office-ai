//! Shared helpers for `cmd_misc.rs` — no `#[tauri::command]` here.
//!
//! Contents: the keychain read seam (commands.rs keeps its `secret_get`
//! private, so the provider-key surface mirrors it here), path plumbing
//! (tilde expansion, the filesystem-root guard behind `app_reset_all`), the
//! Settings hero payload port (main/hero.ts + shared/heroPayload.ts), and the
//! slack reply helper script the prompt rail points at.

use crate::lock_ext::LockExt;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tauri::AppHandle;

/// The OS keychain service name — identical to commands.rs KEYRING_SERVICE
/// (private there; provider_key_* and any secret consumer here share it).
pub const KEYRING_SERVICE: &str = "agent-office";

pub fn err_str(e: anyhow::Error) -> String {
    format!("{e:#}")
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The harness home — the hive root's parent (same resolution as
/// commands::hire_drain_pending / app_info).
pub fn harness_home(app: &AppHandle) -> PathBuf {
    let st = crate::commands::state(app);
    st.hive
        .root
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| st.hive.root.clone())
}

/// Port of upstream fs.ts `expandTilde`: `~`/`~/x`/`~\x` expand against the
/// user home; a value still relative afterwards is returned trimmed and
/// unchanged (callers keep their own not-absolute errors); empty passes
/// through.
/// Upstream `BACKEND_KEY_ENV` (index.ts:399) — translate a keyring entry name
/// like `PROVIDER_KEY_OPENAI` into the env var name(s) the spawned CLI actually
/// reads. Returns None for non-provider secrets (custom env keys pass through
/// unchanged — the user named them literally).
/// Google maps to TWO names: OpenCode/AI-SDK's Google provider reads
/// GOOGLE_GENERATIVE_AI_API_KEY, not GEMINI_API_KEY (upstream Jim NIT #1).
/// DeepSeek's OpenAI-compatible platform reads DEEPSEEK_API_KEY — the backend
/// id "deepseek" pairs with a providerBaseUrls override to api.deepseek.com.
pub fn provider_env_names(keyring_name: &str) -> Option<Vec<&'static str>> {
    let rest = keyring_name.strip_prefix("PROVIDER_KEY_")?;
    let envs: Vec<&'static str> = match rest {
        "ANTHROPIC" => vec!["ANTHROPIC_API_KEY"],
        "OPENAI" => vec!["OPENAI_API_KEY"],
        "GOOGLE" => vec!["GEMINI_API_KEY", "GOOGLE_GENERATIVE_AI_API_KEY"],
        "OPENROUTER" => vec!["OPENROUTER_API_KEY"],
        "GROQ" => vec!["GROQ_API_KEY"],
        "DEEPSEEK" => vec!["DEEPSEEK_API_KEY", "OPENAI_API_KEY"],
        _ => return None,
    };
    Some(envs)
}

/// Settings → AI Engines per-CLI base URL → the `OPENAI_BASE_URL` env pair a
/// spawn should carry. Hermes and OpenClaw both honor it natively on their
/// OpenAI-compatible route — this is the DeepSeek / Ollama / vLLM reroute a
/// user configures in the UI instead of editing CLI config files.
pub fn openai_base_url_env(
    cfg: &crate::config::Config,
    provider: &str,
) -> Option<(String, String)> {
    cfg.provider_base_urls
        .get(provider)
        .map(|u| ("OPENAI_BASE_URL".to_string(), u.clone()))
}

/// `parent[key]` as a mutable object — a missing, null, array or scalar value
/// is replaced by `{}` first, so callers can chain without unwrap panics.
fn obj_child<'a>(
    parent: &'a mut serde_json::Map<String, Value>,
    key: &str,
) -> &'a mut serde_json::Map<String, Value> {
    let v = parent.entry(key.to_string()).or_insert_with(|| json!({}));
    if !v.is_object() {
        *v = json!({});
    }
    v.as_object_mut().expect("just coerced to an object")
}

/// OpenClaw's `tui` subcommand has NO `--model` flag — a model picked in the
/// UI would silently corrupt argv and kill the spawn instantly (the same bug
/// class as hermes' removed `--cli`). The model instead rides a per-agent
/// config seeded under OPENCLAW_CONFIG_PATH: the user's own
/// `~/.openclaw/openclaw.json` is merged when present (auth refs and other
/// keys survive), then `agents.defaults.model.primary` is pinned. A custom
/// base URL also lands as a `models.providers."office-compat"` entry — the
/// openai-completions route the env reroute covers — and the model ref is
/// rewritten to `office-compat/<id>` so it resolves against it. API keys stay
/// env-referenced (`${...}` substitution is a documented config feature), so
/// nothing secret lands in the seeded file.
pub fn openclaw_seed_config(existing: &Value, base_url: Option<&str>, model: Option<&str>) -> Value {
    let mut cfg = if existing.is_object() {
        existing.clone()
    } else {
        json!({})
    };

    let mut model_ref = model.map(str::to_string);
    if let Some(url) = base_url {
        // "${VAR}" substitution is documented config syntax — the key stays
        // in the environment, never in the file. DeepSeek endpoints read the
        // native DEEPSEEK_API_KEY name; everything else falls back to OPENAI.
        let key_env = if url.contains("deepseek.com") {
            "DEEPSEEK_API_KEY"
        } else {
            "OPENAI_API_KEY"
        };
        let model_id = model.and_then(|m| m.rsplit('/').next()).map(str::to_string);
        // Guard: a hand-edited openclaw.json could carry a null/array/scalar
        // at any level — coerce to an object rather than panic the spawn path.
        let root = cfg.as_object_mut().expect("cfg is an object");
        obj_child(obj_child(root, "models"), "providers").insert(
            "office-compat".into(),
            json!({
                "baseUrl": url,
                "apiKey": format!("${{{key_env}}}"),
                "api": "openai-completions",
                "models": model_id
                    .iter()
                    .map(|id| json!({ "id": id, "name": id }))
                    .collect::<Vec<_>>(),
            }),
        );
        if let Some(id) = model_id {
            model_ref = Some(format!("office-compat/{id}"));
        }
    }
    if let Some(m) = model_ref {
        let root = cfg.as_object_mut().expect("cfg is an object");
        let defaults = obj_child(obj_child(root, "agents"), "defaults");
        // `model` accepts a bare "provider/model" STRING in openclaw.json —
        // upgrading a legacy string pin to {primary:…} keeps it valid.
        match defaults.get_mut("model") {
            Some(v) if v.is_object() => {
                v.as_object_mut().unwrap().insert("primary".into(), json!(m));
            }
            _ => {
                defaults.insert("model".into(), json!({ "primary": m }));
            }
        }
    }
    cfg
}

/// Write the merged per-agent OpenClaw config and return its path — the value
/// OPENCLAW_CONFIG_PATH points the spawn at. None when nothing needs seeding.
pub fn seed_openclaw_config(
    agent_dir: &Path,
    base_url: Option<&str>,
    model: Option<&str>,
) -> std::io::Result<Option<PathBuf>> {
    if base_url.is_none() && model.is_none() {
        return Ok(None);
    }
    let user_cfg = dirs::home_dir()
        .map(|h| h.join(".openclaw").join("openclaw.json"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or_else(|| json!({}));
    let merged = openclaw_seed_config(&user_cfg, base_url, model);
    let dir = agent_dir.join("openclaw");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("openclaw.json");
    std::fs::write(&path, serde_json::to_string_pretty(&merged).unwrap_or_default())?;
    Ok(Some(path))
}

/// AGENT_PROVIDER_PRESETS.canReceiveInbox — the backend copy (the preset
/// table lives renderer-side). Only providers that can't take typed inbox
/// nudges return false: the router hands their delivery off to the frontend
/// as a terminal handoff instead of pretending it landed.
pub fn can_receive_inbox(provider: &str) -> bool {
    !matches!(provider, "kimi" | "copilot" | "custom")
}

/// Upstream `bridgeOf(provider)` proxy-tier check: `kind==='proxy'` AND
/// `inboxDelivery==='terminal'` — qwen + crush CAN receive inbox on paper,
/// but only via a synthesized Stop that advances the cursor, so the real mail
/// rides the terminal work-order handoff verbatim (hive.ts 1d).
pub fn inbox_delivery_terminal(provider: &str) -> bool {
    matches!(provider, "qwen" | "crush")
}

// ─── per-provider context commands (shared/providerAutomation.ts) ───────────
//
// The total CONTEXT_COMMANDS table. `null` means "we could not establish a
// command we trust" — a wrong slash command typed into a live terminal is
// worse than no command (at best dead text the model answers, at worst it
// fires some other verb). `focus` is appended only where the provider's
// parser actually reads it (compactTakesFocus).
/// `(compact_verb, clear_verb, compact_takes_focus)`
fn context_commands(provider: &str) -> (Option<&'static str>, Option<&'static str>, bool) {
    match provider {
        "claude" => (Some("/compact"), Some("/clear"), true),
        "codex" => (Some("/compact"), Some("/clear"), false),
        "grok" => (Some("/compact"), Some("/new"), true),
        "kimi" => (Some("/compact"), Some("/clear"), true),
        "antigravity" => (None, Some("/clear"), false),
        "gemini" => (Some("/compress"), Some("/clear"), false),
        "qwen" => (Some("/compress"), Some("/clear"), true),
        "opencode" => (Some("/compact"), Some("/new"), false),
        "pi" => (Some("/compact"), Some("/new"), true),
        // crush/copilot/cursor/custom/hermes/openclaw → NO_CONTEXT_COMMANDS:
        // leave the terminal alone rather than typing an untrusted verb.
        _ => (None, None, false),
    }
}

/// compactionCommandForProvider — the focus message is appended only where
/// the provider's parser reads it. `None` = no trustworthy compact command.
pub fn compact_command(provider: &str, message: &str) -> Option<String> {
    let (compact, _, takes_focus) = context_commands(provider);
    let compact = compact?;
    let focus = message.trim();
    Some(if takes_focus && !focus.is_empty() {
        format!("{compact} {focus}")
    } else {
        compact.to_string()
    })
}

/// clearCommandForProvider — a non-empty message is a VERBATIM override
/// (used by the clear context rule); empty falls back to the table verb.
pub fn clear_command(provider: &str, message: &str) -> Option<String> {
    let m = message.trim();
    if !m.is_empty() {
        return Some(m.to_string());
    }
    let (_, clear, _) = context_commands(provider);
    clear.map(|c| c.to_string())
}

// ─── missing-CLI auto-install ladder (cliInstall.ts) ────────────────────────
//
// Upstream's spawnAgentCore: when the provider's binary is missing it runs a
// bundled installer in an "install PTY" in the same terminal slot, then
// re-runs the spawn on exit-0 (`pty:relaunch:<id>`). The rung ladder is
// native vendor installer first, `npm install -g` second, and when neither
// exists a printed manual hint that intentionally does NOT relaunch (a
// respawn would just die on the still-missing binary).

/// The install commands are trusted + hardcoded upstream — same table here.
/// `(native_posix, native_win32, npm_command)`.
pub fn provider_install(
    provider: &str,
) -> (
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
) {
    match provider {
        "claude" => (
            Some("curl -fsSL https://claude.ai/install.sh | bash"),
            Some("powershell -c irm https://claude.ai/install.ps1 ^| iex"),
            Some("npm install -g @anthropic-ai/claude-code"),
        ),
        "codex" => (None, None, Some("npm install -g @openai/codex")),
        "gemini" => (None, None, Some("npm install -g @google/gemini-cli")),
        "opencode" => (
            Some("curl -fsSL https://opencode.ai/install | bash"),
            Some("choco install opencode -y"),
            Some("npm install -g opencode-ai@latest"),
        ),
        "crush" => (None, None, Some("npm install -g @charmland/crush")),
        "pi" => (
            None,
            None,
            Some("npm install -g --ignore-scripts @earendil-works/pi-coding-agent"),
        ),
        "copilot" => (None, None, Some("npm install -g @github/copilot")),
        "cursor" => (
            Some("curl https://cursor.com/install -fsS | bash"),
            // cmd /c executes these — powershell syntax (iex/irm/|) must ride
            // inside `powershell -c`, and cmd metachars get ^-escaped.
            Some("powershell -NoProfile -c irm https://cursor.com/install?win32=true ^| iex"),
            None,
        ),
        // Hermes — Nous Research's installer. Native Windows works without
        // WSL; the scripts bundle Python+Node+TUI. No npm package. The ps1
        // ends with an interactive `hermes setup` wizard — an install-PTY
        // spawn must NOT park there (a worker agent would wait forever), so
        // the script is fetched to %TEMP% and run -File -NonInteractive; the
        // & chain is plain cmd — no quoting.
        "hermes" => (
            Some("curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash"),
            Some("curl -fsSL https://hermes-agent.nousresearch.com/install.ps1 -o %TEMP%\\hermes-install.ps1 & powershell -NoProfile -ExecutionPolicy Bypass -File %TEMP%\\hermes-install.ps1 -NonInteractive"),
            None,
        ),
        // OpenClaw — the installers end with an interactive `openclaw onboard`
        // wizard unless told otherwise: `--no-onboard` / `-NoOnboard` skip it
        // (an install-PTY spawn has nobody to answer). Same %TEMP%+& shape as
        // hermes so cmd /c carries no quotes. The POSIX URL must be
        // install.sh — bare /install serves the marketing page's HTML.
        "openclaw" => (
            Some("curl -fsSL https://openclaw.ai/install.sh | bash -s -- --no-onboard"),
            Some("curl -fsSL https://openclaw.ai/install.ps1 -o %TEMP%\\openclaw-install.ps1 & powershell -NoProfile -ExecutionPolicy Bypass -File %TEMP%\\openclaw-install.ps1 -NoOnboard"),
            None,
        ),
        // No known installer: grok/kimi/antigravity(agy)/qwen/custom — these
        // either ship inside another product (agy comes with the Antigravity
        // IDE) or have no trusted install spec yet. They spawn and die on
        // their own missing-binary error, same as upstream.
        _ => (None, None, None),
    }
}

/// resolveNodeInstaller (nodeInstall.ts) — the current LTS artifact from
/// nodejs.org with its SHASUMS256 digest. `dist/index.json` → first `lts`
/// entry → `SHASUMS256.txt` for the platform artifact (win-x64 .msi, macOS
/// .pkg, linux .tar.xz). `None` = offline or no artifact — the ladder then
/// falls to the native rung.
const NODE_DIST: &str = "https://nodejs.org/dist";

pub struct NodeInstaller {
    version: String,
    file: String,
    url: String,
    sha256: String,
}

/// Lowest Node major we treat as usable. Below it we offer the install/upgrade;
/// at or above it the user's own toolchain is left completely alone - an
/// existing, working Node is never "upgraded" out from under them.
pub const NODE_FLOOR_MAJOR: u32 = 20;

/// Major version of `v24.19.0` / `24.19.0`, None if unparseable.
pub fn node_major(version: &str) -> Option<u32> {
    let v = version.trim();
    let v = v.strip_prefix('v').unwrap_or(v);
    let (major, _) = v.split_once('.')?;
    major.parse().ok()
}

/// Is the user's own Node good enough to leave alone? Absent/unparseable
/// (None) and anything below the floor route into the install rung.
pub fn node_is_usable(version: Option<&str>) -> bool {
    version
        .and_then(node_major)
        .is_some_and(|m| m >= NODE_FLOOR_MAJOR)
}

/// `node --version` from the binary the user's PATH resolves; None when absent
/// or the probe fails/hangs (5s cap) - both mean "we cannot vouch for this
/// runtime".
pub fn detect_node_version(node_path: &str) -> Option<String> {
    let mut cmd = std::process::Command::new(node_path);
    cmd.arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().ok()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    let out = out.trim().to_string();
    node_major(&out).map(|_| out)
}

/// The installer artifact for a platform/arch (`std::env::consts` spellings:
/// "windows"/"macos"/"linux", "aarch64"/"x86_64"). Windows ARM gets the
/// arm64 .msi (published, though index.json lists none); macOS has the single
/// universal .pkg; Linux only has tarballs. None = unsupported.
pub fn node_artifact_for(version: &str, os: &str, arch: &str) -> Option<String> {
    match os {
        "macos" => Some(format!("node-{version}.pkg")),
        "windows" => {
            let a = if arch == "aarch64" { "arm64" } else { "x64" };
            Some(format!("node-{version}-{a}.msi"))
        }
        "linux" => {
            let a = match arch {
                "aarch64" => "arm64",
                "x86_64" => "x64",
                _ => return None,
            };
            Some(format!("node-{version}-linux-{a}.tar.xz"))
        }
        _ => None,
    }
}

/// One bounded GET for the installer lookup - it runs INSIDE a spawn, so an
/// unreachable nodejs.org must fail fast and drop the ladder to its next rung.
fn node_dist_get(url: &str) -> Option<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(6)))
        .https_only(true)
        .build()
        .into();
    agent.get(url).call().ok()?.body_mut().read_to_string().ok()
}

fn resolve_node_installer() -> Option<NodeInstaller> {
    let index: serde_json::Value =
        serde_json::from_str(&node_dist_get(&format!("{NODE_DIST}/index.json"))?).ok()?;
    let lts = index
        .as_array()?
        .iter()
        .find(|e| e["lts"].is_string() || e["lts"] == serde_json::json!(true))?;
    let version = lts["version"].as_str()?.to_string();
    let file = node_artifact_for(&version, std::env::consts::OS, std::env::consts::ARCH)?;
    let sha_body = node_dist_get(&format!("{NODE_DIST}/{version}/SHASUMS256.txt"))?;
    // "<sha>  <file>" lines — pull our artifact's digest. No digest = we would
    // run an unverified installer: refuse.
    let sha256 = sha_body.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let sha = it.next()?;
        (it.next()? == file).then(|| sha.to_string())
    })?;
    Some(NodeInstaller {
        url: format!("{NODE_DIST}/{version}/{file}"),
        version,
        file,
        sha256,
    })
}

/// buildNodeInstallScript (nodeInstall.ts) — download → digest-verify →
/// install. Windows: certutil + findstr (the only guaranteed tools);
/// msiexec's own UAC prompt is the elevation — never silent. POSIX:
/// shasum on macOS, sha256sum on Linux.
fn node_install_script(n: &NodeInstaller) -> String {
    if cfg!(windows) {
        let f = format!("%TEMP%\\{}", n.file);
        return [
            format!(
                "echo   Downloading Node.js {} ^(official installer^)...",
                n.version
            ),
            format!("curl -fSL {} -o {f}", n.url),
            "if errorlevel 1 exit /b 1".to_string(),
            "echo   Verifying checksum...".to_string(),
            format!(
                "certutil -hashfile {f} SHA256 | findstr /i /c:{} >nul",
                n.sha256
            ),
            "if errorlevel 1 (echo   [x] CHECKSUM MISMATCH - refusing to install ^& exit /b 1)"
                .to_string(),
            "echo   Installing - approve the Windows prompt if it appears...".to_string(),
            format!("msiexec /i {f} /passive /norestart"),
            "if errorlevel 1 exit /b 1".to_string(),
            "set PATH=%ProgramFiles%\\nodejs;%PATH%".to_string(),
        ]
        // cmd /c reads ONE line — `&` chains the steps (same reason the
        // caller's rung text can't carry quotes).
        .join(" & ");
    }
    let verify = if cfg!(target_os = "macos") {
        format!(
            "echo \"{}  $__f\" | shasum -a 256 -c - >/dev/null",
            n.sha256
        )
    } else {
        format!("echo \"{}  $__f\" | sha256sum -c - >/dev/null", n.sha256)
    };
    let install = if cfg!(target_os = "macos") {
        "sudo installer -pkg \"$__f\" -target /".to_string()
    } else {
        "sudo tar -xJf \"$__f\" -C /usr/local --strip-components=1".to_string()
    };
    [
        format!("echo '  Downloading Node.js {} (official installer)...'", n.version),
        "__tmp=$(mktemp -d)".to_string(),
        format!("__f=$__tmp/{}", n.file),
        format!("curl -fSL --progress-bar {} -o \"$__f\" || {{ echo '  [x] Download failed.'; exit 1; }}", n.url),
        "echo '  Verifying checksum...'".to_string(),
        format!("{verify} || {{ echo '  [x] CHECKSUM MISMATCH - refusing to install.'; rm -rf \"$__tmp\"; exit 1; }}"),
        "echo ''".to_string(),
        "echo '  Installing Node.js. Enter your password if prompted -'".to_string(),
        "echo '  the official installer writes outside your home directory.'".to_string(),
        install,
    ].join("\n")
}

/// chooseInstallRung + buildMissingCliScript, in the UPSTREAM rung order —
/// npm first, then install-Node-then-npm, then the self-contained native
/// installer, then the manual hint. `node_probe` is the detected Node
/// installer program being reachable on PATH. Returns `(script, relaunch)` —
/// the manual rung returns `false` (a respawn would die on the same missing
/// binary, so upstream arms no relaunch there).
///
/// The script becomes `cmd /d /s /c <string>` on Windows — cmd consumes ONE
/// line of a /c argument, so statements chain with `&` there; `sh -c` gets
/// real newlines. No double-quotes anywhere: the arg is passed verbatim and
/// a quote would unbalance cmd's parse.
pub fn install_script(provider: &str, bin: &str, npm_available: bool) -> Option<(String, bool)> {
    install_script_with_rung(provider, bin, npm_available).map(|(s, relaunch, _)| (s, relaunch))
}

/// `install_script` plus the rung that was chosen - `npm` | `node-then-npm` |
/// `native` | `manual` (the closed `agent_install_started.rung` enum; the
/// manual rung only prints a hint and is never armed for relaunch).
pub fn install_script_with_rung(
    provider: &str,
    bin: &str,
    npm_available: bool,
) -> Option<(String, bool, &'static str)> {
    let (native_posix, native_win32, npm) = provider_install(provider);
    let native = if cfg!(windows) {
        native_win32
    } else {
        native_posix
    };
    let header = format!("[agent-office] {bin} is not installed - ");
    // One joiner for every rung: `&` chains under cmd, newline under sh.
    let join = |a: String, b: &str| -> String {
        if cfg!(windows) {
            format!("{a} & {b}")
        } else {
            format!("{a}\n{b}")
        }
    };

    // Rung 1: npm present — run the package install directly.
    if let (Some(cmd), true) = (npm, npm_available) {
        return Some((
            join(format!("echo {header}installing it with npm."), cmd),
            true,
            "npm",
        ));
    }

    // Rung 2: npm missing but the official Node installer resolves — install
    // Node first, then the package (node-then-npm). The PATH splice lets the
    // npm step resolve node inside the same shell.
    if let Some(cmd) = npm {
        if let Some(n) = resolve_node_installer() {
            let path_fix = if cfg!(windows) {
                ""
            } else {
                "export PATH=/usr/local/bin:$PATH"
            };
            return Some((
                join(
                    join(
                        format!("echo {header}installing Node.js first, then the package."),
                        &node_install_script(&n),
                    ),
                    &format!("{path_fix}{cmd}"),
                ),
                true,
                "node-then-npm",
            ));
        }
    }

    // Rung 3: the vendor's self-contained installer (no Node needed).
    if let Some(cmd) = native {
        return Some((
            join(
                format!("echo {header}running the official {provider} installer - no Node needed."),
                cmd,
            ),
            true,
            "native",
        ));
    }

    // Rung 4: manual hint — honest output, intentionally no relaunch.
    if native.is_some() || npm.is_some() {
        return Some((
            join(
                format!("echo {header}no bundled installer works on this machine."),
                &format!("echo Install {bin} manually, then respawn this agent."),
            ),
            false,
            "manual",
        ));
    }
    None
}

/// tokenizeCommand (shared/commandLine.ts) — split a command string into
/// argv respecting double/single quotes so a model value with spaces
/// (`--model "Gemini 3.1 Pro (High)"`) stays one token. Quotes are stripped.
/// Shared because BOTH sides split command lines: the renderer's spawn flows
/// and main's god-hired-worker path. A worker whose command line splits
/// differently from the renderer's is exactly the class of silent breakage
/// the spawn-request fix exists to prevent.
pub fn tokenize_command(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = command.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c == '"' {
            chars.next();
            // Inside double quotes the Windows CRT rules apply (what
            // `quote_command_token` emits): 2n backslashes + `"` -> n
            // backslashes and the quote CLOSES; 2n+1 -> n backslashes and a
            // literal `"`. Backslashes not followed by `"` stay literal, so
            // "C:\Program Files\x" is unchanged.
            let mut token = String::new();
            let mut bs = 0usize;
            for ch in chars.by_ref() {
                match ch {
                    '\\' => bs += 1,
                    '"' => {
                        token.extend(std::iter::repeat('\\').take(bs / 2));
                        if bs % 2 == 0 {
                            bs = 0;
                            break;
                        }
                        token.push('"');
                        bs = 0;
                    }
                    other => {
                        token.extend(std::iter::repeat('\\').take(bs));
                        bs = 0;
                        token.push(other);
                    }
                }
            }
            token.extend(std::iter::repeat('\\').take(bs));
            out.push(token);
        } else if c == '\'' {
            chars.next();
            let mut token = String::new();
            while let Some(&ch) = chars.peek() {
                if ch == '\'' {
                    chars.next();
                    break;
                }
                token.push(ch);
                chars.next();
            }
            out.push(token);
        } else {
            let mut token = String::new();
            while let Some(&ch) = chars.peek() {
                if ch.is_whitespace() || ch == '"' || ch == '\'' {
                    break;
                }
                token.push(ch);
                chars.next();
            }
            out.push(token);
        }
    }
    out
}

/// Inverse of [`tokenize_command`] for ONE argv token: bare when plain,
/// otherwise double-quoted with CRT backslash/quote escaping, so
/// `tokenize_command(&quote_command_token(t)) == [t]` for every `t` (embedded
/// quotes, trailing backslashes, empties, JSON blobs).
pub fn quote_command_token(t: &str) -> String {
    if !t.is_empty() && !t.chars().any(|c| c.is_whitespace() || matches!(c, '"' | '\'')) {
        return t.to_string();
    }
    let mut out = String::with_capacity(t.len() + 2);
    out.push('"');
    let mut bs = 0usize;
    for c in t.chars() {
        match c {
            '\\' => bs += 1,
            '"' => {
                out.extend(std::iter::repeat('\\').take(bs * 2 + 1));
                out.push('"');
                bs = 0;
            }
            other => {
                out.extend(std::iter::repeat('\\').take(bs));
                bs = 0;
                out.push(other);
            }
        }
    }
    out.extend(std::iter::repeat('\\').take(bs * 2));
    out.push('"');
    out
}

pub fn expand_tilde(p: &str) -> String {
    let t = p.trim();
    if t.is_empty() {
        return p.to_string();
    }
    let home = || {
        dirs::home_dir()
            .map(|h| h.display().to_string())
            .unwrap_or_default()
    };
    let out = if t == "~" {
        home()
    } else if let Some(rest) = t.strip_prefix("~/").or_else(|| t.strip_prefix("~\\")) {
        format!("{}/{}", home().trim_end_matches(['/', '\\']), rest)
    } else {
        t.to_string()
    };
    out
}

/// True when `p` IS a filesystem/drive root — the `app_reset_all` refuse
/// guard: deleting `hive/` + `roster.json` + `harness.db` is only ever done
/// under the configured harness home, never at a volume root. Works on the
/// canonicalized path (falls back to the lexical one when the dir is gone).
pub fn is_filesystem_root(p: &Path) -> bool {
    let canon = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    // `C:\`, `/`, `\\?\C:\`, a bare drive prefix — none have a file_name and
    // none have a parent above them.
    canon.parent().is_none() || canon.file_name().is_none()
}

/// Read one credential from the OS keychain (mirrors commands.rs secret_get —
/// private there, so the same lookup lives here for the provider-key surface).
pub fn secret_get(key: &str) -> Option<String> {
    keyring::Entry::new(KEYRING_SERVICE, key)
        .ok()
        .and_then(|e| e.get_password().ok())
}

/// Write a credential and VERIFY it landed — Windows Credential Manager is
/// documented to drop or race writes (keyring-rs #163: a set followed by an
/// immediate read on another thread can return NoEntry), and on some VM
/// hosts the whole store silently loses entries between processes. The
/// contract becomes "stored AND retrievable": a lost write errors so the UI
/// says so, instead of registering the key name in secret_env_keys and
/// spawning authless agents that 401 forever.
pub fn secret_set_verified(key: &str, value: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, key)
        .map_err(|e| format!("keychain: {e}"))?;
    entry
        .set_password(value)
        .map_err(|e| format!("keychain: {e}"))?;
    // Read-back MUST go through a FRESH Entry — on VM hosts where CredMan
    // drops generic credentials the writing Entry still returns the value it
    // just set (it answers from the in-flight write), while every other
    // process/Entry sees nothing. Verifying on the same Entry is the
    // half-measure that produced this bug in the first place.
    for _ in 0..5 {
        let back = keyring::Entry::new(KEYRING_SERVICE, key)
            .ok()
            .and_then(|e| e.get_password().ok());
        match back {
            Some(v) if v == value => return Ok(()),
            _ => std::thread::sleep(std::time::Duration::from_millis(120)),
        }
    }
    Err("keychain accepted the write but the key does not read back — the OS \
         credential store is not persisting (Windows Credential Manager on \
         this host). Re-enter the key or report this machine."
        .into())
}

// ─── DPAPI file fallback (upstream's Electron safeStorage) ──────────────────
//
// Windows Credential Manager on some VM hosts drops generic credentials
// between processes — the write "succeeds", the next spawn's read finds
// nothing, and every agent boots authless. When keychain verify fails we
// fall back to a per-user DPAPI-encrypted file at <harness>/secrets/<name> —
// the same protection Electron's safeStorage uses on Windows (user+machine
// bound, survives process restarts, unreadable off this account).

/// `CryptProtectData` — returns the encrypted blob, None when the OS call
/// fails. Caller writes the blob to disk; only this Windows account on this
/// machine can reverse it.
#[cfg(windows)]
fn dpapi_protect(data: &[u8]) -> Option<Vec<u8>> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};
    unsafe {
        let desc: Vec<u16> = std::ffi::OsStr::new("agent-office secret")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let ok = CryptProtectData(
            &mut input,
            desc.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut output,
        );
        if ok == 0 || output.pbData.is_null() {
            return None;
        }
        let blob = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(output.pbData as *mut core::ffi::c_void);
        Some(blob)
    }
}

/// `CryptUnprotectData` on a blob produced by `dpapi_protect`.
#[cfg(windows)]
fn dpapi_unprotect(blob: &[u8]) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};
    unsafe {
        let mut input = CRYPT_INTEGER_BLOB {
            cbData: blob.len() as u32,
            pbData: blob.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let ok = CryptUnprotectData(
            &mut input,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut output,
        );
        if ok == 0 || output.pbData.is_null() {
            return None;
        }
        let plain = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(output.pbData as *mut core::ffi::c_void);
        Some(plain)
    }
}

/// The fallback file for one secret name (names are already env-var-safe).
fn dpapi_secret_path(dir: &std::path::Path, key: &str) -> std::path::PathBuf {
    let safe: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .collect();
    dir.join("secrets").join(format!("{safe}.dpapi"))
}

/// Keychain-first read with the DPAPI-file fallback — a key the CredMan lost
/// between processes is still found when it was persisted to the file store.
pub fn secret_get_persist(key: &str, store_dir: &std::path::Path) -> Option<String> {
    if let Some(v) = secret_get(key) {
        return Some(v);
    }
    #[cfg(windows)]
    {
        let blob = std::fs::read(dpapi_secret_path(store_dir, key)).ok()?;
        let plain = dpapi_unprotect(&blob)?;
        String::from_utf8(plain).ok()
    }
    #[cfg(not(windows))]
    {
        let _ = store_dir;
        None
    }
}

/// Persist a secret: keychain with read-back verification first; on a host
/// where CredMan drops the write, fall back to the DPAPI file — which is
/// what Electron safeStorage did for the same data upstream. Returns which
/// store landed it so callers can report honestly.
pub fn secret_set_persist(
    key: &str,
    value: &str,
    store_dir: &std::path::Path,
) -> Result<&'static str, String> {
    if secret_set_verified(key, value).is_ok() {
        let _ = std::fs::remove_file(dpapi_secret_path(store_dir, key));
        return Ok("keychain");
    }
    #[cfg(windows)]
    {
        let blob = dpapi_protect(value.as_bytes())
            .ok_or_else(|| "DPAPI encrypt failed".to_string())?;
        let path = dpapi_secret_path(store_dir, key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, blob).map_err(|e| e.to_string())?;
        log::warn!("[secrets] keychain unpersisted — {key} stored via DPAPI file");
        return Ok("dpapi-file");
    }
    #[cfg(not(windows))]
    {
        let _ = store_dir;
        Err("credential store did not persist the key".into())
    }
}

/// Remove a secret from BOTH stores.
pub fn secret_delete_persist(key: &str, store_dir: &std::path::Path) {
    if let Ok(e) = keyring::Entry::new(KEYRING_SERVICE, key) {
        let _ = e.delete_credential();
    }
    let _ = std::fs::remove_file(dpapi_secret_path(store_dir, key));
}

/// Last-resort OS opener — `explorer.exe` / `rundll32` on Windows (never
/// `cmd /c start`: cmd.exe re-parses `&`, `^`, `%` in a URL or folder name as
/// commands), `open` on macOS, `xdg-open` elsewhere. Used when the opener
/// plugin fails, and as the directory opener for `fs_reveal_path`.
pub fn os_open(target: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        if target.chars().any(|c| c.is_control()) {
            return Err("invalid target".into());
        }
        let lower = target.to_ascii_lowercase();
        let mut cmd = if lower.starts_with("http://") || lower.starts_with("https://") {
            let mut c = std::process::Command::new("rundll32.exe");
            c.args(["url.dll,FileProtocolHandler", target]);
            c
        } else {
            let mut c = std::process::Command::new("explorer.exe");
            c.arg(target);
            c
        };
        cmd.spawn().map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(target)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(target)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 256-bit random hex (two UUIDs — the same entropy upstream's
/// randomBytes(32).toString('hex') produces).
pub fn random_hex32() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Best-effort mime for the fs_read_binary surface — the same extension table
/// fs_ops uses for image previews; anything else is octet-stream.
pub fn mime_for(p: &Path) -> &'static str {
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "tif" | "tiff" => "image/tiff",
        "heic" => "image/heic",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

// ─── Settings hero payload (main/hero.ts + shared/heroPayload.ts) ───────────

/// The remote payload URL — docs/hero.json on this repo's main branch. The file
/// is optional: a 404 / offline / malformed body is a normal state and
/// `load_hero` degrades to the built-in default.
pub const HERO_URL: &str =
    "https://raw.githubusercontent.com/fkrndev/crew-office-ai/main/docs/hero.json";
/// Plan copy changes on a human timescale — 6h TTL (upstream TTL_MS).
pub const HERO_TTL_MS: u64 = 6 * 60 * 60 * 1000;

const HERO_PLAN_BLURB: &str = "Every agent runs on your machine, in your folders, under your own keys. No seat limit, nothing metered.";

/// The payload compiled into the binary — also the fallback whenever fetch or
/// parse fails, so the card never renders empty and never waits on the
/// network (upstream DEFAULT_HERO).
pub fn default_hero() -> Value {
    json!({
        "plan": { "label": "Local", "blurb": HERO_PLAN_BLURB },
        "sponsor": null,
        "notice": null
    })
}

const MAX_LABEL: usize = 24;
const MAX_BLURB: usize = 240;
const MAX_NAME: usize = 48;
const MAX_NOTICE: usize = 160;
const MAX_URL: usize = 300;

/// str() from heroPayload.ts — non-string/blank → None; over-cap → cap-1 chars
/// + '…' (char-safe so a multibyte boundary can't produce invalid UTF-8).
fn str_cap(v: Option<&Value>, cap: usize) -> Option<String> {
    let s = v.and_then(|v| v.as_str())?.trim();
    if s.is_empty() {
        return None;
    }
    if s.chars().count() > cap {
        let mut t: String = s.chars().take(cap.saturating_sub(1)).collect();
        t.push('…');
        Some(t)
    } else {
        Some(s.to_string())
    }
}

/// https-only URL gate (upstream httpsUrl — `app_open_external` enforces the
/// scheme again at the bridge).
fn https_url(v: Option<&Value>) -> Option<String> {
    let s = str_cap(v, MAX_URL)?;
    s.to_lowercase().starts_with("https://").then_some(s)
}

/// Port of `parseHeroPayload` — total function: any field that fails
/// validation drops back to its default rather than failing the whole card.
pub fn parse_hero_payload(raw: &Value) -> Value {
    if !raw.is_object() {
        return default_hero();
    }
    let empty = json!({});
    let plan_in = raw.get("plan").filter(|v| v.is_object()).unwrap_or(&empty);
    let mut plan = json!({
        "label": str_cap(plan_in.get("label"), MAX_LABEL).unwrap_or_else(|| "Local".into()),
        "blurb": str_cap(plan_in.get("blurb"), MAX_BLURB).unwrap_or_else(|| HERO_PLAN_BLURB.into()),
    });
    // Both or neither — a button with no destination is worse than no button.
    if let Some(up) = plan_in.get("upgrade").filter(|v| v.is_object()) {
        if let (Some(label), Some(url)) = (
            str_cap(up.get("label"), MAX_LABEL),
            https_url(up.get("url")),
        ) {
            plan["upgrade"] = json!({ "label": label, "url": url });
        }
    }
    let mut sponsor = Value::Null;
    if let Some(sp) = raw.get("sponsor").filter(|v| v.is_object()) {
        if let (Some(name), Some(url)) =
            (str_cap(sp.get("name"), MAX_NAME), https_url(sp.get("url")))
        {
            sponsor = json!({
                "name": name,
                "blurb": str_cap(sp.get("blurb"), MAX_BLURB).unwrap_or_default(),
                "url": url,
            });
        }
    }
    let notice = str_cap(raw.get("notice"), MAX_NOTICE)
        .map(Value::String)
        .unwrap_or(Value::Null);
    json!({ "plan": plan, "sponsor": sponsor, "notice": notice })
}

/// One bounded GET — upstream getText(url, { timeoutMs }). Errors are plain
/// strings; the caller always degrades rather than propagating.
fn fetch_text(url: &str, timeout_ms: u64) -> Result<String, String> {
    // https only (a redirect to http must not downgrade the payload) and a
    // 512 KiB body cap — these are small JSON documents.
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_millis(timeout_ms)))
        .https_only(true)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut resp = agent
        .get(url)
        .header("user-agent", "agent-office")
        .call()
        .map_err(|e| e.to_string())?;
    if !(200..300).contains(&resp.status().as_u16()) {
        return Err(format!("http {}", resp.status()));
    }
    resp.body_mut()
        .with_config()
        .limit(512 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())
}

// ─── Remote model catalog (main/modelCatalog.ts + shared/modelCatalogPayload.ts) ──

/// docs/model-catalog.json on this repo's main branch — editing that one file
/// puts a new model in every installed copy within the TTL, with no release.
pub const MODEL_CATALOG_URL: &str =
    "https://raw.githubusercontent.com/fkrndev/crew-office-ai/main/docs/model-catalog.json";
/// Models ship on a human timescale — six hours, same as the hero payload.
pub const MODEL_CATALOG_TTL_MS: u64 = 6 * 60 * 60 * 1000;
/// The baked catalog, compiled in; the floor the pickers never fall below.
pub const BAKED_MODEL_CATALOG: &str = include_str!("modelCatalog.json");
const CATALOG_SCHEMA_VERSION: u64 = 1;

/// str() from modelCatalogPayload.ts — control chars become spaces (an id with
/// a newline would be a command-line splice), whitespace collapsed, trimmed,
/// hard-capped; None when nothing usable remains.
fn catalog_str(v: Option<&Value>, cap: usize) -> Option<String> {
    let s = v?.as_str()?;
    let flat: String = s
        .chars()
        .map(|c| if (c as u32) < 0x20 || c as u32 == 0x7f { ' ' } else { c })
        .collect();
    let clean = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.is_empty() {
        return None;
    }
    Some(clean.chars().take(cap).collect())
}

/// Port of `parseModelCatalog` — total: None means "nothing usable, keep the
/// baked catalog", never a partially-built catalog.
pub fn parse_model_catalog(raw: &Value) -> Option<Value> {
    let o = raw.as_object()?;
    if o.get("version").and_then(|v| v.as_u64()) != Some(CATALOG_SCHEMA_VERSION) {
        return None;
    }
    let providers_in = o.get("providers")?.as_object()?;
    let mut providers = serde_json::Map::new();
    for (key, list) in providers_in {
        if providers.len() >= 40 {
            break;
        }
        let key_ok = !key.is_empty()
            && key.len() <= 40
            && key.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            && !matches!(key.as_str(), "__proto__" | "constructor" | "prototype");
        let Some(rows) = list.as_array().filter(|_| key_ok) else {
            continue;
        };
        let mut models: Vec<Value> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for entry in rows.iter().take(60) {
            let Some(eo) = entry.as_object() else { continue };
            let Some(label) = catalog_str(eo.get("label"), 60) else {
                continue;
            };
            let mut m = serde_json::Map::new();
            m.insert("label".into(), Value::String(label));
            let id = catalog_str(eo.get("id"), 120);
            if let Some(id) = &id {
                m.insert("id".into(), Value::String(id.clone()));
            }
            if let Some(min) = catalog_str(eo.get("minAppVersion"), 24) {
                m.insert("minAppVersion".into(), Value::String(min));
            }
            if let Some(max) = catalog_str(eo.get("maxAppVersion"), 24) {
                m.insert("maxAppVersion".into(), Value::String(max));
            }
            if !seen.insert(id.unwrap_or_default()) {
                continue;
            }
            models.push(Value::Object(m));
        }
        if models.is_empty() && !rows.is_empty() {
            continue;
        }
        providers.insert(key.clone(), Value::Array(models));
    }
    if providers.is_empty() {
        return None;
    }
    Some(json!({ "version": CATALOG_SCHEMA_VERSION, "providers": providers }))
}

/// Port of `loadModelCatalog`: cache while fresh, refetch otherwise, NEVER
/// fatal. Always `{ catalog, fetchedAt, stale }`; catalog null = keep the
/// baked one. The cache is re-validated on READ.
pub fn load_model_catalog(cache_path: &Path, force: bool) -> Value {
    load_model_catalog_with(cache_path, force, || {
        fetch_text(MODEL_CATALOG_URL, 8000)
    })
}

pub fn load_model_catalog_with(
    cache_path: &Path,
    force: bool,
    fetch: impl FnOnce() -> Result<String, String>,
) -> Value {
    let now = now_ms();
    let cached: Option<(Value, u64)> = std::fs::read_to_string(cache_path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|read| {
            let catalog = parse_model_catalog(read.get("catalog")?)?;
            Some((catalog, read.get("fetchedAt")?.as_u64()?))
        });
    if let Some((catalog, at)) = &cached {
        if !force && now.saturating_sub(*at) < MODEL_CATALOG_TTL_MS {
            return json!({ "catalog": catalog, "fetchedAt": at, "stale": false });
        }
    }
    let fresh = fetch()
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|raw| parse_model_catalog(&raw));
    match fresh {
        Some(catalog) => {
            if let Some(dir) = cache_path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(
                cache_path,
                json!({ "catalog": catalog, "fetchedAt": now }).to_string(),
            );
            json!({ "catalog": catalog, "fetchedAt": now, "stale": false })
        }
        None => match cached {
            Some((catalog, at)) => json!({ "catalog": catalog, "fetchedAt": at, "stale": true }),
            None => json!({ "catalog": Value::Null, "fetchedAt": 0, "stale": true }),
        },
    }
}

/// Port of `loadHero`: serve the cache while fresh, refresh otherwise, and
/// NEVER fail — the response is always `{ hero, fetchedAt, stale }`.
pub fn load_hero(cache_path: &Path, force: bool) -> Value {
    let now = now_ms();
    let cached: Option<Value> = std::fs::read_to_string(cache_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let cached_hero = cached.as_ref().and_then(|c| c.get("hero").cloned());
    let cached_at = cached
        .as_ref()
        .and_then(|c| c.get("fetchedAt").and_then(|v| v.as_u64()))
        .unwrap_or(0);

    if let Some(hero) = &cached_hero {
        if !force && now.saturating_sub(cached_at) < HERO_TTL_MS {
            return json!({ "hero": hero, "fetchedAt": cached_at, "stale": false });
        }
    }

    // Parse the JSON and the SHAPE separately — valid JSON that is not a hero
    // payload still degrades to defaults rather than rendering undefined.
    match fetch_text(HERO_URL, 8000)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    {
        Some(raw) => {
            let hero = parse_hero_payload(&raw);
            if let Some(dir) = cache_path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(
                cache_path,
                json!({ "hero": hero, "fetchedAt": now }).to_string(),
            );
            json!({ "hero": hero, "fetchedAt": now, "stale": false })
        }
        None => match cached_hero {
            Some(hero) => json!({ "hero": hero, "fetchedAt": cached_at, "stale": true }),
            None => json!({ "hero": default_hero(), "fetchedAt": 0, "stale": true }),
        },
    }
}

// ─── stored-home normalisation (config.ts writeConfig / normalizeStoredHomes) ──

/// Expanded, trimmed, de-duplicated string list (order kept) — registeredRepos
/// and recentHives after `~` expansion.
fn expand_dedupe(v: &Value) -> Option<Vec<Value>> {
    let arr = v.as_array()?;
    let mut seen = std::collections::HashSet::new();
    Some(
        arr.iter()
            .filter_map(|e| e.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(expand_tilde)
            .filter(|s| !s.is_empty() && seen.insert(s.clone()))
            .map(Value::String)
            .collect(),
    )
}

/// normalizeHiveHome: the new home expanded to an absolute path, pushed to the
/// front of `prior` (itself expanded), de-duplicated, capped at 8 — so no
/// consumer (mkdir, the hive root, the launch picker) ever sees a literal `~`.
pub fn normalize_hive_home(home: &str, prior: &[Value]) -> (String, Vec<String>) {
    let abs = expand_tilde(home);
    let mut seen = std::collections::HashSet::from([abs.clone()]);
    let mut recents = vec![abs.clone()];
    for h in prior.iter().filter_map(|v| v.as_str()) {
        if h.trim().is_empty() {
            continue;
        }
        let e = expand_tilde(h);
        if seen.insert(e.clone()) {
            recents.push(e);
        }
    }
    recents.truncate(8);
    (abs, recents)
}

/// The write boundary: expand `~` and de-duplicate `registeredRepos` /
/// `recentHives`, and expand `harnessHome` (also stamping it onto
/// `recentHives`), in a config PATCH before it is persisted. `current_recents`
/// is the on-disk recentHives the new home is merged into.
pub fn normalize_config_patch(
    patch: &mut serde_json::Map<String, Value>,
    current_recents: &[Value],
) {
    if let Some(list) = patch.get("registeredRepos").and_then(expand_dedupe) {
        patch.insert("registeredRepos".into(), Value::Array(list));
    }
    if let Some(list) = patch.get("recentHives").and_then(expand_dedupe) {
        patch.insert("recentHives".into(), Value::Array(list));
    }
    if let Some(home) = patch
        .get("harnessHome")
        .and_then(|v| v.as_str())
        .filter(|h| !h.is_empty())
        .map(str::to_string)
    {
        // A patch carrying its own recentHives list wins as the prior set.
        let prior: Vec<Value> = match patch.get("recentHives").and_then(|v| v.as_array()) {
            Some(a) => a.clone(),
            None => current_recents.to_vec(),
        };
        let (abs, recents) = normalize_hive_home(&home, &prior);
        patch.insert("harnessHome".into(), Value::String(abs));
        patch.insert(
            "recentHives".into(),
            Value::Array(recents.into_iter().map(Value::String).collect()),
        );
    }
}

/// normalizeStoredHomes — the read side: a config.json written before the
/// boundary above learned to expand `~` still holds literal `~/…` strings, and
/// nothing rewrites the file until the next write, so clean them on the way
/// OUT. Expanded duplicates collapse.
pub fn normalize_stored_homes(cfg: &mut serde_json::Map<String, Value>) {
    if let Some(h) = cfg
        .get("harnessHome")
        .and_then(|v| v.as_str())
        .filter(|h| !h.trim().is_empty())
        .map(expand_tilde)
    {
        cfg.insert("harnessHome".into(), Value::String(h));
    }
    for key in ["recentHives", "registeredRepos"] {
        if let Some(list) = cfg.get(key).and_then(expand_dedupe) {
            cfg.insert(key.into(), Value::Array(list));
        }
    }
}

/// The read side's deep-fill of the list keys the renderer iterates: a fresh
/// config has none of them, and `config.registeredRepos[0]` threw in the Add
/// Agent modal (white screen). A present value of any shape is left alone.
pub fn fill_list_defaults(cfg: &mut serde_json::Map<String, Value>) {
    for key in ["registeredRepos", "recentHives"] {
        cfg.entry(key).or_insert_with(|| Value::Array(Vec::new()));
    }
}

// ─── public tunnel ──────────────────────────────────────────────────────────

/// Exact tunnelmole release the tunnel runs (`npm view tunnelmole version`);
/// pinned so `npx --yes` never executes whatever is newest on the registry.
/// Bump deliberately after reviewing the new release.
const TUNNELMOLE_SPEC: &str = "tunnelmole@2.4.0";

/// openTunnel (webhook.ts/slack.ts) — upstream imports `tunnelmole`; the port
/// shells the equivalent CLI (`npx --yes tunnelmole@<pinned> <port>`) and scrapes the
/// minted https URL from its stdout. Best-effort: a missing npx or a mint
/// timeout surfaces as Err — the local server stays up regardless. The
/// returned Child keeps the tunnel alive; kill it to drop the tunnel
/// (upstream's teardown is equally best-effort).
pub fn start_tunnel(port: u16) -> Result<(String, std::process::Child), String> {
    let exe = if cfg!(windows) { "npx.cmd" } else { "npx" };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["--yes", TUNNELMOLE_SPEC, &port.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        // No console flash for the npx.cmd → node chain.
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("tunnelmole spawn failed ({exe}): {e}"))?;
    let (tx, rx) = std::sync::mpsc::channel();
    // Both pipes are read to EOF for the tunnel's whole life: a full pipe
    // buffer (or a closed read end) would stall/kill the long-running node
    // child. Lines are forwarded only until the URL has been scraped.
    if let Some(out) = child.stdout.take() {
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = tx.send(line); // receiver gone after the URL → just drain
            }
        });
    }
    if let Some(err) = child.stderr.take() {
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                log::debug!("[tunnel] {line}");
            }
        });
    }
    // TUNNEL_TIMEOUT_MS — upstream caps the wait at ~45s.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    let mut url = String::new();
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(remaining) {
            Ok(line) => {
                for tok in line.split_whitespace() {
                    if tok.starts_with("https://") {
                        url = tok
                            .trim_end_matches(|c: char| c == '.' || c == ',')
                            .to_string();
                        break;
                    }
                }
                if !url.is_empty() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    if url.is_empty() {
        kill_process_tree(&mut child);
        return Err("tunnelmole returned no URL within 45s".into());
    }
    Ok((url, child))
}

/// Kill a child AND its descendants. On Windows `npx.cmd` is a cmd shim, so
/// `Child::kill` only ends the shim and leaves the node grandchild (the actual
/// tunnel) running — `taskkill /T` sweeps the tree. Elsewhere a plain kill.
pub fn kill_process_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

// ─── slack reply helper script ──────────────────────────────────────────────

/// Write (idempotently) the `<harness>/bin/slack-reply.<ext>` helper and return
/// its path — the analogue of upstream's bundled `md-slack-reply.cjs`. Agents
/// POST {channel, thread_ts, text} to the app's loopback reply rail, whose
/// {port, token} it discovers from `<harness>/slack-reply.json` (written by
/// slack_start). The bot token stays in the main process — the worker env
/// never carries it.
pub fn write_slack_reply_script(home: &Path) -> Result<PathBuf, String> {
    let dir = home.join("bin");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let cfg_file = home.join("slack-reply.json").display().to_string();
    #[cfg(windows)]
    {
        let path = dir.join("slack-reply.ps1");
        // Single-quoted PowerShell literal: only `'` is special (doubled).
        let cfg_path = cfg_file.replace('\'', "''");
        let body = format!(
            r#"param(
  [Parameter(Mandatory = $true)][string]$Channel,
  [Parameter(Mandatory = $true)][string]$Thread,
  [Parameter(Mandatory = $true)][string]$Text
)
# Post a reply into a Slack thread via the Agent Office loopback reply rail
# (the app applies the bot token — it never lives in this process's env).
$cfg = '{cfg_path}'
if (-not (Test-Path -LiteralPath $cfg)) {{ Write-Error 'slack-reply.json missing — is the Slack rail running?'; exit 1 }}
$c = Get-Content -LiteralPath $cfg -Raw | ConvertFrom-Json
$body = @{{ channel = $Channel; thread_ts = $Thread; text = $Text }} | ConvertTo-Json -Compress
Invoke-RestMethod -Method Post -Uri ("http://127.0.0.1:" + $c.port + "/reply") `
  -Headers @{{ 'x-md-reply-token' = $c.token }} `
  -ContentType 'application/json; charset=utf-8' `
  -Body ([System.Text.Encoding]::UTF8.GetBytes($body)) | ConvertTo-Json -Compress
"#
        );
        // UTF-8 BOM: Windows PowerShell 5.1 reads BOM-less scripts as ANSI,
        // which mangles a non-ASCII profile path / the em dash above.
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(body.as_bytes());
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        Ok(path)
    }
    #[cfg(not(windows))]
    {
        let path = dir.join("slack-reply.sh");
        // POSIX single-quoted literal: `'` becomes `'\''`.
        let cfg_path = format!("'{}'", cfg_file.replace('\'', "'\\''"));
        let body = format!(
            r#"#!/bin/sh
# slack-reply.sh --channel <id> --thread <ts> --text <msg>
# Post a reply into a Slack thread via the Agent Office loopback reply rail
# (the app applies the bot token — it never lives in this process's env).
set -e
CHANNEL=""; THREAD=""; TEXT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --channel) CHANNEL="$2"; shift 2 ;;
    --thread) THREAD="$2"; shift 2 ;;
    --text) TEXT="$2"; shift 2 ;;
    *) shift ;;
  esac
done
CFG={cfg_path}
PORT=$(sed -n 's/.*"port":\([0-9]*\).*/\1/p' "$CFG" | head -1)
TOKEN=$(sed -n 's/.*"token":"\([^"]*\)".*/\1/p' "$CFG" | head -1)
[ -n "$PORT" ] && [ -n "$TOKEN" ] || {{ echo "slack-reply.json missing/invalid" >&2; exit 1; }}
# JSON-string escape: drop control chars (keeping tab/newline), tabs -> space,
# escape backslash + quote, join lines with a literal \n.
json_escape() {{
  printf '%s' "$1" | tr -d '\000-\010\013-\037' | tr '\t' ' ' \
    | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' \
    | awk 'BEGIN{{ORS=""}} NR>1{{printf "\\n"}} {{print}}'
}}
printf '{{"channel":"%s","thread_ts":"%s","text":"%s"}}' \
  "$(json_escape "$CHANNEL")" "$(json_escape "$THREAD")" "$(json_escape "$TEXT")" \
  | curl -sS -X POST "http://127.0.0.1:$PORT/reply" \
      -H "x-md-reply-token: $TOKEN" \
      -H 'Content-Type: application/json' \
      --data-binary @-
"#
        );
        std::fs::write(&path, body).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
        }
        Ok(path)
    }
}

/// healthCheckPtys (index.ts) — after a detected resume, probe each live
/// PTY's pid liveness; ones whose process vanished but whose PTY is still
/// registered are the `dead` the renderer auto-revives with --resume.
pub fn power_resume_probe(app: &tauri::AppHandle, reason: &str, away_ms: u64) {
    use tauri::Emitter;
    let st = crate::commands::state(app);
    let ids: Vec<(String, Option<u32>)> = st
        .ptys
        .lock_safe()
        .iter()
        .map(|(id, h)| (id.clone(), h.pid))
        .collect();
    let dead: Vec<String> = ids
        .iter()
        .filter(|(_, pid)| pid.map_or(false, |p| !pid_alive(p)))
        .map(|(id, _)| id.clone())
        .collect();
    if !dead.is_empty() {
        log::warn!(
            "[power] {reason} (away ~{}s): {}/{} PTY(s) look wedged (process gone): {}",
            away_ms / 1000,
            dead.len(),
            ids.len(),
            dead.join(", ")
        );
    }
    let _ = app.emit(
        "power:resume",
        serde_json::json!({
            "reason": reason, "awayMs": away_ms,
            "dead": dead, "total": ids.len(),
        }),
    );
}

/// process.kill(pid, 0) liveness probe — ESRCH means gone.
fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        // tasklist filtered to the pid; "No tasks are running" means dead.
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output();
        match out {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout);
                s.contains(&pid.to_string())
            }
            Err(_) => true,
        }
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_receive_inbox_matches_frontend_table() {
        // Providers that CAN take typed inbox nudges.
        for p in [
            "claude", "codex", "gemini", "grok", "antigravity", "hermes", "openclaw", "pi",
            "opencode", "qwen", "crush", "cursor",
        ] {
            assert!(can_receive_inbox(p), "{p} should receive inbox");
        }
        // The three that cannot — the router hands these to the terminal path.
        for p in ["kimi", "copilot", "custom"] {
            assert!(!can_receive_inbox(p), "{p} must not receive inbox");
        }
    }

    #[test]
    fn inbox_delivery_terminal_only_proxy_tier() {
        assert!(inbox_delivery_terminal("qwen"));
        assert!(inbox_delivery_terminal("crush"));
        assert!(!inbox_delivery_terminal("claude"));
        assert!(!inbox_delivery_terminal("hermes"));
    }

    #[test]
    fn compact_command_focus_only_where_parsed() {
        // claude reads trailing focus text.
        assert_eq!(
            compact_command("claude", "keep the task").as_deref(),
            Some("/compact keep the task")
        );
        // codex drops the focus (its parser ignores it).
        assert_eq!(compact_command("codex", "keep the task").as_deref(), Some("/compact"));
        // providers with no trustworthy verb get None — never type a guess.
        assert_eq!(compact_command("hermes", "x"), None);
        assert_eq!(compact_command("crush", "x"), None);
        assert_eq!(compact_command("custom", "x"), None);
    }

    #[test]
    fn clear_command_override_and_fallback() {
        // A non-empty message is a VERBATIM override (escape hatch for
        // providers the table maps to nothing).
        assert_eq!(clear_command("custom", "/nuke").as_deref(), Some("/nuke"));
        assert_eq!(clear_command("claude", "").as_deref(), Some("/clear"));
        assert_eq!(clear_command("grok", "").as_deref(), Some("/new"));
        assert_eq!(clear_command("hermes", ""), None);
    }

    #[test]
    fn provider_install_table() {
        let (posix, win32, npm) = provider_install("claude");
        assert!(posix.unwrap().contains("claude.ai/install.sh"));
        assert!(win32.unwrap().contains("install.ps1"));
        assert_eq!(npm, Some("npm install -g @anthropic-ai/claude-code"));
        // cursor is native-only (no npm).
        let (cp, cw, cn) = provider_install("cursor");
        assert!(cp.is_some() && cw.is_some() && cn.is_none());
        // hermes — Nous installer, native windows rung, no npm.
        let (hp, hw, hn) = provider_install("hermes");
        assert!(hp.unwrap().contains("install.sh"));
        assert!(hw.unwrap().contains("install.ps1"));
        assert!(hn.is_none());
        // openclaw — official install script both platforms.
        let (op, ow, on) = provider_install("openclaw");
        assert!(op.unwrap().contains("openclaw.ai/install"));
        assert!(ow.unwrap().contains("openclaw.ai/install.ps1"));
        assert!(on.is_none());
        // unknown provider → no rungs at all.
        assert_eq!(provider_install("zzz"), (None, None, None));
    }

    /// UI-only OpenAI-compatible reroute: the Settings → AI Engines base-URL
    /// row must reach hermes AND openclaw spawns as OPENAI_BASE_URL, and stay
    /// silent (no env) when unset — otherwise a configured endpoint is a dead
    /// setting, or an unset one injects garbage into every spawn.
    #[test]
    fn openai_base_url_env_wiring() {
        let mut cfg = crate::config::Config::default();
        assert!(openai_base_url_env(&cfg, "hermes").is_none());
        assert!(openai_base_url_env(&cfg, "openclaw").is_none());
        cfg.provider_base_urls
            .insert("hermes".into(), "https://api.deepseek.com".into());
        cfg.provider_base_urls
            .insert("openclaw".into(), "http://localhost:4000/v1".into());
        assert_eq!(
            openai_base_url_env(&cfg, "hermes"),
            Some(("OPENAI_BASE_URL".into(), "https://api.deepseek.com".into()))
        );
        assert_eq!(
            openai_base_url_env(&cfg, "openclaw"),
            Some(("OPENAI_BASE_URL".into(), "http://localhost:4000/v1".into()))
        );
        // A provider with no row configured gets no env — the field is
        // per-provider, not a global override.
        assert!(openai_base_url_env(&cfg, "claude").is_none());
    }

    /// The openclaw config seed: pins agents.defaults.model.primary, adds an
    /// office-compat provider only when a base URL is set, env-references the
    /// key (never plaintext), merges over the user's existing config, and a
    /// legacy string-form `model` gets upgraded instead of crashing.
    #[test]
    fn openclaw_seed_config_shape() {
        // Model only — no provider block, model ref kept verbatim.
        let out = openclaw_seed_config(&json!({}), None, Some("openrouter/auto"));
        assert_eq!(out["agents"]["defaults"]["model"]["primary"], "openrouter/auto");
        assert!(out["models"].is_null());

        // Base URL + model → office-compat provider + rewritten ref.
        let out = openclaw_seed_config(
            &json!({}),
            Some("https://api.deepseek.com"),
            Some("deepseek-chat"),
        );
        let prov = &out["models"]["providers"]["office-compat"];
        assert_eq!(prov["baseUrl"], "https://api.deepseek.com");
        assert_eq!(prov["apiKey"], "${DEEPSEEK_API_KEY}");
        assert_eq!(prov["models"][0]["id"], "deepseek-chat");
        assert_eq!(
            out["agents"]["defaults"]["model"]["primary"],
            "office-compat/deepseek-chat"
        );

        // Non-deepseek URL → OPENAI_API_KEY env ref.
        let out = openclaw_seed_config(&json!({}), Some("http://localhost:4000/v1"), Some("x/y"));
        assert_eq!(
            out["models"]["providers"]["office-compat"]["apiKey"],
            "${OPENAI_API_KEY}"
        );

        // Existing config survives — unrelated keys and other providers kept.
        let existing = json!({
            "gateway": { "port": 18789 },
            "agents": { "defaults": { "model": "openai/gpt-5.5" } },
            "models": { "providers": { "ollama": { "baseUrl": "http://localhost:11434/v1" } } }
        });
        let out = openclaw_seed_config(&existing, Some("http://x/v1"), Some("llama3"));
        assert_eq!(out["gateway"]["port"], 18789);
        assert_eq!(out["models"]["providers"]["ollama"]["baseUrl"], "http://localhost:11434/v1");
        assert_eq!(out["agents"]["defaults"]["model"]["primary"], "office-compat/llama3");
    }

    /// Fresh-install contract: every provider that CAN self-install has a rung
    /// for the current OS, and every catalog provider id is classified — either
    /// installable (a rung exists) or known-no-installer (dies on its own
    /// missing-binary error, upstream-parity). A new provider landing in the
    /// shared catalog without either arm fails here instead of silently doing
    /// nothing on a fresh machine.
    #[test]
    fn every_provider_is_classified_for_install() {
        const INSTALLABLE: &[&str] = &[
            "claude", "codex", "gemini", "opencode", "crush", "pi", "copilot",
            "cursor", "hermes", "openclaw",
        ];
        const NO_INSTALLER: &[&str] = &["grok", "kimi", "antigravity", "qwen", "custom"];
        for p in INSTALLABLE {
            let (posix, win32, npm) = provider_install(p);
            let has_rung = if cfg!(windows) { win32.is_some() } else { posix.is_some() };
            assert!(
                has_rung || npm.is_some(),
                "provider {p} must have an install rung for this OS or npm"
            );
        }
        for p in NO_INSTALLER {
            assert_eq!(provider_install(p), (None, None, None), "{p} unexpectedly gained a rung — update the matrix");
        }
    }

    /// install_script picks npm first when npm exists, native otherwise — and
    /// never returns a script for a provider with no rungs (spawn must fall
    /// through to the normal path, not install a phantom).
    #[test]
    fn install_script_rung_order_and_missing() {
        // npm available → npm rung wins (fast, no admin).
        let (script, _relaunch) = install_script("claude", "claude", true).unwrap();
        assert!(script.contains("npm install -g"));
        // Native-only provider (no npm package) → vendor installer regardless
        // of npm presence — hermes has no npm rung at all.
        let (script, _) = install_script("hermes", "hermes", true).unwrap();
        if cfg!(windows) {
            assert!(script.contains("install.ps1"));
        } else {
            assert!(script.contains("install.sh"));
        }
        // npm-missing on an npm-capable provider: either the node-then-npm
        // rung (Node installer resolved) or the native fallback (offline).
        let (script, _) = install_script("claude", "claude", false).unwrap();
        assert!(
            script.contains("node") || script.contains("install.ps1") || script.contains("install.sh"),
            "expected node-then-npm or native fallback, got: {script}"
        );
        // no-installer provider → None (spawn falls through, never loops).
        assert!(install_script("custom", "custom", false).is_none());
        assert!(install_script("grok", "grok", true).is_none());
    }

    /// The install script becomes `cmd /d /s /c <one arg>` — cmd executes
    /// only the FIRST line of a /c string and a double-quote unbalances the
    /// parse, so on Windows every script must be a single `&`-chained line
    /// with no quotes. The native-only providers are checked here (their
    /// rung never touches the network for a Node probe).
    #[test]
    fn install_script_is_one_quote_free_cmd_line() {
        if !cfg!(windows) {
            return;
        }
        for p in ["hermes", "openclaw", "cursor"] {
            let (script, relaunch) = install_script(p, p, false).unwrap();
            assert!(!script.contains('\n'), "{p}: must be one cmd line: {script}");
            assert!(!script.contains('"'), "{p}: quotes would break /c: {script}");
            assert!(script.contains(" & "), "{p}: expected &-chained cmd: {script}");
            // Powershell syntax must ride inside `powershell -c` — bare
            // `irm`/`iex`/`(` are cmd errors, not installs.
            assert!(script.contains("powershell"), "{p}: native rung needs powershell -c: {script}");
            assert!(relaunch, "{p}: native rung must re-arm the spawn");
        }
        // POSIX stays newline-joined — sh -c handles multi-line.
    }

    #[test]
    fn tokenize_command_quotes_and_glue() {
        assert_eq!(tokenize_command("claude --model opus"), ["claude", "--model", "opus"]);
        assert_eq!(
            tokenize_command(r#"agy --model "Gemini 3.1 Pro (High)""#),
            ["agy", "--model", "Gemini 3.1 Pro (High)"]
        );
        assert_eq!(tokenize_command("a 'b c' d"), ["a", "b c", "d"]);
        assert_eq!(tokenize_command(""), Vec::<String>::new());
        assert_eq!(tokenize_command("   "), Vec::<String>::new());
    }

    #[test]
    fn quote_and_tokenize_round_trip_losslessly() {
        // Property-style: every token (and every pair/triple of them) survives
        // quote -> join -> tokenize unchanged.
        let atoms = [
            "", "a", "a b", "  ", "\"", "\\", "\\\\", "\\\"", "a\"b", "'", "it's", "a'b c'd",
            "{\"a\":1}", "{\"k\": \"v w\"}", "C:\\Program Files\\x\\", "C:\\dir\\", "x\\\\\"y",
            "tab\tsep", "new\nline", "--flag=\"q\"", "\"\"", "'\"'", "é 文 \"",
        ];
        for a in atoms {
            assert_eq!(tokenize_command(&quote_command_token(a)), [a], "single {a:?}");
            for b in atoms {
                let line = format!("{} {}", quote_command_token(a), quote_command_token(b));
                assert_eq!(tokenize_command(&line), [a, b], "pair {a:?} {b:?}");
            }
        }
        // The motivating case: --settings with an inline JSON blob.
        let args = vec!["--settings".to_string(), r#"{"a":1}"#.to_string()];
        let line = format!("claude {}", args.iter().map(|a| quote_command_token(a)).collect::<Vec<_>>().join(" "));
        assert_eq!(tokenize_command(&line), ["claude", "--settings", r#"{"a":1}"#]);
        // Windows paths in quotes keep their backslashes.
        assert_eq!(
            tokenize_command(r#"x "C:\Program Files\a\b.json""#),
            ["x", r"C:\Program Files\a\b.json"]
        );
    }

    #[test]
    fn slack_reply_script_quotes_the_config_path() {
        let home = std::env::temp_dir().join(format!("o'a-slack {}", uuid::Uuid::new_v4().simple()));
        let path = write_slack_reply_script(&home).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        #[cfg(windows)]
        {
            assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF], "PS 5.1 needs a UTF-8 BOM");
            // `'` doubled inside the single-quoted literal, backslashes untouched.
            let cfg = home.join("slack-reply.json").display().to_string().replace('\'', "''");
            assert!(text.contains(&format!("$cfg = '{cfg}'")), "{text}");
            assert!(!cfg.contains("\\\\"));
        }
        #[cfg(not(windows))]
        {
            let cfg = home.join("slack-reply.json").display().to_string().replace('\'', "'\\''");
            assert!(text.contains(&format!("CFG='{cfg}'")), "{text}");
            assert!(text.contains("json_escape"));
            assert!(!text.contains("--data \"{"), "must not interpolate JSON by hand");
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn openclaw_seed_survives_non_object_sections() {
        for junk in [json!(null), json!([1, 2]), json!("s"), json!(7)] {
            let existing = json!({ "models": junk, "agents": junk });
            let out = openclaw_seed_config(&existing, Some("https://api.deepseek.com"), Some("deepseek/chat"));
            assert_eq!(out["agents"]["defaults"]["model"]["primary"], "office-compat/chat");
            assert_eq!(
                out["models"]["providers"]["office-compat"]["baseUrl"],
                "https://api.deepseek.com"
            );
            let existing = json!({ "agents": { "defaults": junk }, "models": { "providers": junk } });
            let out = openclaw_seed_config(&existing, Some("http://x"), Some("m"));
            assert_eq!(out["agents"]["defaults"]["model"]["primary"], "office-compat/m");
        }
    }

    #[test]
    fn expand_tilde_rules() {
        assert_eq!(expand_tilde(""), "");
        assert_eq!(expand_tilde("/abs/path"), "/abs/path");
        assert_eq!(expand_tilde("relative/path"), "relative/path");
        let expanded = expand_tilde("~/sub");
        assert!(!expanded.starts_with('~'), "tilde must expand: {expanded}");
        assert!(expanded.ends_with("sub"));
    }

    #[test]
    fn is_filesystem_root_blocks_volume_roots() {
        // The reset guard: roots must read true, normal dirs false.
        let tmp = std::env::temp_dir();
        assert!(!is_filesystem_root(&tmp));
        #[cfg(windows)]
        assert!(is_filesystem_root(std::path::Path::new("C:\\")));
        #[cfg(not(windows))]
        assert!(is_filesystem_root(std::path::Path::new("/")));
    }

    #[test]
    fn node_floor_and_artifacts() {
        assert_eq!(node_major("v24.19.0"), Some(24));
        assert_eq!(node_major(" 20.1.0\n"), Some(20));
        assert_eq!(node_major("nope"), None);
        assert!(node_is_usable(Some("v20.0.0")));
        assert!(node_is_usable(Some("v24.19.0")));
        assert!(!node_is_usable(Some("v18.20.4")));
        assert!(!node_is_usable(Some("v19.9.0")));
        assert!(!node_is_usable(None));
        assert!(!node_is_usable(Some("garbage")));
        // Windows ARM gets the arm64 msi, x64 the x64 one
        assert_eq!(
            node_artifact_for("v24.1.0", "windows", "aarch64").as_deref(),
            Some("node-v24.1.0-arm64.msi")
        );
        assert_eq!(
            node_artifact_for("v24.1.0", "windows", "x86_64").as_deref(),
            Some("node-v24.1.0-x64.msi")
        );
        assert_eq!(
            node_artifact_for("v24.1.0", "macos", "aarch64").as_deref(),
            Some("node-v24.1.0.pkg")
        );
        assert_eq!(
            node_artifact_for("v24.1.0", "linux", "aarch64").as_deref(),
            Some("node-v24.1.0-linux-arm64.tar.xz")
        );
        assert_eq!(node_artifact_for("v24.1.0", "linux", "riscv64"), None);
        assert_eq!(node_artifact_for("v24.1.0", "freebsd", "x86_64"), None);
    }

    #[test]
    fn stored_homes_expand_tilde_and_dedupe() {
        let home = dirs::home_dir().unwrap().display().to_string();
        let mut patch = serde_json::Map::new();
        patch.insert("harnessHome".into(), json!("~/HarnessAgents"));
        patch.insert("registeredRepos".into(), json!(["~/dev/a", "~/dev/a", "", "C:/x"]));
        let prior = vec![json!("~/HarnessAgents"), json!("D:/old"), json!("D:/old")];
        normalize_config_patch(&mut patch, &prior);
        let abs = patch["harnessHome"].as_str().unwrap().to_string();
        assert!(abs.starts_with(&home) && abs.ends_with("HarnessAgents"), "{abs}");
        assert!(!abs.contains('~'));
        assert_eq!(patch["recentHives"], json!([abs, "D:/old"]));
        let repos: Vec<&str> = patch["registeredRepos"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(repos.len(), 2);
        assert!(repos[0].ends_with("dev/a") && !repos[0].contains('~'));
        assert_eq!(repos[1], "C:/x");
        // the read side cleans a legacy file
        let mut cfg = serde_json::Map::new();
        cfg.insert("harnessHome".into(), json!("~/H"));
        cfg.insert("recentHives".into(), json!(["~/H", "~/H", "E:/z"]));
        normalize_stored_homes(&mut cfg);
        assert!(!cfg["harnessHome"].as_str().unwrap().contains('~'));
        assert_eq!(cfg["recentHives"].as_array().unwrap().len(), 2);
        // untouched when absent
        let mut empty = serde_json::Map::new();
        normalize_config_patch(&mut empty, &prior);
        assert!(empty.is_empty());
    }

    #[test]
    fn config_view_defaults_the_list_keys_the_ui_iterates() {
        let mut cfg = serde_json::Map::new();
        cfg.insert("recentHives".into(), json!(["D:/h"]));
        fill_list_defaults(&mut cfg);
        assert_eq!(cfg["registeredRepos"], json!([]));
        assert_eq!(cfg["recentHives"], json!(["D:/h"]));
    }

    #[test]
    fn baked_model_catalog_copies_stay_identical() {
        // The Rust-embedded copy and the renderer's build-time copy are one
        // catalog; a drift would give the two sides different pickers.
        assert_eq!(
            include_str!("modelCatalog.json"),
            include_str!("../../src/shared/modelCatalog.json"),
        );
        let v: Value = serde_json::from_str(BAKED_MODEL_CATALOG).unwrap();
        assert!(parse_model_catalog(&v).is_some(), "baked catalog must validate");
    }

    #[test]
    fn model_catalog_parser_rejects_and_sanitises() {
        assert!(parse_model_catalog(&json!({"version": 2, "providers": {"a": []}})).is_none());
        assert!(parse_model_catalog(&json!({"version": 1, "providers": {}})).is_none());
        assert!(parse_model_catalog(&json!([1])).is_none());
        let c = parse_model_catalog(&json!({"version": 1, "providers": {
            "claude": [
                {"id": "a\nb", "label": "A"}, {"id": "a b", "label": "dup"}, {"label": ""}, {"id": "x"}
            ],
            "bad": [{"nope": 1}],
            "__proto__": [{"label": "x"}],
            "custom": []
        }})).unwrap();
        let p = c["providers"].as_object().unwrap();
        assert!(p.contains_key("claude") && p.contains_key("custom"));
        assert!(!p.contains_key("bad") && !p.contains_key("__proto__"));
        // newline neutralised to a space, duplicate id dropped, label-less row dropped
        assert_eq!(c["providers"]["claude"], json!([{"id": "a b", "label": "A"}]));
    }

    #[test]
    fn model_catalog_cache_ttl_force_and_fallback() {
        let dir = std::env::temp_dir().join(format!("oa-mcat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = dir.join("model-catalog.json");
        let body = r#"{"version":1,"providers":{"codex":[{"id":"m1","label":"M1"}]}}"#;
        // cold + fetch fails -> null/stale
        let r = load_model_catalog_with(&cache, false, || Err("offline".into()));
        assert!(r["catalog"].is_null() && r["stale"] == true && r["fetchedAt"] == 0);
        // fetch ok -> fresh + cached
        let r = load_model_catalog_with(&cache, false, || Ok(body.into()));
        assert_eq!(r["stale"], false);
        assert_eq!(r["catalog"]["providers"]["codex"][0]["id"], "m1");
        // within TTL: served from cache without fetching
        let r = load_model_catalog_with(&cache, false, || panic!("must not fetch"));
        assert_eq!(r["stale"], false);
        // force + fetch fails -> cached copy, stale
        let r = load_model_catalog_with(&cache, true, || Err("offline".into()));
        assert_eq!(r["stale"], true);
        assert_eq!(r["catalog"]["providers"]["codex"][0]["id"], "m1");
        // garbage body -> falls back to cache too
        let r = load_model_catalog_with(&cache, true, || Ok("{\"version\":9}".into()));
        assert_eq!(r["stale"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
