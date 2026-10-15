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

pub const HIRE_SPEC_V1: &str = "agent-office/hire@1";
/// Spec tag written by the upstream project this app was forked from. Manifests
/// authored for it are still accepted on import (they are normalized to
/// `HIRE_SPEC_V1`); nothing is ever written with it. The one deliberate
/// remaining occurrence of the old name — keep it so existing shared hires
/// keep working.
pub const LEGACY_HIRE_SPEC_V1: &str = "munder-difflin/hire@1";
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
        && t.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '.' | '_' | '/' | '=' | ':' | ',' | '@' | '+' | '-')
        })
}

/// Model id shape — flows onto the spawn command line, so shell metachars
/// are rejected outright (a Windows .cmd shim routes through cmd.exe).
fn model_re_ok(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 80
        && t.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    ' ' | '.' | '_' | '(' | ')' | '[' | ']' | '/' | ':' | '@' | '+' | '-'
                )
        })
}

/// Default-deny safe-flag allowlist — every flag PROVABLY cannot escalate
/// permissions, redirect the backend, inject config/MCP, or run commands.
/// (Three rounds of upstream re-review each found one more dangerous
/// spelling — the fix was closing the class, not expanding the denylist.)
const SAFE_FLAG_NAMES: [&str; 4] = ["--model", "--max-turns", "--output-format", "--verbose"];

fn is_safe_flag(token: &str) -> bool {
    token.starts_with('-')
        && SAFE_FLAG_NAMES.contains(
            &token
                .split('=')
                .next()
                .unwrap_or("")
                .to_lowercase()
                .as_str(),
        )
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
    if !matches!(raw["spec"].as_str(), Some(HIRE_SPEC_V1) | Some(LEGACY_HIRE_SPEC_V1)) {
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
    let character =
        capped(g("character"), 24, "character", &mut errors, false).map(|s| s.to_lowercase());
    let accent = capped(g("accent"), 24, "accent", &mut errors, false).map(|s| s.to_lowercase());
    let model = capped(g("model"), 80, "model", &mut errors, false);
    if let Some(m) = &model {
        if !model_re_ok(m) {
            errors.push(
                "\"model\" contains disallowed characters (it goes onto the spawn command line)"
                    .into(),
            );
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
                        errors.push(format!(
                            "commandFlags entry {tok:?} is not a safe flag token"
                        ));
                        value_allowed = false;
                        continue;
                    }
                    if i == 0 && !tok.starts_with('-') {
                        errors.push(
                            "\"commandFlags\" must start with a flag (e.g. \"--model\")".into(),
                        );
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

/// Parse an `agent-office://hire?src=<https-url>` deep link (the app's only
/// registered scheme).
pub fn parse_hire_deep_link(link: &str) -> Option<String> {
    let rest = link.strip_prefix("agent-office:")?;
    // Accept both agent-office://hire?src= and agent-office:hire?src=
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

/// A strictly-parsed manifest URL authority. Hand-rolled (no `url` crate) and
/// deliberately narrow: anything ambiguous between parsers — userinfo (`@`),
/// backslashes, whitespace/control chars, non-ASCII, percent-escapes in the
/// host, odd ports — is rejected outright, because the SSRF guard and the HTTP
/// client must agree on which host a URL means.
#[derive(Debug, PartialEq, Eq)]
struct ManifestUrl {
    https: bool,
    /// Lowercased host; an IPv6 literal is stored WITHOUT brackets.
    host: String,
    port: u16,
}

impl ManifestUrl {
    fn is_loopback_host(&self) -> bool {
        self.host == "localhost"
            || self
                .host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    }
}

fn parse_manifest_url(u: &str) -> Option<ManifestUrl> {
    if u.is_empty() || u.len() > 2048 || !u.is_ascii() {
        return None;
    }
    if u.chars().any(|c| c.is_control() || c == ' ' || c == '\\') {
        return None;
    }
    let lower = u.to_ascii_lowercase();
    let (https, rest) = if let Some(r) = lower.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = lower.strip_prefix("http://") {
        (false, r)
    } else {
        return None;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') || authority.contains('%') {
        return None;
    }
    let (host, port_str) = if let Some(inner) = authority.strip_prefix('[') {
        // [v6] or [v6]:port
        let (addr, after) = inner.split_once(']')?;
        addr.parse::<std::net::Ipv6Addr>().ok()?;
        let port = match after {
            "" => None,
            p => Some(p.strip_prefix(':')?),
        };
        (addr.to_string(), port)
    } else {
        let (h, p) = match authority.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        };
        if h.is_empty()
            || h.contains(':')
            || h.starts_with(['.', '-'])
            || h.ends_with('-')
            || !h.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        {
            return None;
        }
        (h.to_string(), p)
    };
    let port = match port_str {
        None => {
            if https {
                443
            } else {
                80
            }
        }
        Some(p) => {
            if p.is_empty() || p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            match p.parse::<u16>() {
                Ok(n) if n > 0 => n,
                _ => return None,
            }
        }
    };
    Some(ManifestUrl { https, host, port })
}

/// https everywhere; http ONLY for loopback (local gallery development).
pub fn is_allowed_manifest_url(u: &str) -> bool {
    parse_manifest_url(u).is_some_and(|p| p.https || p.is_loopback_host())
}

/* ── transport (main-side hire.ts) ───────────────────────────────────────── */

/// An IPv6 literal that embeds an IPv4 destination - v4-mapped
/// `::ffff:a.b.c.d`, the deprecated v4-compatible `::a.b.c.d` (and `::1`),
/// NAT64 `64:ff9b::/96`, or 6to4 `2002::/16` - yields that v4 so the v4
/// classifier gates it (upstream embeddedV4). All of these ROUTE to a v4
/// target, and the hex-group spelling is exactly what slipped past a textual
/// v6 check.
fn embedded_v4(v6: &std::net::Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    let g = v6.segments();
    let v4 = |hi: u16, lo: u16| {
        std::net::Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8)
    };
    let top5_zero = g[..5].iter().all(|s| *s == 0);
    if top5_zero && g[5] == 0xffff {
        return Some(v4(g[6], g[7])); // v4-mapped
    }
    if top5_zero && g[5] == 0 && (g[6] != 0 || g[7] != 0) {
        return Some(v4(g[6], g[7])); // v4-compatible (and ::1)
    }
    if g[0] == 0x0064 && g[1] == 0xff9b && g[2..6].iter().all(|s| *s == 0) {
        return Some(v4(g[6], g[7])); // NAT64 64:ff9b::/96
    }
    if g[0] == 0x2002 {
        return Some(v4(g[1], g[2])); // 6to4 2002::/16
    }
    None
}

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
            if let Some(v4) = embedded_v4(v6) {
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

/// Connect-time guard: resolves through ureq's default resolver, then refuses
/// to hand the connector ANY address for the manifest host that fails policy.
/// The addresses validated here are the ones connected to (one resolution —
/// no DNS-rebinding window between check and use). Other hosts (an
/// env-configured proxy) pass through untouched.
#[derive(Debug)]
struct GuardedResolver {
    host: String,
    loopback_only: bool,
}

impl GuardedResolver {
    fn ip_ok(&self, ip: &std::net::IpAddr) -> bool {
        if self.loopback_only {
            let v4 = match ip {
                std::net::IpAddr::V6(v6) => v6.to_ipv4_mapped(),
                _ => None,
            };
            ip.is_loopback() || v4.is_some_and(|a| a.is_loopback())
        } else {
            !ip_blocked(ip)
        }
    }
}

impl ureq::unversioned::resolver::Resolver for GuardedResolver {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        let addrs = ureq::unversioned::resolver::DefaultResolver::default()
            .resolve(uri, config, timeout)?;
        let is_target = uri
            .host()
            .is_some_and(|h| h.trim_matches(['[', ']']).eq_ignore_ascii_case(&self.host));
        if is_target && addrs.iter().any(|sa| !self.ip_ok(&sa.ip())) {
            return Err(ureq::Error::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "manifest host resolves to a blocked (private/reserved) address",
            )));
        }
        Ok(addrs)
    }
}

/// Fetch a manifest from an https URL with the SSRF guard: the URL is parsed
/// strictly (no userinfo/backslash tricks), http is loopback-only, EVERY
/// resolved address is validated for both schemes, redirects are off (a 3xx
/// to an internal host is an error, not a hop), and the client connects only
/// to the addresses that were validated.
pub fn fetch_manifest(url: &str) -> Result<Value, String> {
    let parsed = parse_manifest_url(url)
        .filter(|p| p.https || p.is_loopback_host())
        .ok_or_else(|| "manifest URL must be https (or loopback http for dev)".to_string())?;
    let loopback_only = !parsed.https;
    let guard = GuardedResolver {
        host: parsed.host.clone(),
        loopback_only,
    };
    {
        // Early, readable failure; the resolver above re-checks at connect.
        use std::net::ToSocketAddrs;
        let resolved: Vec<std::net::SocketAddr> = (parsed.host.as_str(), parsed.port)
            .to_socket_addrs()
            .map_err(|e| format!("DNS resolution failed: {e}"))?
            .collect();
        if resolved.is_empty() {
            return Err("DNS resolution returned no addresses".into());
        }
        if resolved.iter().any(|sa| !guard.ip_ok(&sa.ip())) {
            return Err("manifest host resolves to a blocked (private/reserved) address".into());
        }
    }
    let config = ureq::Agent::config_builder()
        .max_redirects(0)
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .build();
    let agent = ureq::Agent::with_parts(
        config,
        ureq::unversioned::transport::DefaultConnector::new(),
        guard,
    );
    let mut resp = agent
        .get(url)
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
    let v: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("manifest is not JSON: {e}"))?;
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

/// readHireManifestFiles - validate every selected manifest independently so
/// one bad file never costs the valid neighbours picked with it. Errors name
/// the file by BASENAME (never its full local path - that would leak the user's
/// directory structure into logs/screenshots). Returns (manifests, errors).
pub fn read_hire_manifest_files(paths: &[std::path::PathBuf]) -> (Vec<Value>, Vec<String>) {
    let mut manifests = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "manifest".into());
        let outcome = read_manifest_file(path).and_then(|raw| {
            let (m, errs, _consent) = validate_hire_manifest(&raw);
            m.ok_or_else(|| errs.join("; "))
        });
        match outcome {
            Ok(m) => manifests.push(m),
            Err(e) => errors.push(format!(
                "{name}: {}",
                e.replace(&path.display().to_string(), &name)
            )),
        }
    }
    (manifests, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssrf_guard_blocks_v6_forms_that_embed_a_private_v4() {
        let blocked = |s: &str| ip_blocked(&s.parse::<std::net::IpAddr>().unwrap());
        // v4-mapped (dotted + hex), v4-compatible, ::1
        assert!(blocked("::ffff:127.0.0.1"));
        assert!(blocked("::ffff:7f00:1"));
        assert!(blocked("::127.0.0.1"));
        assert!(blocked("::7f00:1"));
        assert!(blocked("::1"));
        assert!(blocked("::10.0.0.5"));
        // NAT64 64:ff9b::/96 wrapping loopback / RFC1918 / metadata
        assert!(blocked("64:ff9b::7f00:1"));
        assert!(blocked("64:ff9b::a00:5"));
        assert!(blocked("64:ff9b::a9fe:a9fe"));
        // 6to4 2002::/16 wrapping 127.0.0.1 / 10.0.0.1 / 169.254.169.254
        assert!(blocked("2002:7f00:1::"));
        assert!(blocked("2002:a00:1::1"));
        assert!(blocked("2002:a9fe:a9fe::"));
        // ...but the same wrappers around PUBLIC v4 addresses stay reachable
        assert!(!blocked("::ffff:8.8.8.8"));
        assert!(!blocked("::808:808"));
        assert!(!blocked("64:ff9b::808:808"));
        assert!(!blocked("2002:808:808::"));
        assert!(!blocked("2606:4700:4700::1111"));
    }

    #[test]
    fn batch_import_keeps_valid_neighbours_and_names_files_by_basename() {
        let d = std::env::temp_dir().join(format!("oa-hirebatch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        let ok = d.join("good.json");
        std::fs::write(
            &ok,
            r#"{"spec":"agent-office/hire@1","name":"Docs Writer","provider":"claude"}"#,
        )
        .unwrap();
        let bad_spec = d.join("bad-spec.json");
        std::fs::write(&bad_spec, r#"{"spec":"nope","name":"X"}"#).unwrap();
        let not_json = d.join("broken.json");
        std::fs::write(&not_json, "{ nope").unwrap();
        let missing = d.join("gone.json");
        let (manifests, errors) =
            read_hire_manifest_files(&[bad_spec, ok, not_json, missing]);
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0]["name"], "Docs Writer");
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(errors[0].starts_with("bad-spec.json: "), "{errors:?}");
        assert!(errors[1].starts_with("broken.json: "), "{errors:?}");
        assert!(errors[2].starts_with("gone.json: "), "{errors:?}");
        // no full local paths leak
        let dir = d.display().to_string();
        assert!(errors.iter().all(|e| !e.contains(&dir)), "{errors:?}");
        assert_eq!(read_hire_manifest_files(&[]), (vec![], vec![]));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn valid_manifest_rounds_out_fields() {
        let (m, errs, consent) = validate_hire_manifest(&json!({
            "spec": "agent-office/hire@1",
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
                "spec": "agent-office/hire@1", "name": "n", "commandFlags": flags
            }));
            assert!(!errs.is_empty(), "{desc} should reject: {flags}");
        }
        let (_, errs, _) = validate_hire_manifest(&json!({
            "spec": "agent-office/hire@1", "name": "n", "provider": "custom"
        }));
        assert!(!errs.is_empty(), "custom provider must reject");
        let (_, errs, _) = validate_hire_manifest(&json!({
            "spec": "agent-office/hire@1", "name": "n", "model": "x; rm -rf /"
        }));
        assert!(!errs.is_empty(), "model metachars must reject");
    }

    #[test]
    fn legacy_spec_tag_is_accepted_and_normalized() {
        let (m, errs, _) = validate_hire_manifest(&json!({
            "spec": LEGACY_HIRE_SPEC_V1, "name": "Old Hire"
        }));
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(m.unwrap()["spec"], HIRE_SPEC_V1);
        let (m, errs, _) = validate_hire_manifest(&json!({ "spec": "nope/hire@1", "name": "x" }));
        assert!(m.is_none() && !errs.is_empty());
    }

    #[test]
    fn deep_link_and_url_rules() {
        assert_eq!(
            parse_hire_deep_link("agent-office://hire?src=https%3A%2F%2Fex.com%2Fm.json"),
            Some("https://ex.com/m.json".to_string())
        );
        assert!(parse_hire_deep_link("agent-office://spawn?src=x").is_none());        assert!(is_allowed_manifest_url("https://ex.com/a.json"));
        assert!(is_allowed_manifest_url("http://localhost:8000/a.json"));
        assert!(!is_allowed_manifest_url("http://ex.com/a.json"));
        assert!(!is_allowed_manifest_url("ftp://ex.com"));
    }

    #[test]
    fn ssrf_bypass_strings_are_rejected() {
        for bad in [
            // userinfo smuggling: the real host is what follows the '@'
            "https://example.com:x@10.0.0.5/m.json",
            "https://user:pw@example.com/m.json",
            "http://localhost:80@169.254.169.254/latest/meta-data",
            "http://127.0.0.1@169.254.169.254/",
            // http is loopback-only
            "http://169.254.169.254/",
            "http://10.0.0.5/m.json",
            "http://localhost.evil.com/m.json",
            "http://127.0.0.1.evil.com/m.json",
            // parser-differential characters
            "https://exa\\mple.com/m.json",
            "https://example.com\\@10.0.0.5/",
            "https://example.com/\r\nHost: 10.0.0.5",
            "https://exa mple.com/",
            "https://ex%2eample.com/",
            "https://exämple.com/",
            // malformed authority/port
            "https://example.com:99999/",
            "https://example.com:0/",
            "https://example.com:/m.json",
            "https://example.com:80:80/",
            "https://:443/m.json",
            "https:///m.json",
            "https://[::1/m.json",
            "https://[not-ip]/m.json",
        ] {
            assert!(!is_allowed_manifest_url(bad), "must reject: {bad:?}");
        }
        // Still fine: https hosts/ports/IPv6 literal, loopback http for dev.
        for ok in [
            "https://example.com/m.json",
            "https://example.com:8443/m.json?x=1#f",
            "HTTPS://Example.COM/m.json",
            "http://localhost:8000/a.json",
            "http://127.0.0.1/a.json",
            "http://[::1]:9000/a.json",
        ] {
            assert!(is_allowed_manifest_url(ok), "must accept: {ok:?}");
        }
        let p = parse_manifest_url("https://Example.com:8443/x").unwrap();
        assert_eq!((p.host.as_str(), p.port, p.https), ("example.com", 8443, true));
    }

    #[test]
    fn blocklist_covers_internal_ranges_for_the_guard() {
        for ip in ["127.0.0.1", "10.0.0.5", "169.254.169.254", "192.168.1.1", "100.64.0.1", "::1", "fe80::1", "fd00::1", "::ffff:10.0.0.5"] {
            assert!(ip_blocked(&ip.parse().unwrap()), "{ip} must be blocked");
        }
        assert!(!ip_blocked(&"93.184.216.34".parse().unwrap()));
        // loopback-only mode (http dev) accepts ONLY loopback, even for "localhost"
        let g = GuardedResolver { host: "localhost".into(), loopback_only: true };
        assert!(g.ip_ok(&"127.0.0.1".parse().unwrap()));
        assert!(g.ip_ok(&"::1".parse().unwrap()));
        assert!(!g.ip_ok(&"10.0.0.5".parse().unwrap()));
        assert!(!g.ip_ok(&"169.254.169.254".parse().unwrap()));
        let g = GuardedResolver { host: "example.com".into(), loopback_only: false };
        assert!(!g.ip_ok(&"127.0.0.1".parse().unwrap()));
        assert!(g.ip_ok(&"93.184.216.34".parse().unwrap()));
    }

    #[test]
    fn fetch_refuses_blocked_targets_without_network() {
        for u in [
            "http://169.254.169.254/latest/meta-data",
            "https://10.0.0.5/m.json",
            "https://127.0.0.1:1/m.json",
            "http://localhost:80@169.254.169.254/",
        ] {
            assert!(fetch_manifest(u).is_err(), "{u}");
        }
    }

    #[test]
    fn ssrf_blocklist_covers_private_and_metadata() {
        use std::net::IpAddr;
        for ip in [
            "10.0.0.5",
            "172.16.0.1",
            "192.168.1.1",
            "127.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(
                ip_blocked(&ip.parse::<IpAddr>().unwrap()),
                "{ip} should be blocked"
            );
        }
        for ip in ["8.8.8.8", "1.1.1.1", "100.63.255.255"] {
            assert!(
                !ip_blocked(&ip.parse::<IpAddr>().unwrap()),
                "{ip} should pass"
            );
        }
    }
}
