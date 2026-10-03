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
use std::sync::atomic::{AtomicBool, Ordering};
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
        Self { set: HashSet::new(), order: VecDeque::new(), max }
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
    let (c, t) = (ev["channel"].as_str().unwrap_or(""), ev["ts"].as_str().unwrap_or(""));
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
    if ev.get("bot_id").is_some()
        || ev["subtype"].as_str().is_some_and(|s| s != "file_share")
    {
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
        F: Fn(String, String, String, String, Vec<Value>) + Send + 'static,
    {
        if signing_secret.is_empty() {
            return Err("missing signing secret".into());
        }
        if self.is_running() {
            return Err("already running".into());
        }
        let server = tiny_http::Server::http(("0.0.0.0", port))
            .map_err(|e| format!("failed to bind port {port}: {e}"))?;
        let stop = self.stop.clone();
        stop.store(false, Ordering::Relaxed);
        let channel_id = channel_id.filter(|c| !c.trim().is_empty());
        let handle = std::thread::spawn(move || {
            let mut bot_user_id: Option<String> = None;
            let mut threads = FifoSet::new(ACTIVATED_THREADS_MAX);
            let mut seen = FifoSet::new(SEEN_EVENTS_MAX);
            while !stop.load(Ordering::Relaxed) {
                let Ok(Some(mut req)) = server.recv_timeout(std::time::Duration::from_millis(400)) else {
                    continue;
                };
                if req.method().as_str() != "POST" {
                    let _ = req.respond(tiny_http::Response::from_string("")
                        .with_status_code(405));
                    continue;
                }
                let mut body = Vec::new();
                let mut take = req.as_reader().take((MAX_BODY_BYTES + 1) as u64);
                if take.read_to_end(&mut body).is_err() || body.len() > MAX_BODY_BYTES {
                    let _ = req.respond(tiny_http::Response::from_string("")
                        .with_status_code(413));
                    continue;
                }
                let raw = String::from_utf8_lossy(&body).to_string();
                // 1) Authenticate over the RAW body BEFORE parsing.
                let sig = req
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("x-slack-signature"))
                    .map(|h| h.value.as_str().to_string())
                    .unwrap_or_default();
                let ts = req
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("x-slack-request-timestamp"))
                    .map(|h| h.value.as_str().to_string())
                    .unwrap_or_default();
                let now_s = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let ts_num: u64 = ts.parse().unwrap_or(0);
                let fresh = ts_num > 0 && now_s.abs_diff(ts_num) <= REPLAY_WINDOW_SECS;
                let expected = compute_sig(&signing_secret, &ts, &raw).unwrap_or_default();
                let given = sig.strip_prefix("v0=").unwrap_or("");
                let ok = fresh
                    && !given.is_empty()
                    && given.len() == expected.len()
                    && crate::webhook::secret_eq(given, &expected);
                if !ok {
                    let _ = req.respond(tiny_http::Response::from_string("")
                        .with_status_code(403));
                    continue;
                }
                let Ok(payload) = serde_json::from_str::<Value>(&raw) else {
                    let _ = req.respond(tiny_http::Response::from_string("")
                        .with_status_code(400));
                    continue;
                };
                // 2) url_verification handshake — echo the challenge.
                if payload["type"].as_str() == Some("url_verification") {
                    let challenge = payload["challenge"].as_str().unwrap_or("").to_string();
                    let _ = req.respond(
                        tiny_http::Response::from_string(
                            json!({ "challenge": challenge }).to_string(),
                        )
                        .with_status_code(200),
                    );
                    continue;
                }
                // 3) event_callback — learn bot id, then the trigger gate.
                if payload["type"].as_str() == Some("event_callback") {
                    if bot_user_id.is_none() {
                        if let Some(u) = payload["authorizations"]
                            .as_array()
                            .and_then(|a| a.first())
                            .and_then(|a| a["user_id"].as_str())
                        {
                            bot_user_id = Some(u.to_string());
                        }
                    }
                    let ev = &payload["event"];
                    let (trigger, raw_text, files) = should_trigger(
                        ev,
                        bot_user_id.as_deref(),
                        channel_id.as_deref(),
                        &mut threads,
                    );
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
                            if !seen.seen(&key) {
                                on_message(text, channel, ts, thread_ts, files);
                            }
                        }
                    }
                }
                // Always 200 — Slack treats it as delivered, no retry storm.
                let _ = req.respond(tiny_http::Response::from_string("").with_status_code(200));
            }
        });
        *self.handle.lock().unwrap() = Some(handle);
        *self.url_port.lock().unwrap() = Some(port);
        Ok(())
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(channel: &str, ts: &str, ty: &str, text: &str) -> Value {
        json!({"channel": channel, "ts": ts, "type": ty, "text": text})
    }

    #[test]
    fn trigger_mentions_activated_threads_and_filters() {
        let mut th = FifoSet::new(ACTIVATED_THREADS_MAX);
        // app_mention triggers and activates the thread.
        let (t, _, _) = should_trigger(&ev("C", "1.0", "app_mention", "<@B01> hi"), Some("B01"), None, &mut th);
        assert!(t);
        assert!(th.contains("1.0"));
        // A later reply in that thread triggers without a mention.
        let mut rep = ev("C", "1.1", "message", "follow-up");
        rep["thread_ts"] = json!("1.0");
        let (t2, _, _) = should_trigger(&rep, Some("B01"), None, &mut th);
        assert!(t2);
        // A text mention elsewhere triggers; unrelated messages don't.
        let (t3, _, _) = should_trigger(&ev("C", "2", "message", "hey <@B01>"), Some("B01"), None, &mut th);
        assert!(t3);
        let (t4, _, _) = should_trigger(&ev("C", "3", "message", "noise"), Some("B01"), None, &mut th);
        assert!(!t4);
        // Bot posts + non-file_share subtypes are dropped (self-loop guard).
        let mut bot = ev("C", "4", "app_mention", "<@B01>");
        bot["bot_id"] = json!("B9");
        assert!(!should_trigger(&bot, Some("B01"), None, &mut th).0);
        let mut edit = ev("C", "5", "message", "<@B01> x");
        edit["subtype"] = json!("message_changed");
        assert!(!should_trigger(&edit, Some("B01"), None, &mut th).0);
        // Channel filter.
        assert!(!should_trigger(&ev("OTHER", "6", "app_mention", "<@B01>"), Some("B01"), Some("C"), &mut th).0);
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
        assert_eq!(strip_leading_mention("tell <@B01> something"), "tell <@B01> something");
        assert_eq!(strip_leading_mention("no mention"), "no mention");
    }
}
