//! Integrations registry + secret store + loopback secret broker — port of
//! `src/main/integrations.ts`, `src/main/integrationBroker.ts`, and
//! `src/shared/integrations.ts`.
//!
//!   1. Registry — JSON-backed CRUD over IntegrationRecord metadata (NO secrets).
//!      DEVIATION: upstream kept records in config.json's `integrations` array;
//!      the Rust Config struct has no such field (a save would drop it), so
//!      records persist to `<harness>/integrations/records.json` instead.
//!   2. Secret store — OS keychain via the `keyring` crate, service
//!      "agent-office", key `INTEGRATION_<id>_<key>` (upstream used Electron
//!      safeStorage into a separate encrypted blob file).
//!   3. Broker — a 127.0.0.1-only tiny_http proxy. A worker calls
//!        <METHOD> http://127.0.0.1:<port>/i/<integrationId>/<path...>
//!      authenticating with a PER-WORKER CAPABILITY TOKEN (a handle, NOT any
//!      secret) in `Authorization: Bearer` / `X-MD-Broker-Token`. The broker
//!      validates the token, authorizes the integration, pulls the real secret
//!      from the keychain, injects it as the upstream auth header, forwards to
//!      the integration's baseUrl, and streams the response back. The worker
//!      uses the integration WITHOUT EVER SEEING the credential.
//!
//! Security invariants preserved: 127.0.0.1 bind only, loopback rechecked
//! per request, token-gated (constant-time compare via crate::webhook::secret_eq),
//! secrets never returned to the renderer / logged / echoed in errors, NOT an
//! open proxy (path confined under the integration origin).

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::webhook::secret_eq;

const MAX_BODY_BYTES: usize = 10 * 1024 * 1024; // 10 MB request-body cap
const UPSTREAM_TIMEOUT_MS: u64 = 30_000;
const TEST_TIMEOUT_MS: u64 = 15_000;
const DEFAULT_USER_AGENT: &str = "munder-difflin-broker/1";
const KEYRING_SERVICE: &str = "agent-office";

/// Hop-by-hop headers never forwarded in either direction.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];
/// Request headers the worker is NEVER allowed to pass upstream.
fn strip_request(name: &str) -> bool {
    matches!(
        name,
        "authorization" | "x-md-broker-token" | "host" | "cookie" | "content-length"
    ) || HOP_BY_HOP.contains(&name)
}
/// Header names an `authType: "header"` integration may NOT inject — they
/// frame the request itself (smuggling / host-confusion), or are the broker's
/// own credential/hop-by-hop plumbing.
fn reserved_auth_header(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    matches!(
        l.as_str(),
        "host" | "content-length" | "x-md-broker-token" | "cookie"
    ) || HOP_BY_HOP.contains(&l.as_str())
}
/// Response headers never forwarded back (the client decodes the body, so the
/// upstream content-encoding/length would be wrong).
fn strip_response(name: &str) -> bool {
    matches!(name, "content-encoding" | "content-length" | "set-cookie")
        || HOP_BY_HOP.contains(&name)
}

// ─── shared/integrations.ts — schema + validation ────────────────────────────

/// A registered integration. METADATA ONLY — never the secret value, only a
/// `secretRef` handle. Field names match upstream's camelCase on disk/IPC.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IntegrationRecord {
    pub id: String,
    pub label: String,
    /// 'github' | 'custom-rest'
    pub kind: String,
    pub base_url: String,
    /// 'none' | 'bearer' | 'header' | 'github'
    pub auth_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_header: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<String>,
    pub enabled: bool,
    /// epoch ms
    pub created_at: u64,
    /// epoch ms
    pub updated_at: u64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// INTEGRATION_SLUG_RE = /^[a-z0-9](?:[a-z0-9-]{0,38}[a-z0-9])?$/
fn valid_slug(id: &str) -> bool {
    let b = id.as_bytes();
    if b.is_empty() || b.len() > 40 {
        return false;
    }
    let alnum = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    if !alnum(b[0]) || !alnum(b[b.len() - 1]) {
        return false;
    }
    b.iter().all(|&c| alnum(c) || c == b'-')
}

/// HEADER_NAME_RE = /^[A-Za-z0-9-]{1,64}$/
fn valid_header_name(h: &str) -> bool {
    !h.is_empty() && h.len() <= 64 && h.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
}

const ALL_AUTH_TYPES: &[&str] = &["none", "bearer", "header", "github"];
const ALL_KINDS: &[&str] = &["github", "custom-rest"];

/// The secretRef handle for an integration id (1:1).
pub fn secret_ref_for(id: &str) -> String {
    format!("int:{id}")
}

/// True iff this auth type needs a stored secret.
pub fn auth_type_needs_secret(t: &str) -> bool {
    t != "none"
}

/// Minimal absolute-URL parser (no `url` crate): scheme://authority/path.
/// Rejects userinfo-bearing authorities only via `has_userinfo` (validate
/// decides), tolerates trailing path/query/fragment (query/fragment split off).
#[derive(Clone, Debug)]
pub struct ParsedUrl {
    pub scheme: String,
    /// host[:port] as it appeared, without userinfo.
    pub authority: String,
    /// lowercase host, brackets stripped for IPv6.
    pub host: String,
    /// path beginning with '/' ('/' when absent). Query/fragment split off.
    pub path: String,
    pub has_userinfo: bool,
}

pub fn parse_url(s: &str) -> Option<ParsedUrl> {
    let (scheme, rest) = s.split_once("://")?;
    let scheme_lc = scheme.to_lowercase();
    if scheme_lc.is_empty()
        || !scheme_lc.bytes().enumerate().all(|(i, c)| {
            (i == 0 && c.is_ascii_lowercase())
                || (i > 0
                    && (c.is_ascii_lowercase()
                        || c.is_ascii_digit()
                        || matches!(c, b'+' | b'.' | b'-')))
        })
    {
        return None;
    }
    // authority = up to first '/', '?', or '#'
    let auth_end = rest
        .find(|c| c == '/' || c == '?' || c == '#')
        .unwrap_or(rest.len());
    let authority_raw = &rest[..auth_end];
    if authority_raw.is_empty() {
        return None;
    }
    let has_userinfo = authority_raw.contains('@');
    let authority = authority_raw
        .rsplit('@')
        .next()
        .unwrap_or(authority_raw)
        .to_string();
    // host = authority minus :port (IPv6 [::1]:port handled via the brackets)
    let host = if authority.starts_with('[') {
        authority
            .split(']')
            .next()
            .unwrap_or("")
            .trim_start_matches('[')
            .to_string()
    } else {
        authority
            .rsplit_once(':')
            .map(|(h, _)| h.to_string())
            .unwrap_or_else(|| authority.clone())
    }
    .to_lowercase();
    if host.is_empty() {
        return None;
    }
    let tail = &rest[auth_end..];
    let path = {
        let p = tail.split(|c| c == '?' || c == '#').next().unwrap_or("");
        if p.is_empty() {
            "/".to_string()
        } else {
            p.to_string()
        }
    };
    Some(ParsedUrl {
        scheme: scheme_lc,
        authority,
        host,
        path,
        has_userinfo,
    })
}

/// Validate a baseUrl: https origin (+ optional path), no userinfo, no
/// traversal. A loopback http origin is permitted for explicit local targets.
pub fn validate_base_url(base_url: &str) -> Result<ParsedUrl, String> {
    if base_url.trim().is_empty() {
        return Err("baseUrl is required".into());
    }
    let u = parse_url(base_url.trim()).ok_or_else(|| "baseUrl must be a valid URL".to_string())?;
    if u.has_userinfo {
        return Err("baseUrl must not contain userinfo".into());
    }
    if base_url.contains('?') || base_url.contains('#') {
        return Err("baseUrl must not contain a query or fragment".into());
    }
    if base_url.contains("..") {
        return Err("baseUrl must not contain \"..\"".into());
    }
    let is_loopback_host = u.host == "127.0.0.1" || u.host == "::1" || u.host == "localhost";
    if u.scheme == "https" {
        return Ok(u);
    }
    if u.scheme == "http" && is_loopback_host {
        return Ok(u);
    }
    Err("baseUrl must be https (http allowed only for 127.0.0.1/localhost)".into())
}

/// Validate an integration record (the upsert gate). Fail closed — anything
/// not explicitly allowed is rejected. createdAt/updatedAt are stamped by the
/// registry, so they are not required on input.
pub fn validate_record(rec: &Value) -> Result<IntegrationRecord, String> {
    let r = rec
        .as_object()
        .ok_or_else(|| "record must be an object".to_string())?;
    let str_of = |k: &str| -> String {
        r.get(k)
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    let id = str_of("id");
    if !valid_slug(&id) {
        return Err(
            "id must be a lowercase slug (2–40 chars, a–z 0–9 -, no leading/trailing hyphen)"
                .into(),
        );
    }
    let label = str_of("label");
    if label.is_empty() || label.chars().count() > 60 {
        return Err("label is required and must be <= 60 chars".into());
    }
    let kind = str_of("kind");
    if !ALL_KINDS.contains(&kind.as_str()) {
        return Err(format!("kind must be one of {}", ALL_KINDS.join(", ")));
    }
    let auth_type = str_of("authType");
    if !ALL_AUTH_TYPES.contains(&auth_type.as_str()) {
        return Err(format!(
            "authType must be one of {}",
            ALL_AUTH_TYPES.join(", ")
        ));
    }
    validate_base_url(&str_of("baseUrl"))?;
    let auth_header = if auth_type == "header" {
        let h = str_of("authHeader");
        if h.is_empty() || !valid_header_name(&h) {
            return Err("authType 'header' requires authHeader matching [A-Za-z0-9-]{1,64}".into());
        }
        if reserved_auth_header(&h) {
            return Err(format!("authHeader '{h}' is reserved and cannot carry the credential"));
        }
        Some(h)
    } else {
        match r.get("authHeader") {
            Some(v) if !v.is_null() && v.as_str().map(|s| !s.trim().is_empty()).unwrap_or(true) => {
                return Err("authHeader is only valid when authType === 'header'".into());
            }
            _ => None,
        }
    };
    let needs_secret = auth_type_needs_secret(&auth_type);
    Ok(IntegrationRecord {
        id,
        label,
        kind,
        base_url: str_of("baseUrl"),
        auth_type,
        auth_header,
        secret_ref: needs_secret.then(|| secret_ref_for(&str_of("id"))),
        enabled: r.get("enabled").and_then(|e| e.as_bool()).unwrap_or(false),
        created_at: 0,
        updated_at: 0,
    })
}

/// The upstream auth headers the broker injects when forwarding (lowercased
/// keys, injected auth ALWAYS wins at merge time).
pub fn build_auth_headers(
    auth_type: &str,
    auth_header: Option<&str>,
    secret: Option<&str>,
) -> Vec<(String, String)> {
    match auth_type {
        "bearer" => match secret {
            Some(s) if !s.is_empty() => {
                vec![("authorization".into(), format!("Bearer {s}"))]
            }
            _ => vec![],
        },
        "header" => match (secret, auth_header) {
            (Some(s), Some(h)) if !s.is_empty() && !h.is_empty() && !reserved_auth_header(h) => {
                vec![(h.to_lowercase(), s.to_string())]
            }
            _ => vec![],
        },
        "github" => {
            let mut v = vec![
                ("accept".into(), "application/vnd.github+json".to_string()),
                ("x-github-api-version".into(), "2022-11-28".to_string()),
            ];
            if let Some(s) = secret.filter(|s| !s.is_empty()) {
                v.insert(0, ("authorization".into(), format!("Bearer {s}")));
            }
            v
        }
        _ => vec![],
    }
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// decodeURIComponent — %XX → bytes → UTF-8. None on malformed input
/// ('+' is NOT decoded, matching decodeURIComponent semantics).
pub fn pct_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if i + 2 >= b.len() {
                return None;
            }
            out.push(hex_val(b[i + 1])? * 16 + hex_val(b[i + 2])?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Resolve `.`/`..` segments like WHATWG URL parsing (empty segments — `//` —
/// are preserved, as `new URL` does; `..` is already rejected upstream of this).
fn normalize_url_path(path: &str) -> String {
    let trailing_dot = path.ends_with("/.") || path.ends_with("/..");
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            ".." => {
                if out.len() > 1 {
                    out.pop();
                }
            }
            "." => {}
            _ => out.push(seg),
        }
    }
    let mut s = out.join("/");
    if !s.starts_with('/') {
        s.insert(0, '/');
    }
    if trailing_dot && !s.ends_with('/') {
        s.push('/');
    }
    s
}

/**
 * Join an integration baseUrl with a worker-supplied path, confining the result
 * under the baseUrl origin (NOT an open proxy). Returns None if the path would
 * escape the origin (absolute URL, host override, or traversal above the base
 * path). `path_and_query` is the part after `/i/<integrationId>/` including any
 * query string.
 */
pub fn resolve_upstream_url(base_url: &str, path_and_query: &str) -> Option<String> {
    let base = parse_url(base_url)?;
    // Reject anything that smells like an absolute target or traversal.
    let pq = path_and_query;
    // /^[a-z][a-z0-9+.-]*:\/\//i
    if let Some(colon) = pq.find("://") {
        let scheme = &pq[..colon];
        if !scheme.is_empty()
            && scheme.bytes().enumerate().all(|(i, c)| {
                (i == 0 && c.is_ascii_alphabetic())
                    || (i > 0 && (c.is_ascii_alphanumeric() || matches!(c, b'+' | b'.' | b'-')))
            })
        {
            return None;
        }
    }
    if pq.starts_with("//") {
        return None; // protocol-relative host override
    }
    // Decode the path before the traversal check so an encoded %2e%2e is caught.
    let path_only = pq.split(|c| c == '?' || c == '#').next().unwrap_or("");
    let decoded_path = pct_decode(path_only)?;
    if decoded_path.split('/').any(|seg| seg == "..") {
        return None;
    }

    // Normalize the base path to a directory prefix, then append the worker path.
    let base_path = if base.path.ends_with('/') {
        base.path.clone()
    } else {
        format!("{}/", base.path)
    };
    let rel = pq.trim_start_matches('/');
    let (rel_path, rel_query) = match rel.find('?') {
        Some(i) => (&rel[..i], &rel[i..]),
        None => (rel, ""),
    };
    let rel_query = rel_query.split('#').next().unwrap_or("");
    let joined = format!("{base_path}{rel_path}");
    let resolved_path = normalize_url_path(&joined);

    // Confine: the resolved path stays under the base path prefix.
    if base_path != "/"
        && !format!("{resolved_path}/").starts_with(&base_path)
        && resolved_path != base.path
    {
        return None;
    }
    Some(format!(
        "{}://{}{}{}",
        base.scheme, base.authority, resolved_path, rel_query
    ))
}

// ─── Registry (file-backed, <harness>/integrations/records.json) ────────────

pub fn records_path(harness_home: &Path) -> PathBuf {
    harness_home.join("integrations").join("records.json")
}

pub fn list_records(harness_home: &Path) -> Vec<IntegrationRecord> {
    fs_read(harness_home)
}

fn fs_read(harness_home: &Path) -> Vec<IntegrationRecord> {
    std::fs::read_to_string(records_path(harness_home))
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<IntegrationRecord>>(&t).ok())
        .unwrap_or_default()
}

fn fs_write(harness_home: &Path, records: &[IntegrationRecord]) -> Result<(), String> {
    let p = records_path(harness_home);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("integrations dir: {e}"))?;
    }
    let body = serde_json::to_string_pretty(records).map_err(|e| e.to_string())?;
    std::fs::write(&p, body).map_err(|e| format!("write records: {e}"))
}

/// Look up one record by id.
pub fn get_record(harness_home: &Path, id: &str) -> Option<IntegrationRecord> {
    fs_read(harness_home).into_iter().find(|r| r.id == id)
}

/// Ids of integrations a worker may use right now (enabled, and — for secret
/// auth — actually holding a stored secret). The default capability scope.
pub fn enabled_ids(harness_home: &Path) -> Vec<String> {
    fs_read(harness_home)
        .into_iter()
        .filter(|r| {
            r.enabled
                && (!auth_type_needs_secret(&r.auth_type) || has_secret(r.secret_ref.as_deref()))
        })
        .map(|r| r.id)
        .collect()
}

/// Create or replace a record (validated). Stamps createdAt/updatedAt;
/// preserves the original createdAt on update. Does NOT touch the secret store.
pub fn upsert_record(harness_home: &Path, input: &Value) -> Result<IntegrationRecord, String> {
    let mut v = validate_record(input)?;
    let now = now_ms();
    let mut records = fs_read(harness_home);
    let existing = records.iter().find(|r| r.id == v.id);
    v.created_at = existing.map(|e| e.created_at).unwrap_or(now);
    v.updated_at = now;
    records.retain(|r| r.id != v.id);
    records.push(v.clone());
    fs_write(harness_home, &records)?;
    Ok(v)
}

/// Remove a record AND its stored secret.
pub fn remove_record(harness_home: &Path, id: &str) -> Result<(), String> {
    let mut records = fs_read(harness_home);
    records.retain(|r| r.id != id);
    fs_write(harness_home, &records)?;
    delete_secret(&secret_ref_for(id));
    Ok(())
}

/// Records with secretRef redacted to a boolean — the renderer-safe shape.
pub fn list_redacted(harness_home: &Path) -> Vec<Value> {
    fs_read(harness_home)
        .into_iter()
        .map(|r| {
            let has = r
                .secret_ref
                .as_deref()
                .map(|s| !s.is_empty() && has_secret(Some(s)))
                .unwrap_or(false);
            let mut v = serde_json::to_value(&r).unwrap_or(json!({}));
            if let Some(o) = v.as_object_mut() {
                o.remove("secretRef");
                o.insert("hasSecret".into(), json!(has));
            }
            v
        })
        .collect()
}

// ─── Secret store (OS keychain, write-only from the renderer's side) ────────

/// Keychain key for a secret slot: `INTEGRATION_<id>_<key>` — id and key
/// uppercased, hyphens → underscores (safe in every credential backend).
pub fn keyring_key(integration_id: &str, key: &str) -> String {
    let norm = |s: &str| s.to_ascii_uppercase().replace('-', "_");
    format!("INTEGRATION_{}_{}", norm(integration_id), norm(key))
}

fn key_for_ref(secret_ref: &str) -> Option<String> {
    let id = secret_ref.strip_prefix("int:")?;
    Some(keyring_key(id, "secret"))
}

/// Store a secret under an explicit integration id + slot key
/// (`INTEGRATION_<id>_<key>`). A failure to reach the keychain fails closed
/// (never a plaintext fallback).
pub fn set_secret_key(integration_id: &str, key: &str, plaintext: &str) -> Result<(), String> {
    if integration_id.is_empty() {
        return Err("id required".into());
    }
    if key.is_empty() {
        return Err("secret key required".into());
    }
    if plaintext.is_empty() {
        return Err("secret required".into());
    }
    keyring::Entry::new(KEYRING_SERVICE, &keyring_key(integration_id, key))
        .and_then(|e| e.set_password(plaintext))
        .map_err(|e| format!("keychain: {e}"))
}

/// Store a secret by ref (`int:<id>` → the default "secret" slot — upstream
/// stored exactly one secret per ref). Fails closed on a keychain error.
pub fn set_secret(secret_ref: &str, plaintext: &str) -> Result<(), String> {
    if secret_ref.is_empty() {
        return Err("secretRef required".into());
    }
    if plaintext.is_empty() {
        return Err("secret required".into());
    }
    // `int:<id>` → its key slot; any other ref (e.g. upstream's `apikey:*`)
    // maps to a deterministic secret slot too.
    let k = key_for_ref(secret_ref).unwrap_or_else(|| keyring_key(secret_ref, "secret"));
    keyring::Entry::new(KEYRING_SERVICE, &k)
        .and_then(|e| e.set_password(plaintext))
        .map_err(|e| format!("keychain: {e}"))
}

fn key_for_any_ref(secret_ref: &str) -> Option<String> {
    if secret_ref.is_empty() {
        return None;
    }
    Some(key_for_ref(secret_ref).unwrap_or_else(|| keyring_key(secret_ref, "secret")))
}

/// Read a credential — MAIN-INTERNAL ONLY, never expose over IPC. Returns None
/// if absent or unreadable (the broker maps that to 503 no_secret).
pub fn get_secret(secret_ref: Option<&str>) -> Option<String> {
    let k = key_for_any_ref(secret_ref?)?;
    keyring::Entry::new(KEYRING_SERVICE, &k)
        .ok()
        .and_then(|e| e.get_password().ok())
}

/// Whether a secret is stored for this ref (no read of the value).
pub fn has_secret(secret_ref: Option<&str>) -> bool {
    let Some(k) = key_for_any_ref(secret_ref.unwrap_or("")) else {
        return false;
    };
    keyring::Entry::new(KEYRING_SERVICE, &k)
        .ok()
        .and_then(|e| e.get_password().ok())
        .is_some()
}

/// Delete a stored secret. Idempotent.
pub fn delete_secret(secret_ref: &str) {
    if let Some(k) = key_for_any_ref(secret_ref) {
        if let Ok(e) = keyring::Entry::new(KEYRING_SERVICE, &k) {
            let _ = e.delete_credential();
        }
    }
}

// ─── templates (src/shared/integrations.ts INTEGRATION_TEMPLATES) ───────────

pub fn templates() -> Value {
    json!([
        {
            "kind": "github",
            "label": "GitHub",
            "baseUrl": "https://api.github.com",
            "authType": "github",
            "secretLabel": "GitHub personal access token",
            "secretHelp": "Create a fine-grained or classic PAT at github.com/settings/tokens with the scopes your workers need.",
            "docsUrl": "https://docs.github.com/rest",
            "idSuggestion": "github"
        },
        {
            "kind": "custom-rest",
            "label": "Custom REST API",
            "baseUrl": "",
            "authType": "bearer",
            "secretLabel": "API key / token",
            "secretHelp": "Point baseUrl at any REST API. Choose how its credential is sent: Bearer token, a custom header, or none.",
            "idSuggestion": "my-api"
        },
        {
            "kind": "custom-rest",
            "label": "Linear",
            "baseUrl": "https://api.linear.app/graphql",
            "authType": "header",
            "authHeader": "Authorization",
            "secretLabel": "Linear API key",
            "secretHelp": "Linear → Settings → Security & access → Personal API keys. Sent verbatim in Authorization (no \"Bearer\"). Every call POSTs to /graphql.",
            "docsUrl": "https://developers.linear.app/docs/graphql/working-with-the-graphql-api",
            "idSuggestion": "linear"
        },
        {
            "kind": "custom-rest",
            "label": "Jira",
            "baseUrl": "https://your-domain.atlassian.net/rest/api/3",
            "authType": "header",
            "authHeader": "Authorization",
            "secretLabel": "Authorization header (Basic …)",
            "secretHelp": "Basic auth: paste \"Basic \" + base64(\"<email>:<api-token>\"). Token at id.atlassian.com → Security → API tokens. Replace your-domain with your Atlassian site.",
            "docsUrl": "https://developer.atlassian.com/cloud/jira/platform/rest/v3/intro/",
            "idSuggestion": "jira"
        },
        {
            "kind": "custom-rest",
            "label": "Notion",
            "baseUrl": "https://api.notion.com/v1",
            "authType": "bearer",
            "secretLabel": "Notion internal integration token",
            "secretHelp": "notion.so/my-integrations → Internal Integration Secret; share target pages/DBs with it. Every request also needs header \"Notion-Version: 2022-06-28\" (worker sends it per request).",
            "docsUrl": "https://developers.notion.com/reference/intro",
            "idSuggestion": "notion"
        },
        {
            "kind": "custom-rest",
            "label": "Stripe",
            "baseUrl": "https://api.stripe.com/v1",
            "authType": "bearer",
            "secretLabel": "Stripe secret key",
            "secretHelp": "dashboard.stripe.com → Developers → API keys → Secret key (sk_live_/sk_test_). Restricted keys recommended. Bodies are form-encoded, not JSON.",
            "docsUrl": "https://stripe.com/docs/api",
            "idSuggestion": "stripe"
        },
        {
            "kind": "custom-rest",
            "label": "Confluence",
            "baseUrl": "https://your-domain.atlassian.net/wiki/api/v2",
            "authType": "header",
            "authHeader": "Authorization",
            "secretLabel": "Authorization header (Basic …)",
            "secretHelp": "Basic auth: paste \"Basic \" + base64(\"<email>:<api-token>\") (same Atlassian token as Jira). Replace your-domain with your site.",
            "docsUrl": "https://developer.atlassian.com/cloud/confluence/rest/v2/intro/",
            "idSuggestion": "confluence"
        },
        {
            "kind": "custom-rest",
            "label": "Sentry",
            "baseUrl": "https://sentry.io/api/0",
            "authType": "bearer",
            "secretLabel": "Sentry auth token",
            "secretHelp": "sentry.io → Settings → Auth Tokens. Org-scoped routes carry your org slug in the path, e.g. /organizations/<org>/issues/.",
            "docsUrl": "https://docs.sentry.io/api/",
            "idSuggestion": "sentry"
        },
        {
            "kind": "custom-rest",
            "label": "HubSpot",
            "baseUrl": "https://api.hubapi.com",
            "authType": "bearer",
            "secretLabel": "HubSpot private app token",
            "secretHelp": "HubSpot → Settings → Integrations → Private Apps → create app → Access token (scopes crm.objects.*).",
            "docsUrl": "https://developers.hubspot.com/docs/api/crm/understanding-the-crm",
            "idSuggestion": "hubspot"
        }
    ])
}

// ─── the loopback secret broker ─────────────────────────────────────────────

#[derive(Clone)]
struct Capability {
    #[allow(dead_code)]
    worker_id: String,
    allowed_ids: HashSet<String>,
    #[allow(dead_code)]
    granted_at: u64,
}

struct BrokerInner {
    server: Option<Arc<tiny_http::Server>>,
    port: u16,
    home: Option<PathBuf>,
}

pub struct IntegrationBroker {
    inner: Mutex<BrokerInner>,
    stop: AtomicBool,
    /// token → capability. In-memory only; NEVER persisted.
    by_token: Mutex<HashMap<String, Capability>>,
    /// workerId → token, for revoke.
    by_worker: Mutex<HashMap<String, String>>,
}

/// Process-wide broker (upstream's module-level `new IntegrationBroker`).
/// LazyLock because HashMap::new isn't const — everything else behaves as a
/// plain static (`BROKER.start(...)`, `BROKER.grant(...)`).
pub static BROKER: std::sync::LazyLock<IntegrationBroker> =
    std::sync::LazyLock::new(|| IntegrationBroker {
        inner: Mutex::new(BrokerInner {
            server: None,
            port: 0,
            home: None,
        }),
        stop: AtomicBool::new(false),
        by_token: Mutex::new(HashMap::new()),
        by_worker: Mutex::new(HashMap::new()),
    });

/// True for IPv4 loopback and IPv6 ::1 (incl. v4-mapped). Defense in depth —
/// the bind already excludes non-loopback peers.
fn is_loopback(remote: Option<&std::net::SocketAddr>) -> bool {
    remote.map(|a| a.ip().is_loopback()).unwrap_or(false)
}

/// A random capability handle — 32 bytes, base64url (upstream
/// randomBytes(32).toString('base64url')). A handle, NEVER a secret.
fn mint_token() -> String {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

impl IntegrationBroker {
    /// Bind a loopback port (0 ⇒ OS-assigned). Idempotent — a bound broker
    /// just reports its port. The serve loop runs on a std thread.
    pub fn start(&self, home: &Path, preferred_port: u16) -> Result<u16, String> {
        {
            let inner = self.inner.lock().unwrap();
            if inner.server.is_some() {
                return Ok(inner.port);
            }
        }
        // 127.0.0.1 ONLY — never bound to a routable interface, never tunneled.
        let server = tiny_http::Server::http(("127.0.0.1", preferred_port))
            .map_err(|e| format!("broker bind :{preferred_port}: {e}"))?;
        let port = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
        };
        let server = Arc::new(server);
        {
            let mut inner = self.inner.lock().unwrap();
            inner.port = port;
            inner.home = Some(home.to_path_buf());
            inner.server = Some(server.clone());
        }
        self.stop.store(false, Ordering::Relaxed);
        let home = home.to_path_buf();
        std::thread::spawn(move || Self::serve(server, home));
        Ok(port)
    }

    /// Close the broker. Idempotent + best-effort. Clears all capabilities.
    pub fn stop_broker(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let mut inner = self.inner.lock().unwrap();
        if let Some(s) = inner.server.take() {
            s.unblock();
        }
        inner.port = 0;
        inner.home = None;
        drop(inner);
        self.by_token.lock().unwrap().clear();
        self.by_worker.lock().unwrap().clear();
    }

    /// Whether the broker is bound and serving.
    pub fn running(&self) -> bool {
        self.inner.lock().unwrap().server.is_some()
    }

    pub fn port(&self) -> u16 {
        self.inner.lock().unwrap().port
    }

    /// The base URL workers use (only valid while running).
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port())
    }

    /// Mint a per-worker capability token granting access to `allowed_ids`.
    /// Any prior token for this worker is revoked first.
    pub fn grant(&self, worker_id: &str, allowed_ids: Vec<String>) -> String {
        self.revoke(worker_id);
        let token = mint_token();
        self.by_token.lock().unwrap().insert(
            token.clone(),
            Capability {
                worker_id: worker_id.to_string(),
                allowed_ids: allowed_ids.into_iter().collect(),
                granted_at: now_ms(),
            },
        );
        self.by_worker
            .lock()
            .unwrap()
            .insert(worker_id.to_string(), token.clone());
        token
    }

    /// Revoke a worker's capability (called on teardown). Idempotent.
    pub fn revoke(&self, worker_id: &str) {
        let token = self.by_worker.lock().unwrap().remove(worker_id);
        if let Some(t) = token {
            self.by_token.lock().unwrap().remove(&t);
        }
    }

    pub fn capability_count(&self) -> usize {
        self.by_token.lock().unwrap().len()
    }

    /// Constant-time-ish lookup of a presented token against live capabilities.
    fn resolve_capability(&self, provided: &str) -> Option<Capability> {
        if provided.is_empty() {
            return None;
        }
        self.by_token
            .lock()
            .unwrap()
            .iter()
            .find(|(token, _)| secret_eq(provided, token))
            .map(|(_, cap)| cap.clone())
    }

    /// Serve loop — one std thread per connection, same shape as webhook.rs.
    /// Runs on the BROKER singleton (the only broker that exists).
    fn serve(server: Arc<tiny_http::Server>, home: PathBuf) {
        loop {
            if BROKER.stop.load(Ordering::Relaxed) {
                return;
            }
            let mut req = match server.recv_timeout(Duration::from_millis(500)) {
                Ok(Some(r)) => r,
                Ok(None) => continue,
                Err(_) => {
                    if BROKER.stop.load(Ordering::Relaxed) {
                        return;
                    }
                    continue;
                }
            };
            let h = home.clone();
            std::thread::spawn(move || {
                let (status, headers, body) = BROKER.handle(&mut req, &h);
                let mut resp = tiny_http::Response::from_data(body).with_status_code(status);
                for (k, v) in headers {
                    if let Ok(hdr) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                        resp = resp.with_header(hdr);
                    }
                }
                let _ = req.respond(resp);
            });
        }
    }

    fn send_error(status: u16, code: &str, message: &str) -> (u16, Vec<(String, String)>, Vec<u8>) {
        (
            status,
            vec![("content-type".into(), "application/json".into())],
            serde_json::to_vec(&json!({ "error": message, "code": code })).unwrap_or_default(),
        )
    }

    /// Bearer / X-MD-Broker-Token from the request.
    fn token_from(req: &tiny_http::Request) -> Option<String> {
        let x = req
            .headers()
            .iter()
            .find(|h| h.field.equiv("x-md-broker-token"))
            .map(|h| h.value.as_str().to_string());
        if let Some(x) = x.filter(|s| !s.is_empty()) {
            return Some(x);
        }
        let auth = req
            .headers()
            .iter()
            .find(|h| h.field.equiv("authorization"))
            .map(|h| h.value.as_str().to_string())?;
        let auth = auth.trim();
        // /^Bearer\s+(.+)$/i — first 7 bytes are ASCII so slicing is safe.
        if auth.len() > 7 && auth[..7].eq_ignore_ascii_case("bearer ") {
            let t = auth[7..].trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
        None
    }

    /// One request → (status, headers, body). The whole gate (loopback → token
    /// → capability → record → origin confinement → secret → forward) lives
    /// here so the failure paths are single-entrance.
    fn handle(
        &self,
        req: &mut tiny_http::Request,
        home: &Path,
    ) -> (u16, Vec<(String, String)>, Vec<u8>) {
        // 1) Loopback — defense in depth even though the bind excludes others.
        if !is_loopback(req.remote_addr()) {
            return Self::send_error(403, "forbidden", "loopback callers only");
        }
        // 2) Capability token.
        let Some(cap) = Self::token_from(req).and_then(|t| self.resolve_capability(&t)) else {
            return Self::send_error(401, "unauthorized", "missing or invalid capability token");
        };

        // 3) Parse /i/<integrationId>/<path...> — id is everything up to the
        //    next '/', '?' or '#'; the rest is path + query verbatim.
        let raw_url = req.url().to_string();
        let Some(rest) = raw_url.strip_prefix("/i/") else {
            return Self::send_error(404, "not_found", "expected /i/<integrationId>/<path>");
        };
        let id_end = rest
            .find(|c| c == '/' || c == '?' || c == '#')
            .unwrap_or(rest.len());
        let raw_id = &rest[..id_end];
        if raw_id.is_empty() {
            return Self::send_error(404, "not_found", "expected /i/<integrationId>/<path>");
        }
        let Some(integration_id) = pct_decode(raw_id) else {
            return Self::send_error(400, "bad_request", "invalid integration id");
        };
        let remainder = &rest[id_end..];
        let path_and_query: String = if let Some(pq) = remainder.strip_prefix('/') {
            pq.to_string()
        } else if remainder.starts_with('?') {
            remainder.to_string()
        } else {
            String::new()
        };

        // 4) Authorize against this worker's capability.
        if !cap.allowed_ids.contains(&integration_id) {
            return Self::send_error(
                403,
                "forbidden",
                "integration not in this worker capability",
            );
        }
        // 5) Resolve the record (still enabled?).
        let Some(rec) = get_record(home, &integration_id) else {
            return Self::send_error(404, "not_found", "unknown integration");
        };
        if !rec.enabled {
            return Self::send_error(403, "forbidden", "integration is disabled");
        }
        // 6) Confine the upstream URL under the integration origin.
        let Some(upstream) = resolve_upstream_url(&rec.base_url, &path_and_query) else {
            return Self::send_error(400, "bad_request", "invalid or out-of-bounds path");
        };
        // 7) Secret (materialized ONLY here, used ONLY to inject the header —
        //    never logged, never returned, never echoed in an error).
        let mut secret: Option<String> = None;
        if auth_type_needs_secret(&rec.auth_type) {
            secret = get_secret(rec.secret_ref.as_deref());
            if secret.is_none() {
                return Self::send_error(
                    503,
                    "no_secret",
                    "no secret configured for this integration",
                );
            }
        }

        self.forward(req, &rec, &upstream, secret.as_deref())
    }

    fn forward(
        &self,
        req: &mut tiny_http::Request,
        rec: &IntegrationRecord,
        upstream: &str,
        secret: Option<&str>,
    ) -> (u16, Vec<(String, String)>, Vec<u8>) {
        // Buffer the request body with a hard cap (write methods).
        let method = req.method().as_str().to_uppercase();
        let has_body = method != "GET" && method != "HEAD";
        let mut body: Vec<u8> = Vec::new();
        if has_body {
            let mut take = req.as_reader().take((MAX_BODY_BYTES + 1) as u64);
            if take.read_to_end(&mut body).is_err() {
                return Self::send_error(400, "bad_request", "could not read request body");
            }
            if body.len() > MAX_BODY_BYTES {
                return Self::send_error(413, "payload_too_large", "request body too large");
            }
        }

        // Sanitize worker headers, then inject the auth header(s). Injected
        // auth ALWAYS wins: the strip list removes any worker-supplied auth.
        let mut out_headers: Vec<(String, String)> = Vec::new();
        for h in req.headers() {
            let key = h.field.as_str().as_str().to_lowercase();
            if strip_request(&key) {
                continue;
            }
            if let Some((_, v)) = out_headers.iter_mut().find(|(k, _)| *k == key) {
                v.push_str(", ");
                v.push_str(h.value.as_str());
            } else {
                out_headers.push((key, h.value.as_str().to_string()));
            }
        }
        if !out_headers.iter().any(|(k, _)| k == "user-agent") {
            out_headers.push(("user-agent".into(), DEFAULT_USER_AGENT.into()));
        }
        for (k, v) in build_auth_headers(&rec.auth_type, rec.auth_header.as_deref(), secret) {
            out_headers.retain(|(ek, _)| *ek != k); // the worker can't shadow injected auth
            out_headers.push((k, v));
        }

        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_millis(UPSTREAM_TIMEOUT_MS)))
                .http_status_as_error(false)
                .max_redirects(0) // upstream fetch used redirect:'manual'
                .build(),
        );

        let mut rb = ureq::http::Request::builder()
            .method(method.as_str())
            .uri(upstream);
        for (k, v) in &out_headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        let upstream_res = if has_body {
            rb.body(body)
                .map_err(|e| e.to_string())
                .and_then(|r| agent.run(r).map_err(|e| e.to_string()))
        } else {
            rb.body(())
                .map_err(|e| e.to_string())
                .and_then(|r| agent.run(r).map_err(|e| e.to_string()))
        };
        let mut upstream_res = match upstream_res {
            Ok(r) => r,
            // The message could in theory contain the URL but never the secret.
            Err(e) => {
                return Self::send_error(
                    502,
                    "bad_gateway",
                    &format!("upstream request failed: {e}"),
                )
            }
        };

        // Stream the response back (status + sanitized headers + body).
        let status = upstream_res.status().as_u16();
        let mut resp_headers: Vec<(String, String)> = Vec::new();
        for (name, value) in upstream_res.headers().iter() {
            let key = name.as_str().to_lowercase();
            if strip_response(&key) {
                continue;
            }
            if let Ok(v) = value.to_str() {
                resp_headers.push((key, v.to_string()));
            }
        }
        // A read error or an over-cap body must surface as a failure, not as
        // the upstream status carrying a silently empty/truncated body.
        let body = match upstream_res
            .body_mut()
            .with_config()
            .limit((MAX_BODY_BYTES * 2) as u64)
            .read_to_vec()
        {
            Ok(b) => b,
            Err(e) => {
                return Self::send_error(
                    502,
                    "bad_gateway",
                    &format!("upstream response unreadable or too large: {e}"),
                )
            }
        };
        (status, resp_headers, body)
    }
}

// ─── integrations:test (index.ts — probe through the same auth path) ────────

/// Probe an integration's reachability through the broker's own auth path.
/// Confines the probe path through the SAME gate as forward() BEFORE the
/// secret is materialized. Secret is used but never returned — only status.
pub fn test_integration(harness_home: &Path, id: &str, path: &str) -> Value {
    if id.is_empty() {
        return json!({ "ok": false, "error": "id required" });
    }
    let Some(rec) = get_record(harness_home, id) else {
        return json!({ "ok": false, "error": "unknown integration" });
    };
    if let Err(e) = validate_base_url(&rec.base_url) {
        return json!({ "ok": false, "error": e });
    }
    let Some(target) = resolve_upstream_url(&rec.base_url, path) else {
        return json!({ "ok": false, "error": "path escapes the integration baseUrl", "code": "bad_request" });
    };
    let secret = get_secret(rec.secret_ref.as_deref());
    let headers = build_auth_headers(
        &rec.auth_type,
        rec.auth_header.as_deref(),
        secret.as_deref(),
    );
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_millis(TEST_TIMEOUT_MS)))
            .http_status_as_error(false)
            .max_redirects(0)
            .build(),
    );
    let mut rb = ureq::http::Request::builder().method("GET").uri(&target);
    for (k, v) in &headers {
        rb = rb.header(k.as_str(), v.as_str());
    }
    match rb
        .body(())
        .map_err(|e| e.to_string())
        .and_then(|r| agent.run(r).map_err(|e| e.to_string()))
    {
        Ok(resp) => {
            let status = resp.status().as_u16();
            json!({ "ok": resp.status().is_success(), "status": status })
        }
        Err(e) => json!({ "ok": false, "error": e }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_and_header_validation() {
        assert!(valid_slug("github"));
        assert!(valid_slug("a"));
        assert!(valid_slug("my-api-2"));
        assert!(!valid_slug("-bad"));
        assert!(!valid_slug("bad-"));
        assert!(!valid_slug("UPPER"));
        assert!(!valid_slug("has space"));
        assert!(!valid_slug(&"x".repeat(41)));
        assert!(valid_header_name("X-Api-Key"));
        assert!(!valid_header_name("has space"));
        assert!(!valid_header_name(""));
    }

    #[test]
    fn base_url_gate() {
        assert!(validate_base_url("https://api.github.com").is_ok());
        assert!(validate_base_url("https://api.github.com/v3").is_ok());
        assert!(validate_base_url("http://127.0.0.1:8080/api").is_ok());
        assert!(validate_base_url("http://localhost").is_ok());
        assert!(validate_base_url("http://example.com").is_err()); // non-loopback http
        assert!(validate_base_url("https://user:pw@api.x.com").is_err()); // userinfo
        assert!(validate_base_url("https://api.x.com/?a=b").is_err()); // query
        assert!(validate_base_url("https://api.x.com/a/../b").is_err()); // traversal
        assert!(validate_base_url("").is_err());
        assert!(validate_base_url("notaurl").is_err());
    }

    #[test]
    fn upstream_url_confinement() {
        let base = "https://api.github.com";
        assert_eq!(
            resolve_upstream_url(base, "repos/a/b"),
            Some("https://api.github.com/repos/a/b".to_string())
        );
        assert_eq!(
            resolve_upstream_url(base, "repos/a?x=1"),
            Some("https://api.github.com/repos/a?x=1".to_string())
        );
        // base path stays a prefix
        let with_base = "https://api.notion.com/v1";
        assert_eq!(
            resolve_upstream_url(with_base, "users"),
            Some("https://api.notion.com/v1/users".to_string())
        );
        assert!(resolve_upstream_url(with_base, "../evil").is_none());
        assert!(resolve_upstream_url(with_base, "%2e%2e/evil").is_none());
        assert!(resolve_upstream_url(with_base, "https://evil.com/x").is_none());
        assert!(resolve_upstream_url(with_base, "//evil.com/x").is_none());
        // escaping the base prefix fails even without '..' (normalize can't
        // produce it anyway, but the confine check is the last word)
        assert!(resolve_upstream_url(with_base, "..%2Fevil").is_none());
    }

    #[test]
    fn auth_headers() {
        assert_eq!(
            build_auth_headers("bearer", None, Some("tok")),
            vec![("authorization".to_string(), "Bearer tok".to_string())]
        );
        assert!(build_auth_headers("bearer", None, None).is_empty());
        assert_eq!(
            build_auth_headers("header", Some("X-Key"), Some("s3cr3t")),
            vec![("x-key".to_string(), "s3cr3t".to_string())]
        );
        let gh = build_auth_headers("github", None, Some("t"));
        assert!(gh
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer t"));
        assert!(gh.iter().any(|(k, _)| k == "x-github-api-version"));
        assert!(build_auth_headers("none", None, Some("x")).is_empty());
    }

    #[test]
    fn framing_headers_cannot_be_the_auth_header() {
        for h in ["Host", "content-length", "Transfer-Encoding", "Connection", "X-MD-Broker-Token", "cookie"] {
            assert!(reserved_auth_header(h), "{h}");
            assert!(build_auth_headers("header", Some(h), Some("s")).is_empty(), "{h}");
            let r = validate_record(&json!({
                "id": "x1", "label": "X", "kind": "custom-rest",
                "baseUrl": "https://api.example.com", "authType": "header", "authHeader": h,
            }));
            assert!(r.is_err(), "{h} must be rejected at save time");
        }
        assert!(!reserved_auth_header("X-Api-Key"));
    }

    #[test]
    fn pct_decode_and_record_validation() {
        assert_eq!(pct_decode("a%20b"), Some("a b".to_string()));
        assert_eq!(pct_decode("%2e%2e"), Some("..".to_string()));
        assert_eq!(pct_decode("bad%2"), None);
        assert_eq!(pct_decode("a+b"), Some("a+b".to_string())); // '+' NOT decoded

        let ok = validate_record(&json!({
            "id": "github", "label": "GitHub", "kind": "github",
            "baseUrl": "https://api.github.com", "authType": "github",
            "enabled": true
        }))
        .unwrap();
        assert_eq!(ok.secret_ref.as_deref(), Some("int:github"));
        assert!(ok.enabled);

        assert!(validate_record(&json!({"id": "bad id"})).is_err());
        // 'header' needs authHeader
        assert!(validate_record(&json!({
            "id": "x", "label": "X", "kind": "custom-rest",
            "baseUrl": "https://x.com", "authType": "header"
        }))
        .is_err());
        // authHeader on non-'header' rejected
        assert!(validate_record(&json!({
            "id": "x", "label": "X", "kind": "custom-rest",
            "baseUrl": "https://x.com", "authType": "bearer",
            "authHeader": "X-Key"
        }))
        .is_err());
        // 'none' → no secretRef
        let none_rec = validate_record(&json!({
            "id": "pub", "label": "Public", "kind": "custom-rest",
            "baseUrl": "https://x.com", "authType": "none"
        }))
        .unwrap();
        assert!(none_rec.secret_ref.is_none());
    }
}
