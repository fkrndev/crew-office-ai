//! Slack adapter — port of `slack.ts` + `slack-trigger.cjs`.
//!
//! A bare HTTP server implementing just enough of the Slack Events API to
//! pipe a channel's @-mentions into the hive: HMAC-SHA256 signature verify
//! over the RAW body + 5-minute replay guard, the url_verification
//! handshake, mention/activated-thread trigger logic, channel:ts dedup, and
//! the loopback reply rail that lets agents post back WITHOUT ever seeing
//! the bot token.
//!
//! NOT ported: the tunnelmole tunnel — upstream opens a public URL so Slack
//! can reach the local port. The local handler is the security boundary and
//! stays identical; deploy behind any tunnel that forwards to `port`.

use std::collections::{HashSet, VecDeque};
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;

/// Reject bodies beyond this — Slack event payloads are tiny; the cap stops
/// an unauthenticated peer from forcing unbounded memory use.
const MAX_BODY_BYTES: usize = 1024 * 1024;
/// Slack's recommended replay window (seconds).
const REPLAY_WINDOW_SECS: u64 = 60 * 5;
const ACTIVATED_THREADS_MAX: usize = 500;
const SEEN_EVENTS_MAX: usize = 500;
const MAX_FILES_PER_MESSAGE: usize = 10;

/* ── slack-trigger.cjs: bounded FIFO sets + the mention gate ─────────────── */

/// Bounded FIFO set — thread roots where the bot was @-mentioned (replies
/// in those threads also trigger), and the dedup cache of recently-seen
/// channel:ts message identities.
#[derive(Default)]
struct FifoSet {
    set: HashSet<String>,
    order: VecDeque<String>,
    max: usize,
}

impl FifoSet {
    fn new(max: usize) -> Self {
        Self {
            set: HashSet::new(),
            order: VecDeque::new(),
            max,
        }
    }
    fn insert(&mut self, k: String) {
        if self.set.contains(&k) {
            return;
        }
        if self.set.len() >= self.max {
            if let Some(oldest) = self.order.pop_front() {
                self.set.remove(&oldest);
            }
        }
        self.set.insert(k.clone());
        self.order.push_back(k);
    }
    fn contains(&self, k: &str) -> bool {
        self.set.contains(k)
    }
    /// Record and report ALREADY-present (dedup): empty keys never dedupe.
    fn seen(&mut self, k: &str) -> bool {
        if k.is_empty() {
            return false;
        }
        if self.set.contains(k) {
            return true;
        }
        self.insert(k.to_string());
        false
    }
}

/// `channel:ts` — the only reliable cross-type key (app_mention + message.*
/// deliveries share it; event_id/client_msg_id do not).
fn dedup_key(ev: &Value) -> String {
    let (c, t) = (
        ev["channel"].as_str().unwrap_or(""),
        ev["ts"].as_str().unwrap_or(""),
    );
    if c.is_empty() || t.is_empty() {
        String::new()
    } else {
        format!("{c}:{t}")
    }
}

/// shouldTrigger — channel filter, self-loop/subtype safety, mention OR
/// activated-thread check; on a mention the thread root is activated.
fn should_trigger(
    ev: &Value,
    bot_user_id: Option<&str>,
    channel_id: Option<&str>,
    threads: &mut FifoSet,
) -> (bool, String, Vec<Value>) {
    if let Some(ch) = channel_id {
        if ev["channel"].as_str() != Some(ch) {
            return (false, String::new(), vec![]);
        }
    }
    // Self-loop + subtype safety: never trigger on bot posts, edits, joins.
    // file_share IS eligible if it mentions the bot / hits an active thread.
    if ev.get("bot_id").is_some() || ev["subtype"].as_str().is_some_and(|s| s != "file_share") {
        return (false, String::new(), vec![]);
    }
    let text = ev["text"].as_str().unwrap_or("").to_string();
    let mention_tag = bot_user_id.map(|b| format!("<@{b}>")).unwrap_or_default();
    let is_mention = ev["type"].as_str() == Some("app_mention")
        || (ev["type"].as_str() == Some("message")
            && !mention_tag.is_empty()
            && text.contains(&mention_tag))
        || (ev["subtype"].as_str() == Some("file_share")
            && !mention_tag.is_empty()
            && text.contains(&mention_tag));
    let is_activated_thread = ev["thread_ts"]
        .as_str()
        .is_some_and(|t| threads.contains(t));
    if !is_mention && !is_activated_thread {
        return (false, String::new(), vec![]);
    }
    if is_mention {
        if let Some(root) = ev["thread_ts"].as_str().or_else(|| ev["ts"].as_str()) {
            threads.insert(root.to_string());
        }
    }
    let files: Vec<Value> = ev["files"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f["url_private"].as_str().is_some_and(|u| !u.is_empty()))
        .take(MAX_FILES_PER_MESSAGE)
        .collect();
    (true, text, files)
}

/// Strip a single leading `<@BOTID>` mention: "@bot do X" enqueues "do X".
fn strip_leading_mention(text: &str) -> String {
    let t = text.trim_start();
    if t.starts_with("<@") {
        if let Some(end) = t.find('>') {
            return t[end + 1..].trim().to_string();
        }
    }
    text.trim().to_string()
}

/* ── the server ───────────────────────────────────────────────────────────── */

pub struct SlackServer {
    stop: Arc<AtomicBool>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    pub url_port: std::sync::Mutex<Option<u16>>,
}

/// HMAC-SHA256 hex of `v0:<ts>:<rawBody>` under the signing secret.
fn compute_sig(secret: &str, ts: &str, raw: &str) -> Option<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(format!("v0:{ts}:{raw}").as_bytes());
    Some(
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect(),
    )
}

impl SlackServer {
    pub fn new() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            handle: Mutex::new(None),
            url_port: Mutex::new(None),
        }
    }

    pub fn is_running(&self) -> bool {
        self.handle.lock().unwrap().is_some()
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.lock().unwrap().take() {
            let _ = h.join();
        }
        *self.url_port.lock().unwrap() = None;
    }

    /// Bind + serve. `on_message` receives the verified inbound (text,
    /// channel, ts, thread_ts, files) — the hive routes it to god.
    pub fn start<F>(
        &self,
        port: u16,
        signing_secret: String,
        channel_id: Option<String>,
        on_message: F,
    ) -> Result<(), String>
    where
        F: Fn(String, String, String, String, Vec<Value>) + Send + Sync + 'static,
    {
        if signing_secret.is_empty() {
            return Err("missing signing secret".into());
        }
        if self.is_running() {
            return Err("already running".into());
        }
        // Loopback only: the tunnel (misc::start_tunnel / any reverse proxy)
        // forwards to localhost, so the signed-event handler is never exposed
        // to the LAN directly.
        let server = tiny_http::Server::http(("127.0.0.1", port))
            .map_err(|e| format!("failed to bind port {port}: {e}"))?;
        let stop = self.stop.clone();
        stop.store(false, Ordering::Relaxed);
        let channel_id = channel_id.filter(|c| !c.trim().is_empty());
        let handle = std::thread::spawn(move || {
            let state = Arc::new(Mutex::new(SlackState {
                bot_user_id: None,
                threads: FifoSet::new(ACTIVATED_THREADS_MAX),
                seen: FifoSet::new(SEEN_EVENTS_MAX),
            }));
            let on_message = Arc::new(on_message);
            let secret = Arc::new(signing_secret);
            let channel_id = Arc::new(channel_id);
            let inflight = Arc::new(AtomicUsize::new(0));
            while !stop.load(Ordering::Relaxed) {
                let Ok(Some(req)) = server.recv_timeout(std::time::Duration::from_millis(400))
                else {
                    continue;
                };
                // One worker per connection: a peer that stalls mid-body can
                // only tie up its own thread, never the accept loop. The cap
                // bounds how many such threads an unauthenticated peer buys.
                if inflight.fetch_add(1, Ordering::SeqCst) >= MAX_INFLIGHT {
                    inflight.fetch_sub(1, Ordering::SeqCst);
                    let _ = req.respond(tiny_http::Response::from_string("").with_status_code(503));
                    continue;
                }
                let (state, on_message, secret, channel_id, inflight) = (
                    state.clone(),
                    on_message.clone(),
                    secret.clone(),
                    channel_id.clone(),
                    inflight.clone(),
                );
                std::thread::spawn(move || {
                    handle_events_request(
                        req,
                        &secret,
                        channel_id.as_deref(),
                        &state,
                        on_message.as_ref(),
                    );
                    inflight.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        *self.handle.lock().unwrap() = Some(handle);
        *self.url_port.lock().unwrap() = Some(port);
        Ok(())
    }
}

/// Mutable trigger state shared by the per-connection workers.
struct SlackState {
    bot_user_id: Option<String>,
    threads: FifoSet,
    seen: FifoSet,
}

/// Cap on concurrently-served connections (see the accept loop).
const MAX_INFLIGHT: usize = 32;

/// One Events API request: cheap header gates BEFORE the body is read, then
/// body → signature over the raw bytes → handshake / trigger.
fn handle_events_request<F>(
    mut req: tiny_http::Request,
    signing_secret: &str,
    channel_id: Option<&str>,
    state: &Mutex<SlackState>,
    on_message: &F,
) where
    F: Fn(String, String, String, String, Vec<Value>),
{
    let reply = |req: tiny_http::Request, code: u16, body: &str| {
        let _ = req.respond(tiny_http::Response::from_string(body.to_string()).with_status_code(code));
    };
    if req.method().as_str() != "POST" {
        return reply(req, 405, "");
    }
    // Declared size first — refuse an oversized body without reading any of it.
    if req.body_length().is_some_and(|n| n > MAX_BODY_BYTES) {
        return reply(req, 413, "");
    }
    let header = |req: &tiny_http::Request, name: &'static str| {
        req.headers()
            .iter()
            .find(|h| h.field.equiv(name))
            .map(|h| h.value.as_str().to_string())
            .unwrap_or_default()
    };
    let sig = header(&req, "x-slack-signature");
    let ts = header(&req, "x-slack-request-timestamp");
    let now_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let ts_num: u64 = ts.parse().unwrap_or(0);
    let fresh = ts_num > 0 && now_s.abs_diff(ts_num) <= REPLAY_WINDOW_SECS;
    let given = sig.strip_prefix("v0=").unwrap_or("");
    // No signature / stale timestamp can never authenticate — drop before
    // spending a read on the body.
    if !fresh || given.is_empty() {
        return reply(req, 403, "");
    }
    let mut body = Vec::new();
    let mut take = req.as_reader().take((MAX_BODY_BYTES + 1) as u64);
    if take.read_to_end(&mut body).is_err() || body.len() > MAX_BODY_BYTES {
        return reply(req, 413, "");
    }
    let raw = String::from_utf8_lossy(&body).to_string();
    // 1) Authenticate over the RAW body BEFORE parsing.
    let expected = compute_sig(signing_secret, &ts, &raw).unwrap_or_default();
    let ok = given.len() == expected.len() && crate::webhook::secret_eq(given, &expected);
    if !ok {
        return reply(req, 403, "");
    }
    let Ok(payload) = serde_json::from_str::<Value>(&raw) else {
        return reply(req, 400, "");
    };
    // 2) url_verification handshake — echo the challenge.
    if payload["type"].as_str() == Some("url_verification") {
        let challenge = payload["challenge"].as_str().unwrap_or("").to_string();
        return reply(req, 200, &json!({ "challenge": challenge }).to_string());
    }
    // 3) event_callback — learn bot id, then the trigger gate.
    if payload["type"].as_str() == Some("event_callback") {
        let ev = &payload["event"];
        let dispatch = {
            let mut st = state.lock().unwrap();
            if st.bot_user_id.is_none() {
                if let Some(u) = payload["authorizations"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|a| a["user_id"].as_str())
                {
                    st.bot_user_id = Some(u.to_string());
                }
            }
            let bot = st.bot_user_id.clone();
            let (trigger, raw_text, files) =
                should_trigger(ev, bot.as_deref(), channel_id, &mut st.threads);
            let mut out = None;
            if trigger {
                let text = strip_leading_mention(&raw_text);
                let channel = ev["channel"].as_str().unwrap_or("").to_string();
                let ts = ev["ts"].as_str().unwrap_or("").to_string();
                let thread_ts = ev["thread_ts"]
                    .as_str()
                    .filter(|t| !t.is_empty())
                    .unwrap_or(&ts)
                    .to_string();
                if (!text.is_empty() || !files.is_empty()) && !channel.is_empty() && !ts.is_empty()
                {
                    let key = dedup_key(ev);
                    if !st.seen.seen(&key) {
                        out = Some((text, channel, ts, thread_ts, files));
                    }
                }
            }
            out
        };
        // Outside the state lock — on_message does hive IO.
        if let Some((text, channel, ts, thread_ts, files)) = dispatch {
            on_message(text, channel, ts, thread_ts, files);
        }
    }
    // Always 200 — Slack treats it as delivered, no retry storm.
    reply(req, 200, "")
}

/// Post a reply into a Slack thread via chat.postMessage — a raw HTTPS POST
/// (zero-SDK approach; the bot token lives in the credential store and is
/// never logged). CLAUSE-1 guard: an explicit channel+thread is required —
/// a blank thread_ts would post to the channel root, an implicit
/// destination the caller never named.
pub fn post_slack_reply(
    bot_token: &str,
    channel: &str,
    thread_ts: &str,
    text: &str,
) -> Result<(), String> {
    if bot_token.is_empty() {
        return Err("missing bot token".into());
    }
    if channel.trim().is_empty() || thread_ts.trim().is_empty() {
        return Err("missing explicit channel or thread_ts".into());
    }
    let body = json!({ "channel": channel, "thread_ts": thread_ts, "text": text });
    let mut resp = ureq::post("https://slack.com/api/chat.postMessage")
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build()
        .header("content-type", "application/json; charset=utf-8")
        .header("authorization", &format!("Bearer {bot_token}"))
        .send_json(&body)
        .map_err(|e| e.to_string())?;
    let v: Value = resp
        .body_mut()
        .read_json()
        .map_err(|e| format!("bad response from Slack: {e}"))?;
    if v["ok"].as_bool() == Some(true) {
        Ok(())
    } else {
        Err(v["error"].as_str().unwrap_or("slack error").to_string())
    }
}

/// SlackReplyServer — port of upstream's loopback reply rail (slack.ts:425).
/// Agents post {channel, thread_ts, text} and the server applies the bot
/// token itself, so the token NEVER enters a worker's environment. Binds
/// 127.0.0.1 only; every request must echo the per-session `x-md-reply-token`
/// header. Main writes `{port, token}` to <harness>/slack-reply.json so the
/// bundled helper can find this socket.
pub struct SlackReplyServer {
    stop: Arc<AtomicBool>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    pub token: String,
    pub port: std::sync::Mutex<Option<u16>>,
}

impl SlackReplyServer {
    pub fn new() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            handle: Mutex::new(None),
            // Per-session shared secret the helper echoes in the header.
            token: format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            )[..48]
                .to_string(),
            port: Mutex::new(None),
        }
    }

    pub fn is_running(&self) -> bool {
        self.handle.lock().unwrap().is_some()
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.lock().unwrap().take() {
            let _ = h.join();
        }
        *self.port.lock().unwrap() = None;
    }

    /// Bind a loopback port (0 ⇒ OS-assigned) and serve. `bot_token` is read
    /// lazily via the closure at reply time so a credential rotation is picked
    /// up without a restart. `on_replied` fires with the thread_ts after a
    /// successful DIRECT post so the done-summary poller can skip the thread.
    pub fn start<G, R>(
        &self,
        preferred_port: u16,
        bot_token: G,
        on_replied: R,
    ) -> Result<u16, String>
    where
        G: Fn() -> Option<String> + Send + Sync + 'static,
        R: Fn(String) + Send + Sync + 'static,
    {
        if self.is_running() {
            return Err("already running".into());
        }
        let server = tiny_http::Server::http(("127.0.0.1", preferred_port))
            .map_err(|e| format!("reply server bind :{preferred_port}: {e}"))?;
        let port = server
            .server_addr()
            .to_ip()
            .map(|a| a.port())
            .unwrap_or(preferred_port);
        let stop = self.stop.clone();
        stop.store(false, Ordering::Relaxed);
        let session_token = Arc::new(self.token.clone());
        let bot_token = Arc::new(bot_token);
        let on_replied = Arc::new(on_replied);
        let inflight = Arc::new(AtomicUsize::new(0));
        let handle = std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let Ok(Some(req)) = server.recv_timeout(std::time::Duration::from_millis(400))
                else {
                    continue;
                };
                // Per-connection worker (a slow body or a slow Slack POST must
                // not stall every other agent's reply), capped.
                if inflight.fetch_add(1, Ordering::SeqCst) >= MAX_INFLIGHT {
                    inflight.fetch_sub(1, Ordering::SeqCst);
                    let _ = req.respond(tiny_http::Response::from_string("").with_status_code(503));
                    continue;
                }
                let (session_token, bot_token, on_replied, inflight) = (
                    session_token.clone(),
                    bot_token.clone(),
                    on_replied.clone(),
                    inflight.clone(),
                );
                std::thread::spawn(move || {
                    handle_reply_request(req, &session_token, &*bot_token, &*on_replied);
                    inflight.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        *self.handle.lock().unwrap() = Some(handle);
        *self.port.lock().unwrap() = Some(port);
        Ok(port)
    }
}

/// One loopback reply-rail request (peer + token gate, then the post).
fn handle_reply_request<G, R>(
    mut req: tiny_http::Request,
    session_token: &str,
    bot_token: &G,
    on_replied: &R,
) where
    G: Fn() -> Option<String>,
    R: Fn(String),
{
    let respond = |req: tiny_http::Request, code: u16, body: &str| {
        let _ = req.respond(
            tiny_http::Response::from_string(body.to_string())
                .with_status_code(code)
                .with_header(
                    "Content-Type: application/json"
                        .parse::<tiny_http::Header>()
                        .unwrap(),
                ),
        );
    };
    // Defense in depth: even bound loopback-only, refuse any
    // non-loopback peer.
    let remote = req
        .remote_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_default();
    let loopback = {
        let a = remote.strip_prefix("::ffff:").unwrap_or(&remote);
        a == "::1" || a.starts_with("127.")
    };
    if !loopback {
        return respond(req, 403, "");
    }
    if req.method().as_str() != "POST" || req.url().split('?').next() != Some("/reply") {
        return respond(req, 404, "");
    }
    let given = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("x-md-reply-token"))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default();
    if !crate::webhook::secret_eq(&given, session_token) {
        return respond(req, 401, "");
    }
    if req.body_length().is_some_and(|n| n > MAX_BODY_BYTES) {
        return respond(req, 413, "");
    }
    let mut body = Vec::new();
    let mut take = req.as_reader().take((MAX_BODY_BYTES + 1) as u64);
    if take.read_to_end(&mut body).is_err() || body.len() > MAX_BODY_BYTES {
        return respond(req, 413, "");
    }
    let Ok(p) = serde_json::from_slice::<Value>(&body) else {
        return respond(req, 400, r#"{"ok":false,"error":"bad json"}"#);
    };
    let Some(token) = bot_token() else {
        return respond(req, 503, r#"{"ok":false,"error":"no bot token"}"#);
    };
    let (channel, thread_ts, text) = (
        p["channel"].as_str().unwrap_or(""),
        p["thread_ts"].as_str().unwrap_or(""),
        p["text"].as_str().unwrap_or(""),
    );
    if channel.is_empty() || thread_ts.is_empty() || text.is_empty() {
        return respond(
            req,
            400,
            r#"{"ok":false,"error":"channel, thread, text required"}"#,
        );
    }
    let thread_owned = thread_ts.to_string();
    match post_slack_reply(&token, channel, thread_ts, text) {
        Ok(()) => {
            on_replied(thread_owned);
            respond(req, 200, r#"{"ok":true}"#);
        }
        Err(e) => {
            respond(req, 502, &json!({ "ok": false, "error": e }).to_string());
        }
    }
}

/* ── attachment downloads (index.ts downloadSlackFile) ───────────────────── */

/// Per-file download size cap — reject files larger than 10 MB before writing.
const SLACK_FILE_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// Sanitize a Slack filename: basename only, non-safe chars → `_`, prefixed
/// with a random tag to stop collisions and path-traversal (index.ts:1476).
fn sanitize_filename(name: Option<&str>, tag: &str) -> String {
    let base = name
        .and_then(|n| std::path::Path::new(n).file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let safe: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = safe.trim_start_matches('.');
    let safe = if safe.is_empty() { "file" } else { safe };
    // ≤200 BYTES, cut on a char boundary — the name is all Unicode
    // alphanumerics, so a raw byte slice can land mid-codepoint and panic.
    let mut end = safe.len().min(200);
    while !safe.is_char_boundary(end) {
        end -= 1;
    }
    let safe = &safe[..end];
    format!("{tag}-{safe}")
}

/// Download a Slack file_share attachment into `dest_dir` using the bot
/// token (Authorization header only — never logged). Returns the local
/// {path,name,mimetype} or None on any failure; a partial list is still
/// useful to the agent.
pub fn download_slack_file(
    file: &Value,
    bot_token: &str,
    dest_dir: &std::path::Path,
) -> Option<Value> {
    let url = file["url_private"].as_str()?;
    if !url.starts_with("https://") {
        return None;
    }
    let tag = format!("{}", uuid::Uuid::new_v4().simple())[..8].to_string();
    let filename = sanitize_filename(file["name"].as_str(), &tag);
    let dest = dest_dir.join(&filename);
    std::fs::create_dir_all(dest_dir).ok()?;
    let resp = ureq::get(url)
        .header("authorization", &format!("Bearer {bot_token}"))
        .call()
        .ok()?;
    let mut reader = resp.into_body().into_reader();
    let mut buf = Vec::new();
    let mut take = reader.by_ref().take(SLACK_FILE_MAX_BYTES + 1);
    take.read_to_end(&mut buf).ok()?;
    if buf.len() as u64 > SLACK_FILE_MAX_BYTES {
        let _ = std::fs::remove_file(&dest);
        return None;
    }
    std::fs::write(&dest, &buf).ok()?;
    Some(json!({
        "path": dest.display().to_string(),
        "name": file["name"].as_str().unwrap_or(&filename),
        "mimetype": file["mimetype"].as_str().unwrap_or("application/octet-stream"),
    }))
}

/// Download every raw file from a file_share event — silently dropping the
/// failures (index.ts downloadSlackFiles).
pub fn download_slack_files(
    raw_files: &[Value],
    bot_token: Option<&str>,
    dest_dir: &std::path::Path,
) -> Vec<Value> {
    let Some(token) = bot_token else {
        return Vec::new();
    };
    raw_files
        .iter()
        .filter_map(|f| download_slack_file(f, token, dest_dir))
        .collect()
}

/* ── the AUTONOMOUS REQUEST PROTOCOL (index.ts buildAutonomousRequestProtocol) ── */

/// Built PER MESSAGE so the protocol block carries THIS request's concrete
/// channel, thread_ts, and the resolved helper path. Prepended to god's
/// working instruction only — the kanban title stays the user's raw text.
pub fn autonomous_request_protocol(channel: &str, thread_ts: &str, helper_path: &str) -> String {
    format!("[AUTONOMOUS REQUEST PROTOCOL — this request arrived via Slack; no interactive human is watching] Handle it under this protocol:
1. ROUTE FAST — triage and hand this to the single most-relevant agent right away. CHECK THE LIVE ROSTER FIRST (active agents in registry.json + their state in fleet.json) and prefer an EXISTING agent that fits — especially when the request names one (\"ask Pam…\", \"have Jim…\"): route to that agent and only spawn a new one if none is a sensible fit. Decompose only if it genuinely needs several. Don't sit on it.
2. DELEGATE WITH THE REPLY HANDLE — tell that agent to do the work autonomously AND to post its result back to THIS Slack thread itself when done, using exactly: \"{helper_path}\" --channel {channel} --thread {thread_ts} --text \"<substantive result>\" (the helper resolves its endpoint + token from the slack-reply.json the app writes — pass the path verbatim.)
3. AUTONOMOUS EXECUTION — no interactive questions. PAUSE/ask ONLY for high-severity actions: pushing to main or any remote; buying or spawning infrastructure or paid services; deleting an existing repo, file, or folder it did not create. Stay READ-ONLY at critical infrastructure and git-push-type changes unless explicitly approved.
4. DIRECT, SUBSTANTIVE REPLY — the agent posts a real Slack-mrkdwn answer (short *bold* headline + the actual outcome/specifics/links), NEVER a bare \"done\"/\":white_check_mark:\".
5. REPORT TO GOD — the agent then tells you (the orchestrator) what it did.
6. ASYNC QUESTIONS — if a decision is genuinely needed, don't block: post the question + numbered OPTIONS to the thread via that reply command, and record {{q, options, askedAt (ISO + day & time), thread_ts {thread_ts}}} so the threaded human reply correlates back and resumes.
The user's message starts now: ")
}

/// The done-summary text for a finished Slack-origin task (index.ts
/// slackDoneSummary) — result → description → title, Slack-friendly ≤2800.
pub fn done_summary(task: &Value) -> String {
    let body = task
        .get("result")
        .and_then(|r| r.as_str())
        .or_else(|| task.get("description").and_then(|d| d.as_str()))
        .unwrap_or("")
        .trim()
        .to_string();
    let title = task["title"].as_str().unwrap_or("");
    let head = format!(":white_check_mark: *{title}*");
    let text = if body.is_empty() {
        head
    } else {
        format!("{head}\n\n{body}")
    };
    if text.chars().count() > 2800 {
        format!("{}…", text.chars().take(2799).collect::<String>())
    } else {
        text
    }
}

/// Slack `chat.postMessage` errors that are permanent for this config —
/// retrying can never make them succeed (index.ts TERMINAL_SLACK_ERRORS).
pub fn is_terminal_slack_error(err: &str) -> bool {
    matches!(
        err,
        "missing_scope"
            | "invalid_auth"
            | "not_authed"
            | "account_inactive"
            | "token_revoked"
            | "token_expired"
            | "no_permission"
            | "channel_not_found"
            | "not_in_channel"
            | "is_archived"
            | "restricted_action"
            | "org_login_required"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(channel: &str, ts: &str, ty: &str, text: &str) -> Value {
        json!({"channel": channel, "ts": ts, "type": ty, "text": text})
    }

    #[test]
    fn sanitize_filename_truncates_on_char_boundary() {
        // 3-byte chars: a byte slice at 200 lands mid-codepoint (200 % 3 != 0).
        let long = "文".repeat(150) + ".txt";
        let out = sanitize_filename(Some(&long), "abcd1234");
        assert!(out.starts_with("abcd1234-"));
        assert!(out.len() <= "abcd1234-".len() + 200);
        assert!(out.ends_with('文'));
        assert_eq!(sanitize_filename(Some("../../.hidden name.txt"), "t"), "t-hidden_name.txt");
        assert_eq!(sanitize_filename(None, "t"), "t-file");
    }

    #[test]
    fn trigger_mentions_activated_threads_and_filters() {
        let mut th = FifoSet::new(ACTIVATED_THREADS_MAX);
        // app_mention triggers and activates the thread.
        let (t, _, _) = should_trigger(
            &ev("C", "1.0", "app_mention", "<@B01> hi"),
            Some("B01"),
            None,
            &mut th,
        );
        assert!(t);
        assert!(th.contains("1.0"));
        // A later reply in that thread triggers without a mention.
        let mut rep = ev("C", "1.1", "message", "follow-up");
        rep["thread_ts"] = json!("1.0");
        let (t2, _, _) = should_trigger(&rep, Some("B01"), None, &mut th);
        assert!(t2);
        // A text mention elsewhere triggers; unrelated messages don't.
        let (t3, _, _) = should_trigger(
            &ev("C", "2", "message", "hey <@B01>"),
            Some("B01"),
            None,
            &mut th,
        );
        assert!(t3);
        let (t4, _, _) = should_trigger(
            &ev("C", "3", "message", "noise"),
            Some("B01"),
            None,
            &mut th,
        );
        assert!(!t4);
        // Bot posts + non-file_share subtypes are dropped (self-loop guard).
        let mut bot = ev("C", "4", "app_mention", "<@B01>");
        bot["bot_id"] = json!("B9");
        assert!(!should_trigger(&bot, Some("B01"), None, &mut th).0);
        let mut edit = ev("C", "5", "message", "<@B01> x");
        edit["subtype"] = json!("message_changed");
        assert!(!should_trigger(&edit, Some("B01"), None, &mut th).0);
        // Channel filter.
        assert!(
            !should_trigger(
                &ev("OTHER", "6", "app_mention", "<@B01>"),
                Some("B01"),
                Some("C"),
                &mut th
            )
            .0
        );
    }

    #[test]
    fn dedup_covers_dual_delivery() {
        let mut seen = FifoSet::new(SEEN_EVENTS_MAX);
        // The SAME message arrives as app_mention + message.* — dedup_key
        // must be identical (channel:ts), so only one onMessage fires.
        let k1 = dedup_key(&ev("C", "1.0", "app_mention", "x"));
        let k2 = dedup_key(&ev("C", "1.0", "message", "x"));
        assert_eq!(k1, k2);
        assert!(!seen.seen(&k1));
        assert!(seen.seen(&k2));
        assert!(!seen.seen("")); // empty keys never dedupe
    }

    #[test]
    fn mention_strip_only_leading() {
        assert_eq!(strip_leading_mention("<@B01> do x"), "do x");
        assert_eq!(strip_leading_mention("  <@B01>   do x"), "do x");
        assert_eq!(
            strip_leading_mention("tell <@B01> something"),
            "tell <@B01> something"
        );
        assert_eq!(strip_leading_mention("no mention"), "no mention");
    }
}
