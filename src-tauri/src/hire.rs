//! Shareable "hires" — portable agent role templates (manifest spec v1).
//! Port of `shared/hire.ts` + the main-side `hire.ts` transport.
//!
//! SECURITY MODEL — a manifest is untrusted input:
//!   - NEVER auto-spawns: importing only pre-fills the Add-Agent modal.
//!   - Cannot carry a raw command — flags must come from a default-deny
//!     allowlist, and the spawn binary always comes from the local provider.
//!   - `skills`/`mcpServers` are references into bundled allowlists only —
//!     never raw paths or specs. Write/secret-tier MCP ids are surfaced for
//!     human consent, never auto-enabled.
//!   - URL fetches are SSRF-gated: https only (loopback http for dev), the
//!     RESOLVED addresses are checked against the private/reserved blocklist
//!     because https alone doesn't make a target safe.

use serde_json::{json, Value};

pub const HIRE_SPEC_V1: &str = "munder-difflin/hire@1";
pub const HIRE_MAX_BYTES: usize = 64 * 1024;

/// Bundled skill ids — the only values `skills` may reference.
const BUNDLED_SKILL_IDS: [&str; 3] = ["md-hive-sync", "md-fetch-summarize", "md-audit"];

/// MCP catalog id → tier (from shared/mcpCatalog.ts). `write` and `secret`
/// tiers require explicit human consent.
fn mcp_catalog_tier(id: &str) -> Option<&'static str> {
    Some(match id {
        "sequential-thinking" | "time" | "fetch" | "context7" | "filesystem" | "git" => {
            "safe-readonly"
        }
        "github-token" | "db" | "email-calendar" | "search-with-key" => "secret",
        _ => return None,
    })
}

/// Providers a manifest may request ('agy' aliases antigravity). 'custom' is
/// deliberately excluded — it would pick an arbitrary local binary.
const PROVIDERS: [&str; 4] = ["claude", "antigravity", "codex", "cursor"];

/// Flag token shape: letters/digits + conservative punctuation; no quotes,
/// backticks, metachars, % (cmd env-expansion), or whitespace. Args reach the
/// PTY as argv (no shell) — this is defense in depth.
fn flag_re_ok(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 100
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '=' | ':' | ',' | '@' | '+' | '-'))
}

/// Model id shape — flows onto the spawn command line, so shell metachars
/// are rejected outright (a Windows .cmd shim routes through cmd.exe).
fn model_re_ok(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 80
        && t.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, ' ' | '.' | '_' | '(' | ')' | '[' | ']' | '/' | ':' | '@' | '+' | '-')
        })
}

/// Default-deny safe-flag allowlist — every flag PROVABLY cannot escalate
/// permissions, redirect the backend, inject config/MCP, or run commands.
/// (Three rounds of upstream re-review each found one more dangerous
/// spelling — the fix was closing the class, not expanding the denylist.)
const SAFE_FLAG_NAMES: [&str; 4] = ["--model", "--max-turns", "--output-format", "--verbose"];

fn is_safe_flag(token: &str) -> bool {
    token.starts_with('-')
        && SAFE_FLAG_NAMES.contains(&token.split('=').next().unwrap_or("").to_lowercase().as_str())
}

fn capped<'a>(
    v: Option<&'a Value>,
    max: usize,
    field: &str,
    errors: &mut Vec<String>,
    required: bool,
) -> Option<String> {
    let Some(v) = v else {
        if required {
            errors.push(format!("\"{field}\" is required"));
        }
        return None;
    };
    if v.is_null() {
        if required {
            errors.push(format!("\"{field}\" is required"));
        }
        return None;
    }
    let Some(s) = v.as_str() else {
        errors.push(format!("\"{field}\" must be a string"));
        return None;
    };
    let t = s.trim();
    if required && t.is_empty() {
        errors.push(format!("\"{field}\" must not be empty"));
        return None;
    }
    if t.len() > max {
        errors.push(format!("\"{field}\" exceeds {max} chars"));
        return None;
    }
    (!t.is_empty()).then(|| t.to_string())
}

/// Validate an untrusted JSON value into a hire manifest.
/// Returns (manifest-or-null, errors, consentRequired).
pub fn validate_hire_manifest(raw: &Value) -> (Option<Value>, Vec<String>, Vec<String>) {
    let mut errors: Vec<String> = Vec::new();
    if !raw.is_object() {
        return (None, vec!["manifest must be a JSON object".into()], vec![]);
    }
    if raw["spec"].as_str() != Some(HIRE_SPEC_V1) {
        return (
            None,
            vec![format!(
                "unsupported spec \"{}\" (expected \"{HIRE_SPEC_V1}\")",
                raw["spec"].as_str().unwrap_or("<missing>")
            )],
            vec![],
        );
    }
    let g = |k: &str| raw.get(k);

    let name = capped(g("name"), 40, "name", &mut errors, true);
    let description = capped(g("description"), 200, "description", &mut errors, false);
    let goal = capped(g("goal"), 4000, "goal", &mut errors, false);
    let character = capped(g("character"), 24, "character", &mut errors, false).map(|s| s.to_lowercase());
    let accent = capped(g("accent"), 24, "accent", &mut errors, false).map(|s| s.to_lowercase());
    let model = capped(g("model"), 80, "model", &mut errors, false);
    if let Some(m) = &model {
        if !model_re_ok(m) {
            errors.push("\"model\" contains disallowed characters (it goes onto the spawn command line)".into());
        }
    }
    let author = capped(g("author"), 80, "author", &mut errors, false);
    let homepage = capped(g("homepage"), 300, "homepage", &mut errors, false);
    if let Some(h) = &homepage {
        if !h.starts_with("https://") {
            errors.push("\"homepage\" must be https".into());
        }
    }

    let provider = match g("provider") {
        None => None,
        Some(v) => {
            let p = if v.as_str() == Some("agy") {
                "antigravity"
            } else {
                v.as_str().unwrap_or("")
            };
            if PROVIDERS.contains(&p) {
                Some(p.to_string())
            } else {
                errors.push(format!(
                    "\"provider\" must be one of {} (or \"agy\")",
                    PROVIDERS.join(", ")
                ));
                None
            }
        }
    };

    let mut command_flags: Vec<String> = Vec::new();
    if let Some(cf) = g("commandFlags") {
        match cf.as_array() {
            Some(arr) if arr.len() <= 16 => {
                let mut value_allowed = false;
                for (i, f) in arr.iter().enumerate() {
                    let Some(tok) = f.as_str() else {
                        errors.push("commandFlags entry must be a string".into());
                        value_allowed = false;
                        continue;
                    };
                    if !flag_re_ok(tok) {
                        errors.push(format!("commandFlags entry {tok:?} is not a safe flag token"));
                        value_allowed = false;
                        continue;
                    }
                    if i == 0 && !tok.starts_with('-') {
                        errors.push("\"commandFlags\" must start with a flag (e.g. \"--model\")".into());
                        value_allowed = false;
                        continue;
                    }
                    if tok.starts_with('-') {
                        if !is_safe_flag(tok) {
                            errors.push(format!("commandFlags entry {tok:?} is not in the shared-hire safe-flag list"));
                            value_allowed = false;
                            continue;
                        }
                        command_flags.push(tok.to_string());
                        value_allowed = !tok.contains('=');
                    } else if value_allowed {
                        command_flags.push(tok.to_string());
                        value_allowed = false;
                    } else {
                        errors.push(format!("commandFlags entry {tok:?} is not allowed here"));
                    }
                }
            }
            _ => errors.push("\"commandFlags\" must be an array of at most 16 items".into()),
        }
    }

    let capabilities: Vec<String> = g("capabilities")
        .map(|c| match c.as_array() {
            Some(a) if a.len() <= 12 => a
                .iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.trim().chars().take(40).collect::<String>())
                .filter(|s| !s.is_empty())
                .collect(),
            _ => {
                errors.push("\"capabilities\" must be an array of at most 12 items".into());
                vec![]
            }
        })
        .unwrap_or_default();

    let isolate = match g("isolate") {
        None => None,
        Some(v) if v.is_boolean() => v.as_bool(),
        Some(_) => {
            errors.push("\"isolate\" must be a boolean".into());
            None
        }
    };

    const MAX_AGENT_TOKEN_CAP: u64 = 10_000_000_000;
    let token_cap = match g("tokenCap") {
        None => None,
        Some(v) => match v.as_u64() {
            Some(n) if n > 0 && n <= MAX_AGENT_TOKEN_CAP => Some(n),
            _ => {
                errors.push("\"tokenCap\" must be a positive integer (max 1e10)".into());
                None
            }
        },
    };

    let mut skills: Vec<String> = Vec::new();
    if let Some(sk) = g("skills") {
        match sk.as_array() {
            Some(a) if a.len() <= 8 => {
                for s in a {
                    let Some(id) = s.as_str().map(|x| x.trim()) else {
                        errors.push("\"skills\" entries must be non-empty strings".into());
                        continue;
                    };
                    if BUNDLED_SKILL_IDS.contains(&id) {
                        skills.push(id.to_string());
                    } else {
                        errors.push(format!("\"skills\" entry {id:?} is not a bundled skill id"));
                    }
                }
            }
            _ => errors.push("\"skills\" must be an array of at most 8 items".into()),
        }
    }

    let mut mcp_servers: Vec<String> = Vec::new();
    let mut consent_required: Vec<String> = Vec::new();
    if let Some(mc) = g("mcpServers") {
        match mc.as_array() {
            Some(a) if a.len() <= 8 => {
                for s in a {
                    let Some(id) = s.as_str().map(|x| x.trim()) else {
                        errors.push("\"mcpServers\" entries must be non-empty strings".into());
                        continue;
                    };
                    match mcp_catalog_tier(id) {
                        Some(tier) => {
                            mcp_servers.push(id.to_string());
                            if tier != "safe-readonly" {
                                consent_required.push(id.to_string());
                            }
                        }
                        None => errors.push(format!(
                            "\"mcpServers\" entry {id:?} is not a known catalog id"
                        )),
                    }
                }
            }
            _ => errors.push("\"mcpServers\" must be an array of at most 8 items".into()),
        }
    }

    if !errors.is_empty() || name.is_none() {
        return (None, errors, vec![]);
    }
    let mut m = serde_json::Map::new();
    m.insert("spec".into(), json!(HIRE_SPEC_V1));
    m.insert("name".into(), json!(name.unwrap()));
    for (k, v) in [
        ("description", description.map(|s| json!(s))),
        ("goal", goal.map(|s| json!(s))),
        ("character", character.map(|s| json!(s))),
        ("accent", accent.map(|s| json!(s))),
        ("provider", provider.map(|s| json!(s))),
        ("model", model.map(|s| json!(s))),
        ("author", author.map(|s| json!(s))),
        ("homepage", homepage.map(|s| json!(s))),
        ("isolate", isolate.map(|b| json!(b))),
        ("tokenCap", token_cap.map(|n| json!(n))),
    ] {
        if let Some(v) = v {
            m.insert(k.into(), v);
        }
    }
    if !command_flags.is_empty() {
        m.insert("commandFlags".into(), json!(command_flags));
    }
    if !capabilities.is_empty() {
        m.insert("capabilities".into(), json!(capabilities));
    }
    if !skills.is_empty() {
        m.insert("skills".into(), json!(skills));
    }
    if !mcp_servers.is_empty() {
        m.insert("mcpServers".into(), json!(mcp_servers));
    }
    (Some(Value::Object(m)), vec![], consent_required)
}

/// Parse a `munderdifflin://hire?src=<https-url>` deep link.
pub fn parse_hire_deep_link(link: &str) -> Option<String> {
    let rest = link.strip_prefix("munderdifflin:")?;
    // Accept both munderdifflin://hire?src= and munderdifflin:hire?src=
    let (action, query) = {
        let body = rest.trim_start_matches('/');
        let (act, q) = match body.split_once('?') {
            Some((a, q)) => (a.to_lowercase(), q.to_string()),
            None => (body.to_lowercase(), String::new()),
        };
        (act, q)
    };
    if action != "hire" {
        return None;
    }
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == "src" {
            let decoded = urlencoding(v);
            if is_allowed_manifest_url(&decoded) {
                return Some(decoded);
            }
        }
    }
    None
}

fn urlencoding(s: &str) -> String {
    s.replace("%3A", ":")
        .replace("%2F", "/")
        .replace("%3F", "?")
        .replace("%3D", "=")
        .replace("%26", "&")
        .replace("%3a", ":")
        .replace("%2f", "/")
        .replace("%3f", "?")
        .replace("%3d", "=")
        .replace("%26", "&")
}

/// https everywhere; http ONLY for loopback (local gallery development).
pub fn is_allowed_manifest_url(u: &str) -> bool {
    if u.starts_with("https://") {
        return true;
    }
    if let Some(rest) = u.strip_prefix("http://") {
        let host = rest.split('/').next().unwrap_or("").split(':').next().unwrap_or("");
        return host == "localhost" || host == "127.0.0.1" || host == "[::1]";
    }
    false
}

/* ── transport (main-side hire.ts) ───────────────────────────────────────── */

/// Address ranges a remote manifest may NEVER resolve to — loopback,
/// RFC1918, link-local incl. 169.254.169.254 (cloud metadata), CGNAT, ULA,
/// multicast/reserved. Checked on the RESOLVED address: an https URL can
/// still point at https://10.x or a v4-mapped v6 form.
fn ip_blocked(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xC0) == 64) // CGNAT 100.64/10
                || o[0] >= 224
        }
        std::net::IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return ip_blocked(&std::net::IpAddr::V4(v4));
            }
            let seg = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (seg[0] & 0xFF80) == 0xFE80 // fe80::/10 link-local
                || (seg[0] & 0xFE00) == 0xFC00 // fc00::/7 ULA
                || (seg[0] & 0xFFC0) == 0xFEC0 // fec0::/10 deprecated site-local
        }
    }
}

/// Fetch a manifest from an https URL with the SSRF guard: resolve the host
/// and reject the whole fetch when ANY resolved address is blocked.
pub fn fetch_manifest(url: &str) -> Result<Value, String> {
    if !is_allowed_manifest_url(url) {
        return Err("manifest URL must be https (or loopback http for dev)".into());
    }
    let rest = url.split("://").nth(1).unwrap_or("");
    let hostport = rest.split('/').next().unwrap_or("");
    let (host, port) = match hostport.split_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(443)),
        None => (hostport.trim_matches(|c| c == '[' || c == ']').to_string(), 443),
    };
    // Loopback http is allowed (dev) — skip the blocklist for it.
    let is_loopback_http = url.starts_with("http://");
    if !is_loopback_http {
        use std::net::ToSocketAddrs;
        let resolved: Vec<std::net::SocketAddr> = (host.as_str(), port)
            .to_socket_addrs()
            .map_err(|e| format!("DNS resolution failed: {e}"))?
            .collect();
        if resolved.is_empty() {
            return Err("DNS resolution returned no addresses".into());
        }
        for sa in &resolved {
            if ip_blocked(&sa.ip()) {
                return Err("manifest host resolves to a blocked (private/reserved) address".into());
            }
        }
    }
    let mut resp = ureq::get(url)
        .call()
        .map_err(|e| format!("fetch failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("fetch failed: HTTP {}", resp.status().as_u16()));
    }
    let bytes = resp
        .body_mut()
        .with_config()
        .limit(HIRE_MAX_BYTES as u64)
        .read_to_vec()
        .map_err(|e| format!("read failed: {e}"))?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| format!("manifest is not JSON: {e}"))?;
    Ok(v)
}

/// Read a manifest from disk (file import) — same byte cap.
pub fn read_manifest_file(path: &std::path::Path) -> Result<Value, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("read failed: {e}"))?;
    if meta.len() > HIRE_MAX_BYTES as u64 {
        return Err(format!("manifest exceeds {HIRE_MAX_BYTES} bytes").into());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("read failed: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("manifest is not JSON: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_manifest_rounds_out_fields() {
        let (m, errs, consent) = validate_hire_manifest(&json!({
            "spec": "munder-difflin/hire@1",
            "name": "Docs Writer",
            "provider": "agy",
            "model": "claude-sonnet-4-6",
            "commandFlags": ["--model", "claude-sonnet-4-6", "--max-turns=20"],
            "skills": ["md-audit"],
            "mcpServers": ["filesystem", "github-token"],
            "tokenCap": 5000000,
            "isolate": true,
        }));
        assert!(errs.is_empty(), "{errs:?}");
        let m = m.unwrap();
        assert_eq!(m["provider"], "antigravity"); // agy alias
        assert_eq!(m["commandFlags"].as_array().unwrap().len(), 3);
        assert_eq!(consent, vec!["github-token"]); // secret tier → consent
    }

    #[test]
    fn unsafe_flags_and_providers_reject() {
        for (flags, desc) in [
            (json!(["--dangerous-allowed"]), "unknown flag"),
            (json!(["-c", "x=y"]), "short flags never allowed"),
            (json!(["value-first"]), "must start with a flag"),
            (json!(["--model", "x", "y"]), "chained second value"),
            (json!(["--verbose;rm"]), "metachars"),
        ] {
            let (_, errs, _) = validate_hire_manifest(&json!({
                "spec": "munder-difflin/hire@1", "name": "n", "commandFlags": flags
            }));
            assert!(!errs.is_empty(), "{desc} should reject: {flags}");
        }
        let (_, errs, _) = validate_hire_manifest(&json!({
            "spec": "munder-difflin/hire@1", "name": "n", "provider": "custom"
        }));
        assert!(!errs.is_empty(), "custom provider must reject");
        let (_, errs, _) = validate_hire_manifest(&json!({
            "spec": "munder-difflin/hire@1", "name": "n", "model": "x; rm -rf /"
        }));
        assert!(!errs.is_empty(), "model metachars must reject");
    }

    #[test]
    fn deep_link_and_url_rules() {
        assert_eq!(
            parse_hire_deep_link("munderdifflin://hire?src=https%3A%2F%2Fex.com%2Fm.json"),
            Some("https://ex.com/m.json".to_string())
        );
        assert!(parse_hire_deep_link("munderdifflin://spawn?src=x").is_none());
        assert!(is_allowed_manifest_url("https://ex.com/a.json"));
        assert!(is_allowed_manifest_url("http://localhost:8000/a.json"));
        assert!(!is_allowed_manifest_url("http://ex.com/a.json"));
        assert!(!is_allowed_manifest_url("ftp://ex.com"));
    }

    #[test]
    fn ssrf_blocklist_covers_private_and_metadata() {
        use std::net::IpAddr;
        for ip in ["10.0.0.5", "172.16.0.1", "192.168.1.1", "127.0.0.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "224.0.0.1", "::1", "fc00::1", "fe80::1", "::ffff:10.0.0.1"] {
            assert!(ip_blocked(&ip.parse::<IpAddr>().unwrap()), "{ip} should be blocked");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "100.63.255.255"] {
            assert!(!ip_blocked(&ip.parse::<IpAddr>().unwrap()), "{ip} should pass");
        }
    }
}
