//! Shared helpers for `cmd_misc.rs` — no `#[tauri::command]` here.
//!
//! Contents: the keychain read seam (commands.rs keeps its `secret_get`
//! private, so the provider-key surface mirrors it here), path plumbing
//! (tilde expansion, the filesystem-root guard behind `app_reset_all`), the
//! Settings hero payload port (main/hero.ts + shared/heroPayload.ts), and the
//! slack reply helper script the prompt rail points at.

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
        // crush/copilot/cursor/custom/hermes → NO_CONTEXT_COMMANDS: leave the
        // terminal alone rather than typing an untrusted verb.
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
pub fn provider_install(provider: &str) -> (Option<&'static str>, Option<&'static str>, Option<&'static str>) {
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
            Some("irm https://cursor.com/install?win32=true | iex"),
            None,
        ),
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

fn resolve_node_installer() -> Option<NodeInstaller> {
    let index: serde_json::Value = ureq::get(&format!("{NODE_DIST}/index.json"))
        .call().ok()?.body_mut().read_json().ok()?;
    let lts = index.as_array()?.iter()
        .find(|e| e["lts"].is_string() || e["lts"] == serde_json::json!(true))?;
    let version = lts["version"].as_str()?.to_string();
    let file = if cfg!(windows) {
        format!("node-{version}-x64.msi")
    } else if cfg!(target_os = "macos") {
        format!("node-{version}.pkg")
    } else {
        let a = match std::env::consts::ARCH { "aarch64" => "arm64", _ => "x64" };
        format!("node-{version}-linux-{a}.tar.xz")
    };
    let sha_body: String = ureq::get(&format!("{NODE_DIST}/{version}/SHASUMS256.txt"))
        .call().ok()?.body_mut().read_to_string().ok()?;
    // "<sha>  <file>" lines — pull our artifact's digest.
    let sha256 = sha_body.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let sha = it.next()?;
        (it.next()? == file).then(|| sha.to_string())
    })?;
    Some(NodeInstaller { url: format!("{NODE_DIST}/{version}/{file}"), version, file, sha256 })
}

/// buildNodeInstallScript (nodeInstall.ts) — download → digest-verify →
/// install. Windows: certutil + findstr (the only guaranteed tools);
/// msiexec's own UAC prompt is the elevation — never silent. POSIX:
/// shasum on macOS, sha256sum on Linux.
fn node_install_script(n: &NodeInstaller) -> String {
    if cfg!(windows) {
        let f = format!("%TEMP%\\{}", n.file);
        return [
            format!("echo   Downloading Node.js {} ^(official installer^)...", n.version),
            format!("curl -fSL {} -o {f}", n.url),
            "if errorlevel 1 exit /b 1".to_string(),
            "echo   Verifying checksum...".to_string(),
            format!("certutil -hashfile {f} SHA256 | findstr /i /c:{} >nul", n.sha256),
            "if errorlevel 1 (echo   [x] CHECKSUM MISMATCH - refusing to install ^& exit /b 1)".to_string(),
            "echo   Installing - approve the Windows prompt if it appears...".to_string(),
            format!("msiexec /i {f} /passive /norestart"),
            "if errorlevel 1 exit /b 1".to_string(),
            "set \"PATH=%ProgramFiles%\\nodejs;%PATH%\"".to_string(),
        ].join("\r\n");
    }
    let verify = if cfg!(target_os = "macos") {
        format!("echo \"{}  $__f\" | shasum -a 256 -c - >/dev/null", n.sha256)
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
pub fn install_script(
    provider: &str,
    bin: &str,
    npm_available: bool,
) -> Option<(String, bool)> {
    let (native_posix, native_win32, npm) = provider_install(provider);
    let native = if cfg!(windows) { native_win32 } else { native_posix };
    let header = format!("[agent-office] \"{bin}\" is not installed — ");

    // Rung 1: npm present — run the package install directly.
    if let (Some(cmd), true) = (npm, npm_available) {
        return Some((
            format!("echo {header}installing it with npm.\r\n{cmd}"),
            true,
        ));
    }

    // Rung 2: npm missing but the official Node installer resolves — install
    // Node first, then the package (node-then-npm). The PATH splice lets the
    // npm step resolve node inside the same shell.
    if let Some(cmd) = npm {
        if let Some(n) = resolve_node_installer() {
            let path_fix = if cfg!(windows) { "" } else { "export PATH=/usr/local/bin:$PATH" };
            return Some((
                format!(
                    "echo {header}installing Node.js first, then the package.\r\n{}\r\n{path_fix}{cmd}",
                    node_install_script(&n)
                ),
                true,
            ));
        }
    }

    // Rung 3: the vendor's self-contained installer (no Node needed).
    if let Some(cmd) = native {
        return Some((
            format!("echo {header}running the official {provider} installer (no Node needed).\r\n{cmd}"),
            true,
        ));
    }

    // Rung 4: manual hint — honest output, intentionally no relaunch.
    if native.is_some() || npm.is_some() {
        return Some((
            format!(
                "echo {header}no bundled installer works on this machine.\r\necho Install {bin} manually, then respawn this agent."
            ),
            false,
        ));
    }
    None
}

pub fn expand_tilde(p: &str) -> String {
    let t = p.trim();
    if t.is_empty() {
        return p.to_string();
    }
    let home = || dirs::home_dir().map(|h| h.display().to_string()).unwrap_or_default();
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

/// Last-resort OS opener — `cmd /c start` on Windows, `open` on macOS,
/// `xdg-open` elsewhere. Used when the shell plugin's `open` fails, and as the
/// directory opener for `fs_reveal_path`.
pub fn os_open(target: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/c", "start", "", target])
            .spawn()
            .map_err(|e| e.to_string())?;
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
        _ => "application/octet-stream",
    }
}

// ─── Settings hero payload (main/hero.ts + shared/heroPayload.ts) ───────────

/// The remote payload URL — upstream HERO_URL verbatim.
pub const HERO_URL: &str =
    "https://raw.githubusercontent.com/chaitanyagiri/munder-difflin/main/docs/hero.json";
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
        if let (Some(label), Some(url)) =
            (str_cap(up.get("label"), MAX_LABEL), https_url(up.get("url")))
        {
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
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_millis(timeout_ms)))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut resp = agent.get(url).call().map_err(|e| e.to_string())?;
    if !(200..300).contains(&resp.status().as_u16()) {
        return Err(format!("http {}", resp.status()));
    }
    resp.body_mut().read_to_string().map_err(|e| e.to_string())
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

// ─── public tunnel ──────────────────────────────────────────────────────────

/// openTunnel (webhook.ts/slack.ts) — upstream imports `tunnelmole`; the port
/// shells the equivalent CLI (`npx --yes tunnelmole <port>`) and scrapes the
/// minted https URL from its stdout. Best-effort: a missing npx or a mint
/// timeout surfaces as Err — the local server stays up regardless. The
/// returned Child keeps the tunnel alive; kill it to drop the tunnel
/// (upstream's teardown is equally best-effort).
pub fn start_tunnel(port: u16) -> Result<(String, std::process::Child), String> {
    let exe = if cfg!(windows) { "npx.cmd" } else { "npx" };
    let mut child = std::process::Command::new(exe)
        .args(["--yes", "tunnelmole", &port.to_string()])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("tunnelmole spawn failed ({exe}): {e}"))?;
    let (tx, rx) = std::sync::mpsc::channel();
    if let Some(out) = child.stdout.take() {
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
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
                        url = tok.trim_end_matches(|c: char| c == '.' || c == ',').to_string();
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
        let _ = child.kill();
        return Err("tunnelmole returned no URL within 45s".into());
    }
    Ok((url, child))
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
    let cfg_path = home.join("slack-reply.json").display().to_string().replace('\\', "\\\\");
    #[cfg(windows)]
    {
        let path = dir.join("slack-reply.ps1");
        let body = format!(r#"param(
  [Parameter(Mandatory = $true)][string]$Channel,
  [Parameter(Mandatory = $true)][string]$Thread,
  [Parameter(Mandatory = $true)][string]$Text
)
# Post a reply into a Slack thread via the Agent Office loopback reply rail
# (the app applies the bot token — it never lives in this process's env).
$cfg = '{cfg_path}'
if (-not (Test-Path $cfg)) {{ Write-Error 'slack-reply.json missing — is the Slack rail running?'; exit 1 }}
$c = Get-Content $cfg -Raw | ConvertFrom-Json
$body = @{{ channel = $Channel; thread_ts = $Thread; text = $Text }} | ConvertTo-Json -Compress
Invoke-RestMethod -Method Post -Uri ("http://127.0.0.1:" + $c.port + "/reply") `
  -Headers @{{ 'x-md-reply-token' = $c.token }} `
  -ContentType 'application/json; charset=utf-8' -Body $body | ConvertTo-Json -Compress
"#);
        std::fs::write(&path, body).map_err(|e| e.to_string())?;
        Ok(path)
    }
    #[cfg(not(windows))]
    {
        let path = dir.join("slack-reply.sh");
        let body = format!(r#"#!/bin/sh
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
CFG="{cfg_path}"
PORT=$(sed -n 's/.*"port":\([0-9]*\).*/\1/p' "$CFG" | head -1)
TOKEN=$(sed -n 's/.*"token":"\([^"]*\)".*/\1/p' "$CFG" | head -1)
[ -n "$PORT" ] && [ -n "$TOKEN" ] || {{ echo "slack-reply.json missing/invalid" >&2; exit 1; }}
curl -sS -X POST "http://127.0.0.1:$PORT/reply" \
  -H "x-md-reply-token: $TOKEN" \
  -H 'Content-Type: application/json' \
  --data "{{\"channel\":\"$CHANNEL\",\"thread_ts\":\"$THREAD\",\"text\":\"$TEXT\"}}"
"#);
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
        .lock()
        .unwrap()
        .iter()
        .map(|(id, h)| (id.clone(), h.pid))
        .collect();
    let dead: Vec<String> = ids
        .iter()
        .filter(|(_, pid)| pid.map_or(false, |p| !pid_alive(p)))
        .map(|(id, _)| id.clone())
        .collect();
    if !dead.is_empty() {
        eprintln!(
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
