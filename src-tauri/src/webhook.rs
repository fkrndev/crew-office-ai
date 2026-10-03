//! WebhookServer — port of `src/main/webhook.ts`: a secret-gated inbound HTTP
//! API that turns external posts into hive work.
//!
//!   POST /<webhookId>  + `x-md-webhook-secret: <secret>` → route to god (auto
//!   mode → 200) or hold for operator review (review mode → 202 + token).
//!   GET /status/<token> → the one token's public status, nothing else.
//!
//! Security contract preserved:
//!   - constant-time secret comparison, length-guarded, per endpoint
//!   - an UNKNOWN endpoint id answers exactly like a WRONG secret (decoy
//!     compare so the failure path does the same work) — no id enumeration
//!   - secrets never logged/echoed/forwarded into routed messages
//!   - 1MB body cap, global + per-endpoint fixed-window rate limits, and one
//!     shared bucket for unknown ids (per-id buckets would let a prober both
//!     grow memory and observe which ids are real)
//!
//! Deviation: loopback only (upstream additionally tunnels via tunnelmole) —
//! an operator can point ngrok/cloudflared at the port for public ingress.

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{Emitter, Manager};

const MAX_BODY_BYTES: usize = 1024 * 1024;
const RATE_LIMIT: u32 = 120;
const PER_ENDPOINT_RATE_LIMIT: u32 = 60;
const RATE_WINDOW: Duration = Duration::from_secs(60);
const UNKNOWN_BUCKET: &str = ":unknown";

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
    /// 'auto' = route straight to god; 'review' = hold for operator approval.
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
            mode: "auto".into(),
            enabled: true,
        }
    }
}

/// A parked review-mode message + the resolution state a GET reports.
/// `hist_id` ties the token to its trigger-ledger row for approve/reject.
enum TokenRec {
    Pending {
        body: Value,
        endpoint_id: String,
        endpoint_name: String,
        hist_id: Option<String>,
    },
    Delivered { target: String, msg_id: String },
}

pub struct WebhookServer {
    endpoints: std::sync::Mutex<HashMap<String, (WebhookEndpoint, Option<jsonschema::Validator>)>>,
    windows: std::sync::Mutex<HashMap<String, (Instant, u32)>>,
    tokens: std::sync::Mutex<HashMap<String, TokenRec>>,
    decoy: String,
    stop: Arc<AtomicBool>,
}

/// Constant-time compare — OR-accumulate, length-guarded first (mirroring
/// upstream's timingSafeEqual shape).
fn secret_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        // Still burn a comparison so length timing doesn't leak.
        let _ = a
            .iter()
            .zip(b.iter().cycle())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y));
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

impl WebhookServer {
    pub fn new() -> Self {
        Self {
            endpoints: std::sync::Mutex::new(HashMap::new()),
            windows: std::sync::Mutex::new(HashMap::new()),
            tokens: std::sync::Mutex::new(HashMap::new()),
            decoy: uuid::Uuid::new_v4().simple().to_string(),
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
        // Drop rate state for ids no longer served (keep global + unknown).
        let mut win = self.windows.lock().unwrap();
        win.retain(|k, _| k.is_empty() || k == UNKNOWN_BUCKET || map.contains_key(k));
        map.len()
    }

    /// Ids currently served — for the settings surface (parity with upstream).
    #[allow(dead_code)]
    pub fn endpoint_ids(&self) -> Vec<String> {
        self.endpoints.lock().unwrap().keys().cloned().collect()
    }

    /// Release a held review-mode message into the hive (operator approve) —
    /// routes it exactly like an auto-mode POST and returns its message id.
    /// Uses the endpoint name the message arrived under (survives a rename).
    /// Approve — routes exactly like an auto-mode POST using the endpoint id
    /// the message arrived under.
    pub fn approve(&self, token: &str, hive: &crate::hive::Hive) -> Option<String> {
        let (body, name, eid) = match self.tokens.lock().unwrap().remove(token)? {
            TokenRec::Pending { body, endpoint_id, endpoint_name, .. } => {
                (body, endpoint_name, endpoint_id)
            }
            rec => {
                self.tokens.lock().unwrap().insert(token.to_string(), rec);
                return None;
            }
        };
        let msg_id = dispatch_to_hive(hive, &eid, &name, &body)?;
        self.mark_delivered(token.to_string(), "god".into(), msg_id.clone());
        Some(msg_id)
    }

    /// Drop a held message (operator reject).
    pub fn reject(&self, token: &str) -> bool {
        matches!(
            self.tokens.lock().unwrap().remove(token),
            Some(TokenRec::Pending { .. })
        )
    }

    /// The pending-review inbox (for a UI surface later).
    pub fn pending(&self) -> Vec<(String, Value)> {
        self.tokens
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(t, r)| match r {
                TokenRec::Pending { body, .. } => Some((t.clone(), body.clone())),
                _ => None,
            })
            .collect()
    }

    /// Park a verified message as pending (returns its capability token).
    fn park(
        &self,
        body: Value,
        endpoint_id: String,
        endpoint_name: String,
        hist_id: Option<String>,
    ) -> String {
        let token = uuid::Uuid::new_v4().simple().to_string();
        self.tokens
            .lock()
            .unwrap()
            .insert(token.clone(), TokenRec::Pending { body, endpoint_id, endpoint_name, hist_id });
        token
    }

    /// The trigger-ledger row id behind a pending token — the command layer
    /// flips that row's `pending` decision on approve/reject.
    pub fn pending_hist(&self, token: &str) -> Option<String> {
        match self.tokens.lock().unwrap().get(token) {
            Some(TokenRec::Pending { hist_id, .. }) => hist_id.clone(),
            _ => None,
        }
    }

    /// Mark a routed message's token for the status endpoint.
    fn mark_delivered(&self, token: String, target: String, msg_id: String) {
        self.tokens
            .lock()
            .unwrap()
            .insert(token, TokenRec::Delivered { target, msg_id });
    }

    /// Consume a pending message the operator approved — returns the body for
    /// the caller to route (keeps hive knowledge out of this class).
    #[allow(dead_code)]
    pub fn take_pending(&self, token: &str) -> Option<Value> {
        match self.tokens.lock().unwrap().remove(token)? {
            TokenRec::Pending { body, .. } => Some(body),
            rec => {
                self.tokens.lock().unwrap().insert(token.to_string(), rec);
                None
            }
        }
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
    /// HookServer's shape. Knows only the hive (for routing + status), never
    /// the whole app state.
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
            return (429, json!({ "error": "rate limited" }));
        }

        let method = req.method().as_str().to_uppercase();
        let url = req.url().to_string();
        let path = url.split('?').next().unwrap_or("/");
        let seg: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        // GET /status/<token> — token-scoped, never enumerates.
        if method == "GET" && seg.len() == 2 && seg[0] == "status" {
            return self.status_of(seg[1], hive);
        }

        if method != "POST" || seg.is_empty() {
            return (404, json!({ "error": "not found" }));
        }
        let endpoint_id = seg[0];

        // Resolve endpoint + compare its secret; an unknown id compares
        // against the decoy so the failure does the same work either way.
        let (ep, compiled, real) = {
            let map = self.endpoints.lock().unwrap();
            match map.get(endpoint_id) {
                Some((e, c)) => (e.clone(), c.is_some(), true),
                None => (
                    WebhookEndpoint {
                        secret: self.decoy.clone(),
                        ..Default::default()
                    },
                    false,
                    false,
                ),
            }
        };
        let bucket = if real { endpoint_id } else { UNKNOWN_BUCKET };
        if !self.rate_ok(bucket, PER_ENDPOINT_RATE_LIMIT) {
            return (429, json!({ "error": "rate limited" }));
        }
        let given = req
            .headers()
            .iter()
            .find(|h| h.field.equiv("x-md-webhook-secret"))
            .map(|h| h.value.as_str().to_string())
            .unwrap_or_default();
        if !secret_eq(&given, &ep.secret) || !real {
            return (401, json!({ "error": "unauthorized" }));
        }

        // Body cap before buffering — stops an unauthenticated-forbidden peer
        // forcing unbounded memory use.
        let mut body = Vec::new();
        let mut take = req.as_reader().take((MAX_BODY_BYTES + 1) as u64);
        if take.read_to_end(&mut body).is_err() || body.len() > MAX_BODY_BYTES {
            return (413, json!({ "error": "body too large" }));
        }
        let Ok(v) = serde_json::from_slice::<Value>(&body) else {
            return (400, json!({ "error": "invalid json" }));
        };

        // Built-in shape: {message, title?, kind?, from?} — message required.
        let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
        if msg.is_empty() {
            return (422, json!({ "error": "message required" }));
        }
        // Custom JSON Schema, when the endpoint carries one.
        if compiled {
            let map = self.endpoints.lock().unwrap();
            if let Some((_, Some(vv))) = map.get(endpoint_id) {
                if !vv.is_valid(&v) {
                    return (422, json!({ "error": "schema validation failed" }));
                }
            }
        }

        let name = if ep.name.is_empty() { endpoint_id } else { &ep.name };
        let _ = hive.log(&json!({
            "event": "webhook_inbound", "endpoint": endpoint_id,
            "mode": ep.mode, "from": v.get("from")
        }));

        // Trigger ledger — record the inbound FIRST so a review park can tie
        // the token to its row. Fields built one by one (never the endpoint —
        // it carries `secret`).
        let from = v.get("from").and_then(|f| f.as_str()).unwrap_or("").to_string();
        let kind = v
            .get("kind")
            .and_then(|k| k.as_str())
            .filter(|k| *k == "communication" || *k == "directive")
            .map(|k| k.to_string())
            .unwrap_or_else(|| {
                crate::trigger_history::classify_inbound_kind(msg).to_string()
            });
        let mk_entry = |decision: &str| crate::trigger_history::TriggerHistoryEntry {
            id: String::new(),
            source: "webhook".into(),
            source_id: endpoint_id.to_string(),
            source_name: name.to_string(),
            direction: "inbound".into(),
            peer: from.clone(),
            title: v.get("title").and_then(|t| t.as_str()).map(|t| t.to_string()),
            body: msg.to_string(),
            kind: kind.clone(),
            decision: Some(decision.into()),
            correlation_id: None,
            task_id: None,
            at: 0,
        };
        let hist = app
            .try_state::<crate::commands::AppState>()
            .map(|st| st.trigger_history.clone());

        if ep.mode == "review" {
            let e = hist.as_ref().map(|h| h.append(mk_entry("pending")));
            let token = self.park(
                v.clone(),
                endpoint_id.to_string(),
                name.to_string(),
                e.map(|e| e.id),
            );
            let _ = app.emit(
                "webhook-pending",
                json!({ "endpoint": name, "token": token }),
            );
            return (202, json!({ "token": token, "pending": true }));
        }

        // Auto mode: hand the verified body to the caller for hive routing.
        let token = uuid::Uuid::new_v4().simple().to_string();
        match dispatch_to_hive(hive, endpoint_id, name, &v) {
            Some(msg_id) => {
                self.mark_delivered(token.clone(), "god".into(), msg_id.clone());
                let mut e = mk_entry("auto-allowed");
                e.task_id = Some(msg_id.clone());
                if let Some(h) = &hist {
                    h.append(e);
                }
                (
                    200,
                    json!({ "token": token, "pending": false, "taskId": msg_id }),
                )
            }
            None => (500, json!({ "error": "dispatch failed" })),
        }
    }

    fn status_of(&self, token: &str, hive: &crate::hive::Hive) -> (u16, Value) {
        let tokens = self.tokens.lock().unwrap();
        match tokens.get(token) {
            Some(TokenRec::Pending { .. }) => (200, json!({ "status": "pending-review" })),
            Some(TokenRec::Delivered { target, msg_id }) => {
                let done = hive
                    .agent_dir(target)
                    .join("inbox/.done")
                    .join(format!("{msg_id}.json"));
                (
                    200,
                    json!({
                        "status": if done.exists() { "done" } else { "delivered" },
                        "target": target,
                        "msgId": msg_id
                    }),
                )
            }
            None => (404, json!({ "error": "unknown token" })),
        }
    }

    /// Signal the serve loop to exit on its next 500ms tick — parity with
    /// upstream stop(); teardown hook arrives with app shutdown handling.
    #[allow(dead_code)]
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Route a verified webhook body into god's inbox as a 'webhook' message —
/// shared by the auto path and a review-mode approve.
pub fn dispatch_to_hive(
    hive: &crate::hive::Hive,
    endpoint_id: &str,
    endpoint_name: &str,
    v: &Value,
) -> Option<String> {
    let title = v
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or(endpoint_name);
    let msg = v.get("message")?.as_str()?;
    let from = v
        .get("from")
        .and_then(|f| f.as_str())
        .unwrap_or(endpoint_id);
    let body = format!("{msg}\n\n(from: {from}, endpoint: {endpoint_name})");
    let sent = hive.send_virtual("webhook", "god", "request", title, &body).ok()?;
    sent.first().map(|d| d.msg_id.clone())
}
