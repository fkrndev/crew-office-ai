//! WebhookServer — port of `src/main/webhook.ts` + the index.ts dispatch half:
//! a secret-gated inbound HTTP API that turns external posts into hive work.
//!
//!   POST /<webhookId>  + `x-md-webhook-secret: <secret>` → route to god
//!     (mode allows → 200 `{ok,token,taskId}`) or hold for the operator
//!     (→ 202 `{ok,pending:true,status:'awaiting-approval',token,detail}`).
//!   POST /  (bare)     → alias for the endpoint id 'legacy'.
//!   GET  /<webhookId>  + `x-md-webhook-token` (or `?token=`) → ONLY that
//!     token's task status: `{ok,status,title,result}`.
//!
//! Security contract preserved verbatim:
//!   - constant-time secret comparison, length-guarded, per endpoint
//!   - an UNKNOWN endpoint id answers exactly like a WRONG secret (decoy
//!     compare so the failure path does the same work) — no id enumeration
//!   - secrets never logged/echoed/forwarded into routed messages, cards, or
//!     ledger rows (the dispatch layer sees {id,name} only)
//!   - the capability token (192-bit) is returned ONCE in the POST response;
//!     only its SHA-256 digest is ever stored (kanban card / held-token map)
//!   - 1MB body cap, global + per-endpoint fixed-window rate limits, and one
//!     shared bucket for unknown ids
//!
//! Deviation: loopback only (upstream additionally tunnels via tunnelmole) —
//! an operator can point ngrok/cloudflared at the port for public ingress.

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{Emitter, Manager};

const MAX_BODY_BYTES: usize = 1024 * 1024;
const RATE_LIMIT: u32 = 120;
const PER_ENDPOINT_RATE_LIMIT: u32 = 60;
const RATE_WINDOW: Duration = Duration::from_secs(60);
const UNKNOWN_BUCKET: &str = ":unknown";
/// Bare `POST /` keeps serving the endpoint the pre-multi-endpoint migration
/// parked under this id (webhook.ts LEGACY_ENDPOINT_ID).
const LEGACY_ENDPOINT_ID: &str = "legacy";
/// persist kv key — tokenHash → pending history entry id, mirrored into the
/// durable store so a restart doesn't 404 a caller still politely waiting.
const HELD_TOKENS_KV_KEY: &str = "triggers.webhook.heldTokens";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WebhookEndpoint {
    pub id: String,
    pub name: String,
    /// Shared secret echoed in `x-md-webhook-secret`. Never leaves this class.
    pub secret: String,
    /// User-editable JSON Schema the inbound body is checked against (empty =
    /// the built-in `{message,title?,kind?,from?}` shape only).
    pub schema: String,
    /// TriggerMode: 'strict' | 'allow-all' | 'communication-only' (legacy
    /// 'auto'/'review' values normalise through normalize_trigger_mode).
    pub mode: String,
    pub enabled: bool,
}

impl Default for WebhookEndpoint {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            secret: String::new(),
            schema: String::new(),
            mode: crate::trigger_history::DEFAULT_TRIGGER_MODE.into(),
            enabled: true,
        }
    }
}

/// What a GET exposes for a token — the kanban's public columns plus the
/// synthetic statuses a held message reports before it becomes work.
pub struct TaskStatus {
    pub status: String,
    pub title: String,
    pub result: Option<String>,
}

pub struct WebhookServer {
    endpoints: Mutex<HashMap<String, (WebhookEndpoint, Option<jsonschema::Validator>)>>,
    windows: Mutex<HashMap<String, (Instant, u32)>>,
    /// tokenHash → pending history-entry id. The raw token is never stored —
    /// the caller holds it and a GET re-hashes to match.
    held: Mutex<Option<HashMap<String, String>>>,
    decoy: String,
    stop: Arc<AtomicBool>,
}

/// Constant-time compare — OR-accumulate, length-guarded first (mirroring
/// upstream's timingSafeEqual shape). Shared with the Slack signature gate.
pub(crate) fn secret_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        let _ = a
            .iter()
            .zip(b.iter().cycle())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y));
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// SHA-256 hex of a capability token — the only form persisted (card stamp
/// and held-token key). Raw tokens are returned once and never stored.
pub(crate) fn hash_token(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex_encode(&h.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `randomBytes(n).toString('hex')` equivalent — two uuid4s as the entropy
/// source so no extra dependency is pulled in.
fn rand_hex(bytes: usize) -> String {
    let mut s = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    s.truncate(bytes * 2);
    s
}

impl WebhookServer {
    pub fn new() -> Self {
        Self {
            endpoints: Mutex::new(HashMap::new()),
            windows: Mutex::new(HashMap::new()),
            held: Mutex::new(None),
            decoy: rand_hex(32),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Swap the served list without restarting the server (upstream
    /// setEndpoints) — a removed id stops resolving on the very next request.
    /// Returns the count of enabled endpoints now served.
    pub fn set_endpoints(&self, list: &[WebhookEndpoint]) -> usize {
        let mut map = self.endpoints.lock().unwrap();
        map.clear();
        for e in list {
            if e.id.is_empty() || e.secret.is_empty() || !e.enabled {
                continue;
            }
            let compiled = if e.schema.trim().is_empty() {
                None
            } else {
                serde_json::from_str::<Value>(&e.schema)
                    .ok()
                    .and_then(|v| jsonschema::validator_for(&v).ok())
            };
            map.insert(e.id.clone(), (e.clone(), compiled));
        }
        let mut win = self.windows.lock().unwrap();
        win.retain(|k, _| k.is_empty() || k == UNKNOWN_BUCKET || map.contains_key(k));
        map.len()
    }

    /// Ids currently served — for the settings surface (parity with upstream).
    pub fn endpoint_ids(&self) -> Vec<String> {
        self.endpoints.lock().unwrap().keys().cloned().collect()
    }

    /// The held-token map, lazily loaded from the durable kv store
    /// (upstream heldTokens()).
    fn held_tokens(&self, app: &tauri::AppHandle) -> HashMap<String, String> {
        let mut guard = self.held.lock().unwrap();
        if let Some(m) = guard.as_ref() {
            return m.clone();
        }
        let stored = app
            .try_state::<crate::commands::AppState>()
            .and_then(|st| {
                st.persist
                    .lock()
                    .unwrap()
                    .get_kv(HELD_TOKENS_KV_KEY)
                    .and_then(|v| v.as_object().cloned())
            })
            .unwrap_or_default();
        let m: HashMap<String, String> = stored
            .into_iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
            .collect();
        *guard = Some(m.clone());
        m
    }

    fn held_set(&self, app: &tauri::AppHandle, hash: &str, entry_id: &str) {
        let mut guard = self.held.lock().unwrap();
        let m = guard.get_or_insert_with(HashMap::new);
        m.insert(hash.to_string(), entry_id.to_string());
        self.persist_held_locked(&m, app);
    }

    fn held_remove(&self, app: &tauri::AppHandle, hash: &str) {
        let mut guard = self.held.lock().unwrap();
        if let Some(m) = guard.as_mut() {
            m.remove(hash);
            self.persist_held_locked(m, app);
        }
    }

    fn persist_held_locked(&self, map: &HashMap<String, String>, app: &tauri::AppHandle) {
        if let Some(st) = app.try_state::<crate::commands::AppState>() {
            let v = json!(map);
            st.persist.lock().unwrap().set_kv(HELD_TOKENS_KV_KEY, &v);
        }
    }

    /// Drop mappings whose history entry has aged out of the capped ledger —
    /// the operator can no longer decide them (upstream pruneHeldTokens).
    pub fn prune_held(&self, app: &tauri::AppHandle, hist: &crate::trigger_history::TriggerHistory) {
        let map = self.held_tokens(app);
        if map.is_empty() {
            return;
        }
        let live: std::collections::HashSet<String> =
            hist.list().into_iter().map(|e| e.id).collect();
        let mut changed = false;
        {
            let mut guard = self.held.lock().unwrap();
            if let Some(m) = guard.as_mut() {
                m.retain(|_, id| {
                    let keep = live.contains(id);
                    if !keep {
                        changed = true;
                    }
                    keep
                });
            }
            if changed {
                self.persist_held_locked(guard.as_ref().unwrap(), app);
            }
        }
    }

    /// tokenHash for a pending history entry — the approval path stamps it
    /// onto the card it spawns (upstream heldTokenHashFor).
    fn held_hash_for(&self, app: &tauri::AppHandle, entry_id: &str) -> Option<String> {
        self.held_tokens(app)
            .into_iter()
            .find(|(_, id)| id == entry_id)
            .map(|(h, _)| h)
    }

    fn rate_ok(&self, bucket: &str, limit: u32) -> bool {
        let mut win = self.windows.lock().unwrap();
        let e = win.entry(bucket.to_string()).or_insert((Instant::now(), 0));
        if e.0.elapsed() >= RATE_WINDOW {
            *e = (Instant::now(), 0);
        }
        e.1 += 1;
        e.1 <= limit
    }

    /// Serve loop — one std thread per connection off tiny_http, matching the
    /// HookServer's shape. Knows the hive (routing + card lookup) and the app
    /// handle (persist + trigger ledger), never the endpoint secrets beyond
    /// the gate.
    pub fn run(
        self: Arc<Self>,
        port: u16,
        hive: Arc<crate::hive::Hive>,
        app: tauri::AppHandle,
    ) -> anyhow::Result<()> {
        let server = tiny_http::Server::http(("127.0.0.1", port))
            .map_err(|e| anyhow::anyhow!("webhook bind :{port}: {e}"))?;
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            let mut req = match server.recv_timeout(Duration::from_millis(500)) {
                Ok(Some(r)) => r,
                Ok(None) => continue,
                Err(_) => continue,
            };
            let me = Arc::clone(&self);
            let app2 = app.clone();
            let h2 = hive.clone();
            std::thread::spawn(move || {
                let (status, body) = me.handle(&mut req, &app2, &h2);
                let _ = req.respond(
                    tiny_http::Response::from_string(body.to_string())
                        .with_status_code(status)
                        .with_header(
                            "Content-Type: application/json"
                                .parse::<tiny_http::Header>()
                                .unwrap(),
                        ),
                );
            });
        }
    }

    /// The endpoint id from the request path: `/foo` → `foo`, bare `/` →
    /// `legacy`, a deeper path (`/a/b`) → null = "no such endpoint", answered
    /// exactly like a wrong secret / unknown token (upstream readEndpointId).
    fn endpoint_id_of(url: &str) -> Option<String> {
        let path = url.split('?').next().unwrap_or("/");
        let seg: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        match seg.len() {
            0 => Some(LEGACY_ENDPOINT_ID.into()),
            1 => Some(
                percent_decode(seg[0])
                    .unwrap_or_else(|| seg[0].to_string()),
            ),
            _ => None,
        }
    }

    /// Pull the capability token from `x-md-webhook-token`, falling back to a
    /// `?token=` query param (header preferred — kept out of URL/access logs).
    fn token_of(req: &tiny_http::Request, url: &str) -> String {
        if let Some(h) = req
            .headers()
            .iter()
            .find(|h| h.field.equiv("x-md-webhook-token"))
        {
            let v = h.value.as_str().trim();
            if !v.is_empty() {
                return v.to_string();
            }
        }
        if let Some(q) = url.split('?').nth(1) {
            for pair in q.split('&') {
                if let Some(v) = pair.strip_prefix("token=") {
                    let v = v.trim();
                    if !v.is_empty() {
                        return percent_decode(v).unwrap_or_else(|| v.to_string());
                    }
                }
            }
        }
        String::new()
    }

    /// One request → (status, json body). The whole secret gate + rate limit
    /// + schema check lives here so the response path is single-entrance.
    fn handle(
        &self,
        req: &mut tiny_http::Request,
        app: &tauri::AppHandle,
        hive: &crate::hive::Hive,
    ) -> (u16, Value) {
        // Rate limit first — even a 401 burns budget so floods can't probe.
        if !self.rate_ok("", RATE_LIMIT) {
            return (429, json!({ "ok": false, "error": "rate limited" }));
        }
        let method = req.method().as_str().to_uppercase();
        let url = req.url().to_string();
        let id = Self::endpoint_id_of(&url);

        // Resolve endpoint (unknown → decoy compare, same work as wrong secret).
        let (ep, real) = {
            let map = self.endpoints.lock().unwrap();
            match id.as_deref().and_then(|i| map.get(i)) {
                Some((e, _)) => (e.clone(), true),
                None => (
                    WebhookEndpoint {
                        secret: self.decoy.clone(),
                        ..Default::default()
                    },
                    false,
                ),
            }
        };
        let bucket = if real { id.as_deref().unwrap() } else { UNKNOWN_BUCKET };
        if !self.rate_ok(bucket, PER_ENDPOINT_RATE_LIMIT) {
            return (429, json!({ "ok": false, "error": "rate limited" }));
        }

        match method.as_str() {
            "GET" => self.handle_status(req, &url, real, app, hive),
            "POST" => self.handle_create(req, &id, &ep, real, app, hive),
            _ => (405, json!({ "ok": false, "error": "method not allowed" })),
        }
    }

    /// GET — the status of the token's task only; never a listing. The lookup
    /// runs even when the endpoint id is unknown, and the answer is then the
    /// same 404 the unknown-token case gives — no enumeration signal.
    fn handle_status(
        &self,
        req: &tiny_http::Request,
        url: &str,
        real: bool,
        app: &tauri::AppHandle,
        hive: &crate::hive::Hive,
    ) -> (u16, Value) {
        let token = Self::token_of(req, url);
        if token.is_empty() {
            return (401, json!({ "ok": false, "error": "token required" }));
        }
        let status = lookup_status(&token, app, hive, self);
        if status.is_none() || !real {
            return (404, json!({ "ok": false, "error": "not found" }));
        }
        let s = status.unwrap();
        (
            200,
            json!({
                "ok": true,
                "status": s.status,
                "title": s.title,
                "result": s.result,
            }),
        )
    }

    /// POST — verify THIS endpoint's secret, then buffer + validate + dispatch.
    fn handle_create(
        &self,
        req: &mut tiny_http::Request,
        id: &Option<String>,
        ep: &WebhookEndpoint,
        real: bool,
        app: &tauri::AppHandle,
        hive: &crate::hive::Hive,
    ) -> (u16, Value) {
        // Authenticate BEFORE reading the body — an unauthenticated peer can't
        // even make us buffer (within the size cap).
        let given = req
            .headers()
            .iter()
            .find(|h| h.field.equiv("x-md-webhook-secret"))
            .map(|h| h.value.as_str().to_string())
            .unwrap_or_default();
        if !secret_eq(&given, &ep.secret) || !real {
            return (401, json!({ "ok": false, "error": "unauthorized" }));
        }
        let endpoint_id = id.clone().unwrap();

        let mut body = Vec::new();
        let mut take = req.as_reader().take((MAX_BODY_BYTES + 1) as u64);
        if take.read_to_end(&mut body).is_err() || body.len() > MAX_BODY_BYTES {
            return (413, json!({ "ok": false, "error": "too large" }));
        }
        let Ok(v) = serde_json::from_slice::<Value>(&body) else {
            return (400, json!({ "ok": false, "error": "bad json" }));
        };

        // The endpoint's own schema decides what a valid body is. An
        // unparseable schema accepts everything (upstream parseSchema).
        {
            let map = self.endpoints.lock().unwrap();
            if let Some((_, Some(vv))) = map.get(&endpoint_id) {
                if !vv.is_valid(&v) {
                    return (400, json!({ "ok": false, "error": "schema validation failed" }));
                }
            }
        }

        // `message` is required regardless of what the user's schema says.
        let message = v
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if message.is_empty() {
            return (400, json!({ "ok": false, "error": "message required" }));
        }
        let title = v
            .get("title")
            .and_then(|t| t.as_str())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        let kind = v
            .get("kind")
            .and_then(|k| k.as_str())
            .filter(|k| *k == "directive" || *k == "communication")
            .map(|k| k.to_string());
        let from = v
            .get("from")
            .and_then(|f| f.as_str())
            .map(|f| f.trim().to_string())
            .filter(|f| !f.is_empty());

        match self.on_message(
            app,
            hive,
            &message,
            title.as_deref(),
            kind.as_deref(),
            from.as_deref(),
            &endpoint_id,
            &ep.name,
            &ep.mode,
        ) {
            Dispatch::Pending { token } => (
                202,
                json!({
                    "ok": true,
                    "pending": true,
                    "status": "awaiting-approval",
                    "token": token,
                    "detail": "accepted — waiting for the operator to approve it before the hive sees it"
                }),
            ),
            Dispatch::Routed { token, task_id } => {
                (200, json!({ "ok": true, "token": token, "taskId": task_id }))
            }
            Dispatch::Failed => {
                (500, json!({ "ok": false, "error": "could not create task" }))
            }
        }
    }

    /// handleWebhookMessage (index.ts:1927) — run a verified POST through the
    /// endpoint's TriggerMode. Either a kanban card + god request (allowed) or
    /// a `pending` ledger row + held token (gated). An `inbound` history row is
    /// recorded either way; no credential ever lands in it.
    fn on_message(
        &self,
        app: &tauri::AppHandle,
        hive: &crate::hive::Hive,
        message: &str,
        title: Option<&str>,
        kind: Option<&str>,
        from: Option<&str>,
        endpoint_id: &str,
        endpoint_name: &str,
        mode: &str,
    ) -> Dispatch {
        // 192-bit unguessable token, returned once; only its hash is stored.
        let token = rand_hex(24);
        let token_hash = hash_token(&token);
        let full = title.unwrap_or(message);
        let title_short = if full.chars().count() > 80 {
            format!("{}…", full.chars().take(79).collect::<String>())
        } else {
            full.to_string()
        };
        // A vanished endpoint falls back to the STRICTEST mode, never the
        // most permissive one (and legacy 'auto'/'review' map forward).
        let mode = crate::trigger_history::normalize_trigger_mode(mode);
        let kind = kind
            .map(|k| k.to_string())
            .unwrap_or_else(|| crate::trigger_history::classify_inbound_kind(message).to_string());
        let peer = from
            .map(|f| f.to_string())
            .unwrap_or_else(|| {
                if endpoint_name.is_empty() {
                    endpoint_id.to_string()
                } else {
                    endpoint_name.to_string()
                }
            });
        let correlation_id = rand_hex(8);

        let hist = app
            .try_state::<crate::commands::AppState>()
            .map(|st| st.trigger_history.clone());
        let base = crate::trigger_history::TriggerHistoryEntry {
            id: String::new(),
            source: "webhook".into(),
            source_id: endpoint_id.to_string(),
            source_name: endpoint_name.to_string(),
            direction: "inbound".into(),
            peer: peer.clone(),
            title: Some(title_short.clone()),
            body: message.to_string(),
            kind: kind.clone(),
            decision: None,
            correlation_id: Some(correlation_id.clone()),
            task_id: None,
            at: 0,
        };

        if !crate::trigger_history::is_auto_allowed(&mode, &kind) {
            if let Some(h) = &hist {
                let mut e = base.clone();
                e.decision = Some("pending".into());
                let e = h.append(e);
                self.held_set(app, &token_hash, &e.id);
            } else {
                // No ledger = no way to decide later — still hold honestly.
                self.held_set(app, &token_hash, "");
            }
            let _ = app.emit("triggerHistory:updated", json!({}));
            return Dispatch::Pending { token };
        }

        let task_id = format!("webhook-{}", rand_hex(8));
        if !dispatch_webhook_work(
            hive,
            &task_id,
            &title_short,
            message,
            Some(&token_hash),
            "webhook",
        ) {
            return Dispatch::Failed;
        }
        if let Some(h) = &hist {
            let mut e = base.clone();
            e.decision = Some("auto-allowed".into());
            e.task_id = Some(task_id.clone());
            h.append(e);
        }
        let _ = app.emit("triggerHistory:updated", json!({}));
        Dispatch::Routed { token, task_id }
    }

    /// The operator's verdict on a held history entry (upstream
    /// triggerHistory:decide). 'approved' releases the message down the
    /// identical card+god path an auto-allowed one takes; 'rejected' just
    /// flips. Idempotent: only a `pending` entry can be decided.
    pub fn decide(
        &self,
        app: &tauri::AppHandle,
        hive: &crate::hive::Hive,
        entry_id: &str,
        decision: &str,
    ) -> Option<Value> {
        let hist = app
            .try_state::<crate::commands::AppState>()
            .map(|st| st.trigger_history.clone())?;
        let entry = hist.list().into_iter().find(|e| e.id == entry_id)?;
        if entry.decision.as_deref() != Some("pending") {
            // Already decided → no-op, not a re-dispatch.
            return serde_json::to_value(&entry).ok();
        }
        if decision == "rejected" {
            let next = hist.update(entry_id, Some("rejected".into()), None, None, None);
            let _ = app.emit("triggerHistory:updated", json!({}));
            return next.and_then(|e| serde_json::to_value(e).ok());
        }
        if decision != "approved" {
            return None;
        }
        let task_id = format!("webhook-{}", rand_hex(8));
        let token_hash = self.held_hash_for(app, entry_id);
        let title = entry.title.clone().unwrap_or_else(|| {
            if entry.body.chars().count() > 80 {
                format!("{}…", entry.body.chars().take(79).collect::<String>())
            } else {
                entry.body.clone()
            }
        });
        let origin = if entry.source == "org" { "org" } else { "webhook" };
        if !dispatch_webhook_work(
            hive,
            &task_id,
            &title,
            &entry.body,
            token_hash.as_deref(),
            origin,
        ) {
            // Leave the entry pending — the operator can approve again once
            // the hive is writable (upstream behaviour).
            return serde_json::to_value(&entry).ok();
        }
        if let Some(h) = &token_hash {
            self.held_remove(app, h);
        }
        let next = hist.update(
            entry_id,
            Some("approved".into()),
            Some(task_id),
            None,
            None,
        );
        self.prune_held(app, &hist);
        let _ = app.emit("triggerHistory:updated", json!({}));
        next.and_then(|e| serde_json::to_value(e).ok())
    }

    /// The pending-review inbox — upstream's surface is the ledger itself
    /// (entries still sitting at `pending`), not an in-memory token map.
    pub fn pending(&self, app: &tauri::AppHandle) -> Vec<Value> {
        app.try_state::<crate::commands::AppState>()
            .map(|st| {
                st.trigger_history
                    .list()
                    .into_iter()
                    .filter(|e| e.decision.as_deref() == Some("pending"))
                    .filter_map(|e| serde_json::to_value(e).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Signal the serve loop to exit on its next 500ms tick.
    #[allow(dead_code)]
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

enum Dispatch {
    Pending { token: String },
    Routed { token: String, task_id: String },
    Failed,
}

/// dispatchWebhookWork (index.ts:1867) — create the stamped kanban card and
/// route the message to god. Split out because the APPROVAL path takes
/// exactly this route later — an operator's "yes" must produce the same card
/// and the same god request an auto-allowed message would have.
///
/// Returns false only when the card — the thing the caller polls — could not
/// be written. God routing is best-effort: the card already exists and is
/// pollable even if the send hiccups.
pub fn dispatch_webhook_work(
    hive: &crate::hive::Hive,
    task_id: &str,
    title: &str,
    message: &str,
    token_hash: Option<&str>,
    origin: &str,
) -> bool {
    let mut card = json!({
        "id": task_id,
        "title": title,
        "description": message,
        "status": "todo",
        "dependsOn": [],
        "priority": 1,
        "createdAt": chrono::Utc::now().to_rfc3339(),
    });
    if let Some(h) = token_hash {
        card["webhook"] = json!({ "tokenHash": h });
    }
    if hive.add_task(&card).is_err() {
        eprintln!("[webhook] could not create task card");
        return false;
    }
    // Body carries ONLY the sender's message + the card id — never the secret,
    // never the raw token.
    let body = format!(
        "{message}\n\n(Inbound via the generic {origin} API, tracked as kanban card {task_id}. When this work is finished, set that card's status to 'done' and fill its 'result' so the caller's status check reflects the outcome.)"
    );
    let _ = hive.send_virtual("webhook", "god", "request", &format!("[{origin}] {title}"), &body);
    true
}

/// lookupWebhookStatus (index.ts:1977) — resolve a capability token to its
/// task's public status, scoped to the ONE held message or ONE card whose
/// stored hash matches. None for any non-match (the caller answers 404 either
/// way — "unknown" and "malformed" are indistinguishable).
pub fn lookup_status(
    token: &str,
    app: &tauri::AppHandle,
    hive: &crate::hive::Hive,
    server: &WebhookServer,
) -> Option<TaskStatus> {
    let hash = hash_token(token);

    // Held messages first — they have no card, and the O(1) hit keeps the
    // common "still waiting" poll off the task scan entirely.
    let held = server.held_tokens(app);
    if let Some(entry_id) = held.get(&hash) {
        if !entry_id.is_empty() {
            if let Some(st) = app.try_state::<crate::commands::AppState>() {
                let entry = st
                    .trigger_history
                    .list()
                    .into_iter()
                    .find(|e| &e.id == entry_id);
                match entry {
                    None => {
                        server.held_remove(app, &hash);
                        return None;
                    }
                    Some(e) if e.decision.as_deref() == Some("pending") => {
                        return Some(TaskStatus {
                            status: "awaiting-approval".into(),
                            title: e.title.unwrap_or_default(),
                            result: None,
                        });
                    }
                    Some(e) if e.decision.as_deref() == Some("rejected") => {
                        return Some(TaskStatus {
                            status: "rejected".into(),
                            title: e.title.unwrap_or_default(),
                            result: None,
                        });
                    }
                    _ => {} // approved → the hash is now on a real card; fall through
                }
            }
        }
    }

    let tasks = hive.read_tasks().ok()?;
    for t in tasks["tasks"].as_array()? {
        let h = t
            .get("webhook")
            .and_then(|w| w.get("tokenHash"))
            .and_then(|h| h.as_str());
        let Some(h) = h else { continue };
        // Fixed-length sha-256 hex; compare in constant time defensively.
        if secret_eq(h, &hash) {
            return Some(TaskStatus {
                status: t["status"].as_str().unwrap_or("todo").to_string(),
                title: t["title"].as_str().unwrap_or_default().to_string(),
                result: t
                    .get("result")
                    .and_then(|r| r.as_str())
                    .map(|s| s.to_string()),
            });
        }
    }
    None
}

/// pollWebhookDoneTasks (index.ts:2035) — the OUTBOUND half of the trigger
/// ledger: watch the kanban for webhook-origin cards that reach 'done' and
/// write the reply side, tagged with the inbound row's correlationId. The
/// ledger itself is the dedup record, so a card that finished while the app
/// was closed still gets its outbound row on the next boot.
pub fn poll_done_tasks(
    hive: &crate::hive::Hive,
    hist: &crate::trigger_history::TriggerHistory,
    app: &tauri::AppHandle,
) {
    let Ok(doc) = hive.read_tasks() else { return };
    let Some(tasks) = doc["tasks"].as_array() else { return };
    let history = hist.list();
    let recorded: std::collections::HashSet<String> = history
        .iter()
        .filter(|e| e.direction == "outbound")
        .filter_map(|e| e.task_id.clone())
        .collect();
    let mut wrote = false;
    for t in tasks {
        let id = t["id"].as_str().unwrap_or("");
        if t["status"].as_str() != Some("done") {
            continue;
        }
        if t.get("webhook").is_none() && !id.starts_with("webhook-") {
            continue;
        }
        if recorded.contains(id) {
            continue;
        }
        // No inbound row = a card from before the ledger existed — nothing to
        // pair with, so mark it handled via a tombstone outbound skip.
        let Some(inbound) = history
            .iter()
            .find(|e| e.direction == "inbound" && e.task_id.as_deref() == Some(id))
        else {
            continue;
        };
        hist.append(crate::trigger_history::TriggerHistoryEntry {
            id: String::new(),
            source: inbound.source.clone(),
            source_id: inbound.source_id.clone(),
            source_name: inbound.source_name.clone(),
            direction: "outbound".into(),
            peer: inbound.peer.clone(),
            title: t["title"].as_str().map(|s| s.to_string()),
            body: t
                .get("result")
                .and_then(|r| r.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "(finished with no result recorded)".into()),
            kind: inbound.kind.clone(),
            decision: None,
            correlation_id: inbound.correlation_id.clone(),
            task_id: Some(id.to_string()),
            at: 0,
        });
        wrote = true;
    }
    if wrote {
        let _ = app.emit("triggerHistory:updated", json!({}));
    }
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = u8::from_str_radix(&s[i + 1..i + 3], 16).ok()?;
            out.push(h);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}
