//! ProxyBridge — port of `PROXY_BRIDGE_SHIM` (hive.ts:3328): a dependency-light
//! loopback reverse proxy that makes a HOOKLESS CLI a hive citizen.
//!
//! A proxy-tier agent (qwen, crush) is pointed at this via its base-URL env
//! (OPENAI_BASE_URL / CRUSH_GLOBAL_CONFIG). Every request forwards to the real
//! upstream UNCHANGED — headers, body, streaming — while the response is teed
//! into a ≤4MB capture that synthesizes the same hook payloads the shims emit:
//!
//!   - `Status`      — context gauge (usage tokens / per-model window)
//!   - `CostSample`  — the cost ledger's token rows
//!   - `PostToolUse` — one per tool call (breaker repeated-tool evidence)
//!   - `Stop`        — armed 800ms after a tool-call-free turn; a new request
//!                     or a tool call cancels it (turn still going)
//!
//! Bodies/keys are never logged; the capture is parsed in-memory and dropped.

use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

const CAPTURE_CAP: usize = 4 * 1024 * 1024;
const BODY_CAP: usize = 8 * 1024 * 1024;
const STOP_DEBOUNCE_MS: u64 = 800;

#[derive(Clone, Copy, PartialEq)]
pub enum ProxyApi {
    /// Anthropic-wire upstream (usage.message_start/content_block_start) —
    /// constructed when a proxy-tier agent targets an anthropic base URL.
    #[allow(dead_code)]
    Anthropic,
    OpenAi,
}

pub struct ProxyBridge {
    agent_id: String,
    session_id: String,
    api: ProxyApi,
    upstream: String,
    sock: String,
    /// Generation counter for the Stop debounce: each arming bumps it; the
    /// sleeper emits only if the generation is still current after 800ms.
    stop_gen: Arc<AtomicU64>,
    /// Teardown flag — set by stop() so the accept loop exits and the
    /// listener dies with its agent.
    stopped: Arc<AtomicU64>,
}

/// Per-model context-window size for the Status gauge — port of ctxSize().
fn ctx_size(model: &str) -> u64 {
    let m = model.to_lowercase();
    if m.contains("[1m]") || m.contains("-1m") {
        1_000_000
    } else if m.contains("claude") {
        200_000
    } else if m.contains("gpt-4o") || m.contains("gpt-4.1") || m.contains("o1") || m.contains("o3")
    {
        128_000
    } else if m.contains("qwen") {
        262_144
    } else {
        200_000
    }
}

impl ProxyBridge {
    pub fn new(
        agent_id: String,
        api: ProxyApi,
        upstream: String,
        sock: String,
        session_id: Option<String>,
    ) -> Self {
        Self {
            agent_id,
            session_id: session_id.unwrap_or_default(),
            api,
            upstream: upstream.trim_end_matches('/').to_string(),
            sock,
            stop_gen: Arc::new(AtomicU64::new(0)),
            stopped: Arc::new(AtomicU64::new(0)),
        }
    }

    /// hive.stopProxyBridge — the teardown half upstream runs on every dead
    /// PTY so a killed agent can't leave an orphan loopback listener.
    pub fn stop(&self) {
        self.stopped.store(1, Ordering::SeqCst);
    }

    /// Bind loopback:0 and serve — one std thread per connection. Returns the
    /// bound port the caller bakes into the agent's env/config.
    pub fn start(self: Arc<Self>) -> anyhow::Result<u16> {
        let server = tiny_http::Server::http(("127.0.0.1", 0))
            .map_err(|e| anyhow::anyhow!("proxy bind: {e}"))?;
        let port = match server.server_addr() {
            tiny_http::ListenAddr::IP(a) => a.port(),
            #[allow(unreachable_patterns)]
            _ => 0,
        };
        std::thread::spawn(move || loop {
            if self.stopped.load(Ordering::SeqCst) != 0 {
                return; // teardownPty → drop the listener
            }
            let req = match server.recv_timeout(Duration::from_millis(500)) {
                Ok(Some(r)) => r,
                _ => continue,
            };
            let me = self.clone();
            std::thread::spawn(move || me.handle(req));
        });
        Ok(port)
    }

    fn emit(&self, payload: Value) {
        let Ok(addr) = self.sock.parse::<std::net::SocketAddr>() else {
            return;
        };
        if let Ok(mut s) = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
            let mut line = serde_json::to_string(&payload).unwrap_or_default();
            line.push('\n');
            use std::io::Write;
            let _ = s.write_all(line.as_bytes());
        }
    }

    fn cancel_stop(&self) {
        self.stop_gen.fetch_add(1, Ordering::SeqCst);
    }

    fn arm_stop(&self) {
        let gen = self.stop_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let (g, agent, session, sock) = (
            self.stop_gen.clone(),
            self.agent_id.clone(),
            self.session_id.clone(),
            self.sock.clone(),
        );
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(STOP_DEBOUNCE_MS));
            if g.load(Ordering::SeqCst) != gen {
                return; // a newer request or tool call cancelled this stop
            }
            let Ok(addr) = sock.parse::<std::net::SocketAddr>() else {
                return;
            };
            if let Ok(mut s) = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
                let mut line = serde_json::to_string(&json!({
                    "agent_id": agent, "event": "Stop",
                    "payload": { "session_id": session }
                }))
                .unwrap_or_default();
                line.push('\n');
                use std::io::Write;
                let _ = s.write_all(line.as_bytes());
            }
        });
    }

    /// Envelope the way the shims send it — event name + payload body.
    fn emit_hook(&self, event: &str, payload: Value) {
        let mut p = payload;
        if let Some(obj) = p.as_object_mut() {
            if obj.get("session_id").is_none() {
                obj.insert("session_id".into(), json!(self.session_id));
            }
        }
        self.emit(json!({
            "agent_id": self.agent_id, "event": event, "payload": p
        }));
    }

    fn handle(&self, mut req: tiny_http::Request) {
        self.cancel_stop();
        if self.upstream.is_empty() {
            let _ = req.respond(
                tiny_http::Response::from_string("proxy: no upstream").with_status_code(502),
            );
            return;
        }
        let target = format!("{}{}", self.upstream, req.url());
        let mut body = Vec::new();
        if std::io::Read::read_to_end(
            &mut std::io::Read::take(req.as_reader(), BODY_CAP as u64 + 1),
            &mut body,
        )
        .is_err()
            || body.len() > BODY_CAP
        {
            let _ = req.respond(tiny_http::Response::empty(413));
            return;
        }

        // Forward headers verbatim except Host (ureq sets it), Content-Length
        // (recomputed) and accept-encoding (ask upstream for plaintext so the
        // tee can parse — the CLI gets uncompressed bytes either way).
        let mut rb = ureq::http::Request::builder()
            .method(req.method().as_str())
            .uri(&target);
        for h in req.headers() {
            let name = h.field.as_str().as_str();
            if name.eq_ignore_ascii_case("host")
                || name.eq_ignore_ascii_case("accept-encoding")
                || name.eq_ignore_ascii_case("content-length")
                || name.eq_ignore_ascii_case("connection")
            {
                continue;
            }
            rb = rb.header(name, h.value.as_str());
        }
        let up_req = match rb.body(&body[..]) {
            Ok(r) => r,
            Err(_) => {
                let _ = req.respond(tiny_http::Response::empty(502));
                return;
            }
        };
        // ureq 3 returns Ok for ANY completed HTTP exchange (4xx/5xx bodies
        // stream verbatim — a faithful proxy relays them); Err means the
        // transport itself failed.
        let (status, headers, reader) = match ureq::run(up_req) {
            Ok(resp) => {
                let st = resp.status();
                let hs: Vec<(String, String)> = resp
                    .headers()
                    .iter()
                    .map(|(n, v)| (n.to_string(), v.to_str().unwrap_or_default().to_string()))
                    .collect();
                (st, hs, resp.into_body().into_reader())
            }
            Err(_) => {
                let _ = req.respond(tiny_http::Response::empty(502));
                return;
            }
        };

        let ct = headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let want_parse = ct.contains("json") || ct.contains("event-stream");
        let is_sse = ct.contains("event-stream");

        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let resp = tiny_http::Response::new(
            tiny_http::StatusCode(status.as_u16()),
            headers
                .iter()
                .filter(|(n, _)| {
                    !n.eq_ignore_ascii_case("content-length")
                        && !n.eq_ignore_ascii_case("transfer-encoding")
                        && !n.eq_ignore_ascii_case("connection")
                })
                .map(|(n, v)| format!("{n}: {v}").parse::<tiny_http::Header>().unwrap())
                .collect(),
            Box::new(TeeShared {
                inner: Box::new(reader),
                out: captured.clone(),
            }),
            None,
            None,
        );
        let _ = req.respond(resp);
        if want_parse {
            let bytes = captured.lock().unwrap().clone();
            if !bytes.is_empty() {
                if let Ok(s) = String::from_utf8(bytes) {
                    self.parse_and_emit(&s, is_sse);
                }
            } else {
                self.arm_stop();
            }
        } else {
            self.arm_stop();
        }
    }

    /// parseAndEmit — usage → Status+CostSample; tool calls → PostToolUse each;
    /// a clean end → arm the idle debounce.
    fn parse_and_emit(&self, body: &str, is_sse: bool) {
        let mut objs: Vec<Value> = Vec::new();
        if is_sse {
            for ln in body.lines() {
                let Some(idx) = ln.find("data:") else {
                    continue;
                };
                let data = ln[idx + 5..].trim();
                if data.is_empty() || data == "[DONE]" {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<Value>(data) {
                    objs.push(v);
                }
            }
        } else if let Ok(v) = serde_json::from_str::<Value>(body) {
            objs.push(v);
        }
        if objs.is_empty() {
            self.arm_stop();
            return;
        }

        let mut model = String::new();
        let (mut input, mut output, mut cache_read, mut cache_creation) = (0u64, 0u64, 0u64, 0u64);
        let mut saw_usage = false;
        let mut tool_calls: Vec<(String, Value)> = Vec::new();
        let mut oai_tools: std::collections::HashMap<u64, (Option<String>, String)> =
            std::collections::HashMap::new();

        for o in &objs {
            if let Some(m) = o.get("model").and_then(|v| v.as_str()) {
                model = m.to_string();
            }
            match self.api {
                ProxyApi::Anthropic => {
                    match o.get("type").and_then(|v| v.as_str()) {
                        Some("message_start") => {
                            let u = o.pointer("/message/usage");
                            input += u
                                .and_then(|u| u.get("input_tokens"))
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            cache_read += u
                                .and_then(|u| u.get("cache_read_input_tokens"))
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            cache_creation += u
                                .and_then(|u| u.get("cache_creation_input_tokens"))
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            if let Some(m) = o.pointer("/message/model").and_then(|v| v.as_str()) {
                                model = m.to_string();
                            }
                            saw_usage = true;
                        }
                        Some("message_delta") => {
                            output += o
                                .pointer("/usage/output_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            saw_usage = true;
                        }
                        Some("content_block_start") => {
                            let cb = o.get("content_block");
                            if cb.and_then(|c| c.get("type")).and_then(|v| v.as_str())
                                == Some("tool_use")
                            {
                                tool_calls.push((
                                    cb.and_then(|c| c.get("name"))
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("tool")
                                        .into(),
                                    cb.and_then(|c| c.get("input"))
                                        .cloned()
                                        .unwrap_or(json!({})),
                                ));
                            }
                        }
                        _ => {
                            if let Some(u) = o.get("usage").filter(|_| o.get("type").is_none()) {
                                input +=
                                    u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                                output +=
                                    u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                                cache_read += u
                                    .get("cache_read_input_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);
                                cache_creation += u
                                    .get("cache_creation_input_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);
                                saw_usage = true;
                            }
                        }
                    }
                    if let Some(content) = o.get("content").and_then(|v| v.as_array()) {
                        for blk in content {
                            if blk.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                                tool_calls.push((
                                    blk.get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("tool")
                                        .into(),
                                    blk.get("input").cloned().unwrap_or(json!({})),
                                ));
                            }
                        }
                    }
                }
                ProxyApi::OpenAi => {
                    if let Some(u) = o.get("usage") {
                        input += u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                        output += u
                            .get("completion_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        cache_read += u
                            .pointer("/prompt_tokens_details/cached_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        saw_usage = true;
                    }
                    for ch in o
                        .get("choices")
                        .and_then(|v| v.as_array())
                        .into_iter()
                        .flatten()
                    {
                        for tc in ch
                            .pointer("/message/tool_calls")
                            .and_then(|v| v.as_array())
                            .into_iter()
                            .flatten()
                        {
                            if let Some(f) = tc.get("function") {
                                tool_calls.push((
                                    f.get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("tool")
                                        .into(),
                                    safe_args(f.get("arguments")),
                                ));
                            }
                        }
                        for (t, tc) in ch
                            .pointer("/delta/tool_calls")
                            .and_then(|v| v.as_array())
                            .into_iter()
                            .flatten()
                            .enumerate()
                        {
                            let k = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(t as u64);
                            let e = oai_tools.entry(k).or_insert((None, String::new()));
                            if let Some(f) = tc.get("function") {
                                if let Some(n) = f.get("name").and_then(|v| v.as_str()) {
                                    e.0 = Some(n.to_string());
                                }
                                if let Some(a) = f.get("arguments").and_then(|v| v.as_str()) {
                                    e.1.push_str(a);
                                }
                            }
                        }
                    }
                }
            }
        }
        for (_, (name, args)) in oai_tools {
            if let Some(n) = name {
                tool_calls.push((n, safe_args_str(&args)));
            }
        }

        if saw_usage {
            self.emit_hook(
                "Status",
                json!({
                    "context_window": {
                        "total_input_tokens": input + cache_read + cache_creation,
                        "context_window_size": ctx_size(&model)
                    }
                }),
            );
            self.emit_hook(
                "CostSample",
                json!({
                    "model": model, "input": input, "output": output,
                    "cache_read": cache_read, "cache_creation": cache_creation
                }),
            );
        }
        if tool_calls.is_empty() {
            self.arm_stop();
        } else {
            self.cancel_stop();
            for (name, input) in tool_calls {
                self.emit_hook(
                    "PostToolUse",
                    json!({ "tool_name": name, "tool_input": input }),
                );
            }
        }
    }
}

/// Shared-capture tee — the reader hands bytes to tiny_http AND appends to
/// the Arc'd capture the handler reads after the body completes.
struct TeeShared {
    inner: Box<dyn Read + Send>,
    out: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
}

impl Read for TeeShared {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        let mut out = self.out.lock().unwrap();
        if out.len() < CAPTURE_CAP {
            let room = CAPTURE_CAP - out.len();
            out.extend_from_slice(&buf[..n.min(room)]);
        }
        Ok(n)
    }
}

fn safe_args(v: Option<&Value>) -> Value {
    match v {
        Some(Value::Object(_)) => v.cloned().unwrap_or(json!({})),
        Some(Value::String(s)) => safe_args_str(s),
        _ => json!({}),
    }
}

fn safe_args_str(s: &str) -> Value {
    serde_json::from_str(s)
        .unwrap_or_else(|_| json!({ "_raw": s.chars().take(500).collect::<String>() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::sync::mpsc;

    /// A fake HookServer: captures every emitted envelope line.
    fn fake_hook_server() -> (String, mpsc::Receiver<Value>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let Ok(conn) = conn else { return };
                    let mut line = String::new();
                    if std::io::BufReader::new(&conn).read_line(&mut line).is_ok() {
                        if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
                            let _ = tx.send(v);
                        }
                    }
                });
            }
        });
        (addr, rx)
    }

    fn bridge(sock: &str) -> ProxyBridge {
        ProxyBridge::new(
            "a1".into(),
            ProxyApi::OpenAi,
            "https://api.openai.com".into(),
            sock.into(),
            Some("sess1".into()),
        )
    }

    #[test]
    fn openai_sse_emits_status_cost_and_tool_use() {
        let (sock, rx) = fake_hook_server();
        let b = bridge(&sock);
        let sse = concat!(
            "data: {\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\"\"}}]}}]}\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\\\":\\\"x.txt\\\"}\"}}]}}]}\n",
            "data: {\"model\":\"gpt-4o\",\"usage\":{\"prompt_tokens\":1200,\"completion_tokens\":30,\"prompt_tokens_details\":{\"cached_tokens\":100}}}\n",
            "data: [DONE]\n"
        );
        b.parse_and_emit(sse, true);

        let mut events: Vec<String> = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while events.len() < 3 && std::time::Instant::now() < deadline {
            if let Ok(v) = rx.recv_timeout(Duration::from_millis(500)) {
                events.push(v["event"].as_str().unwrap_or("?").into());
                if v["event"] == "CostSample" {
                    assert_eq!(v["payload"]["input"], json!(1200));
                    assert_eq!(v["payload"]["cache_read"], json!(100));
                }
            }
        }
        assert!(events.contains(&"Status".into()), "{events:?}");
        assert!(events.contains(&"CostSample".into()), "{events:?}");
        assert!(events.contains(&"PostToolUse".into()), "{events:?}");
    }

    #[test]
    fn no_tool_calls_arms_idle_stop() {
        let (sock, rx) = fake_hook_server();
        let b = bridge(&sock);
        // A completed turn with usage but no tool calls → Stop within ~800ms.
        b.parse_and_emit(
            "{\"model\":\"gpt-4o\",\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}",
            false,
        );
        let mut saw_stop = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if let Ok(v) = rx.recv_timeout(Duration::from_millis(400)) {
                if v["event"] == "Stop" {
                    saw_stop = true;
                    break;
                }
            }
        }
        assert!(saw_stop);
    }
}
