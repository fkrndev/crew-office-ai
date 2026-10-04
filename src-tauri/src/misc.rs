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
    !matches!(provider, "kimi" | "copilot")
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
