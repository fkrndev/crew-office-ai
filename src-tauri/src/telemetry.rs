//! Telemetry collector — port of `src/main/telemetry.ts`.
//!
//! An embedded OTLP/HTTP-JSON collector: Claude Code pushes OpenTelemetry to
//! `OTEL_EXPORTER_OTLP_ENDPOINT` (set per-agent in spawn — the same env block
//! as hive.ts:983): metrics to /v1/metrics, log records to /v1/logs. Loopback
//! only; malformed batches are dropped, never thrown into the socket.
//!
//!   - `claude_code.token.usage` + `claude_code.cost.usage` metrics
//!     accumulate per session → per-agent cumulative sample (the breaker's
//!     LIVE usage source; transcript stays the fallback)
//!   - `tool_result` log records → a 200-deep span ring per agent (waterfall
//!     + "recent distinct tools" evidence)
//!   - `tool_decision` → accept/reject on the newest span
//!   - `api_error` → fed straight to the breaker's error-storm arm

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tauri::Emitter;

const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;
const SPAN_RING_CAP: usize = 200;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpan {
    pub agent_id: String,
    pub session_id: String,
    pub ts: u64,
    pub tool: String,
    pub success: bool,
    pub duration_ms: f64,
    pub decision: Option<String>,
}

#[derive(Default)]
struct SessionAccum {
    model: String,
    ts: u64,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
    usd: f64,
}

pub struct TelemetryCollector {
    sessions: Mutex<HashMap<String, SessionAccum>>,
    /// agentId → its session ids (an agent respawns → new sessions join).
    agent_sessions: Mutex<HashMap<String, HashSet<String>>>,
    /// agentId → ring buffer of recent tool spans (the waterfall).
    spans: Mutex<HashMap<String, Vec<ToolSpan>>>,
    /// agentId → api_error timestamps for the breaker's storm window — kept
    /// here so the collector itself can answer "has live telemetry".
    api_errors: Mutex<HashMap<String, Vec<u64>>>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// OTLP attributes [{key, value:{stringValue|intValue|doubleValue|boolValue}}]
/// → a flat lookup map.
fn flatten_attrs(v: Option<&Value>) -> HashMap<String, Value> {
    let mut out = HashMap::new();
    for a in v.and_then(|v| v.as_array()).into_iter().flatten() {
        if let (Some(k), Some(val)) = (a.get("key").and_then(|k| k.as_str()), a.get("value")) {
            let v = val
                .get("stringValue")
                .or_else(|| val.get("intValue"))
                .or_else(|| val.get("doubleValue"))
                .or_else(|| val.get("boolValue"))
                .cloned()
                .unwrap_or(Value::Null);
            out.insert(k.to_string(), v);
        }
    }
    out
}

fn s(v: Option<&Value>) -> String {
    v.and_then(|v| v.as_str()).unwrap_or_default().to_string()
}

fn f(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(st)) => st.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(st)) => st == "true" || st == "1",
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
        _ => false,
    }
}

fn point_value(dp: &Value) -> f64 {
    if dp.get("asInt").is_some() {
        f(dp.get("asInt"))
    } else {
        f(dp.get("asDouble"))
    }
}

impl TelemetryCollector {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            agent_sessions: Mutex::new(HashMap::new()),
            spans: Mutex::new(HashMap::new()),
            api_errors: Mutex::new(HashMap::new()),
        }
    }

    /// The pull seam: live OTel aggregate for one agent — the same data the
    /// breaker's UsageSample needs. None when the agent has no live telemetry.
    pub fn sample(&self, agent_id: &str) -> Option<crate::breaker::UsageSample> {
        let set = self.agent_sessions.lock().unwrap();
        let sids = set.get(agent_id)?;
        if sids.is_empty() {
            return None;
        }
        let sessions = self.sessions.lock().unwrap();
        let mut out = crate::breaker::UsageSample::default();
        let mut seen = false;
        for sid in sids {
            let Some(a) = sessions.get(sid) else { continue };
            seen = true;
            out.input += a.input;
            out.output += a.output;
            out.cache_read += a.cache_read;
            out.cache_creation += a.cache_creation;
            out.usd += a.usd;
            if a.ts >= out.ts {
                out.ts = a.ts;
            }
        }
        seen.then_some(out)
    }

    /// Per-session usage rows for the cost-ledger append path — upstream's
    /// `if (moved) hive.appendCostLedger(sample)` needs a sessionId + that
    /// session's cumulative usd, not the agent-rolled aggregate.
    pub fn session_samples(&self, agent_id: &str) -> Vec<(String, serde_json::Value)> {
        let sids = self
            .agent_sessions
            .lock()
            .unwrap()
            .get(agent_id)
            .cloned()
            .unwrap_or_default();
        let sessions = self.sessions.lock().unwrap();
        let mut out = Vec::new();
        for sid in sids {
            let Some(a) = sessions.get(&sid) else {
                continue;
            };
            out.push((
                sid.clone(),
                serde_json::json!({
                    "session_id": sid,
                    "input": a.input, "output": a.output,
                    "cache_read": a.cache_read, "cache_creation": a.cache_creation,
                    "model": a.model, "usd": a.usd,
                }),
            ));
        }
        out
    }

    /// True when the agent has ANY live session (even a zero-token boot
    /// sample) — "no turn" means something for a telemetry-exporting agent.
    pub fn has_telemetry(&self, agent_id: &str) -> bool {
        self.agent_sessions
            .lock()
            .unwrap()
            .get(agent_id)
            .is_some_and(|s| !s.is_empty())
    }

    /// Timestamp of the most recent span or usage activity — the wake
    /// watchdog's telemetry evidence channel.
    pub fn last_activity_at(&self, agent_id: &str) -> u64 {
        let span = self
            .spans
            .lock()
            .unwrap()
            .get(agent_id)
            .and_then(|r| r.last())
            .map(|s| s.ts)
            .unwrap_or(0);
        span.max(self.sample(agent_id).map(|s| s.ts).unwrap_or(0))
    }

    /// Respawn-safe: drop a dead agent's live accumulation so a replacement
    /// starts with a fresh counter (ledger + transcripts untouched).
    pub fn forget(&self, agent_id: &str) {
        if let Some(sids) = self.agent_sessions.lock().unwrap().remove(agent_id) {
            let mut sessions = self.sessions.lock().unwrap();
            for sid in sids {
                sessions.remove(&sid);
            }
        }
        self.spans.lock().unwrap().remove(agent_id);
        self.api_errors.lock().unwrap().remove(agent_id);
    }

    /// Recent tool spans, oldest→newest — the per-agent waterfall seam
    /// (consumers land with the detail panel's waterfall view).
    #[allow(dead_code)]
    pub fn spans(&self, agent_id: &str) -> Vec<ToolSpan> {
        self.spans
            .lock()
            .unwrap()
            .get(agent_id)
            .cloned()
            .unwrap_or_default()
    }

    /// The freshest session's (id, model) for one agent — fills the
    /// `sessionId`/`model` fields upstream's AgentUsageSample carries that
    /// our breaker-facing UsageSample does not. Public for the breaker beat's
    /// recordSession fallback.
    pub fn freshest_session_pub(&self, agent_id: &str) -> Option<(String, String)> {
        self.freshest_session(agent_id)
    }

    fn freshest_session(&self, agent_id: &str) -> Option<(String, String)> {
        let sids = self.agent_sessions.lock().unwrap().get(agent_id).cloned()?;
        let sessions = self.sessions.lock().unwrap();
        let mut best: Option<(String, u64, String)> = None;
        for sid in sids {
            if let Some(a) = sessions.get(&sid) {
                if best.as_ref().map(|(_, ts, _)| a.ts >= *ts).unwrap_or(true) {
                    best = Some((sid.clone(), a.ts, a.model.clone()));
                }
            }
        }
        best.map(|(sid, _, model)| (sid, model))
    }

    /// Cold-start backfill — port of upstream telemetry:snapshot: one usage row
    /// per agent with live telemetry + the full span rings, so a renderer that
    /// missed the live pushes can hydrate in one call.
    pub fn snapshot(&self) -> serde_json::Value {
        let agent_ids: Vec<String> = self
            .agent_sessions
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let mut usage = Vec::new();
        for id in &agent_ids {
            if let Some(s) = self.sample(id) {
                let mut v = serde_json::to_value(&s).unwrap_or_else(|_| serde_json::json!({}));
                v["agentId"] = serde_json::json!(id);
                if let Some((sid, model)) = self.freshest_session(id) {
                    v["sessionId"] = serde_json::json!(sid);
                    v["model"] = serde_json::json!(model);
                }
                usage.push(v);
            }
        }
        let spans: serde_json::Map<String, serde_json::Value> = self
            .spans
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    serde_json::to_value(v).unwrap_or_else(|_| serde_json::json!([])),
                )
            })
            .collect();
        serde_json::json!({ "usage": usage, "spans": spans })
    }

    /// Drain the agent's api_error stamps — the beat feeds each one to
    /// breaker.record_error (push vs pull is equivalent here: the breaker
    /// just timestamps each stamp again at feed time).
    pub fn take_api_errors(&self, agent_id: &str) -> Vec<u64> {
        self.api_errors
            .lock()
            .unwrap()
            .remove(agent_id)
            .unwrap_or_default()
    }

    // ─── ingest ──────────────────────────────────────────────────────────────

    fn session(&self, agent_id: &str, session_id: &str) {
        self.agent_sessions
            .lock()
            .unwrap()
            .entry(agent_id.to_string())
            .or_default()
            .insert(session_id.to_string());
        self.sessions
            .lock()
            .unwrap()
            .entry(session_id.to_string())
            .or_default();
    }

    fn ingest_metrics(&self, body: &Value, app: Option<&tauri::AppHandle>) {
        let now = now_ms();
        let mut touched: HashSet<String> = HashSet::new();
        for rm in body
            .get("resourceMetrics")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            let res_attrs = flatten_attrs(rm.pointer("/resource/attributes"));
            for sm in rm
                .get("scopeMetrics")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
            {
                for metric in sm
                    .get("metrics")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                {
                    let name = metric.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let points = metric
                        .pointer("/sum/dataPoints")
                        .or_else(|| metric.pointer("/gauge/dataPoints"))
                        .and_then(|v| v.as_array());
                    for dp in points.into_iter().flatten() {
                        let attrs = flatten_attrs(dp.get("attributes"));
                        let agent_id =
                            s(attrs.get("agent.id").or_else(|| res_attrs.get("agent.id")));
                        let session_id = s(attrs.get("session.id"));
                        if agent_id.is_empty() || session_id.is_empty() {
                            continue;
                        }
                        self.session(&agent_id, &session_id);
                        let model = crate::usage::normalize_model(&s(attrs.get("model")));
                        let value = point_value(dp);
                        {
                            let mut sessions = self.sessions.lock().unwrap();
                            // `forget` (agent teardown) can drop the session
                            // between `self.session()` and this lock — skip
                            // the point instead of unwrapping into an abort.
                            let Some(accum) = sessions.get_mut(&session_id) else {
                                continue;
                            };
                            if !model.is_empty() {
                                accum.model = model;
                            }
                            accum.ts = now;
                            match name {
                                "claude_code.token.usage" => {
                                    match s(attrs.get("type")).as_str() {
                                        "input" => accum.input += value as u64,
                                        "output" => accum.output += value as u64,
                                        "cacheRead" => accum.cache_read += value as u64,
                                        "cacheCreation" => accum.cache_creation += value as u64,
                                        _ => {}
                                    }
                                    touched.insert(agent_id.clone());
                                }
                                "claude_code.cost.usage" => {
                                    accum.usd += value;
                                    touched.insert(agent_id.clone());
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        for id in touched {
            if let (Some(app), Some(s)) = (app, self.sample(&id)) {
                // telemetry:event {kind:'usage', sample: AgentUsageSample} —
                // the sample carries sessionId + model (freshest session),
                // not just the token totals (the renderer reads both).
                let mut v = serde_json::to_value(&s).unwrap_or_else(|_| serde_json::json!({}));
                v["agentId"] = serde_json::json!(id);
                if let Some((sid, model)) = self.freshest_session(&id) {
                    v["sessionId"] = serde_json::json!(sid);
                    v["model"] = serde_json::json!(model);
                }
                let _ = app.emit(
                    "telemetry:event",
                    serde_json::json!({ "kind": "usage", "sample": v }),
                );
            }
        }
    }

    fn ingest_logs(&self, body: &Value, app: Option<&tauri::AppHandle>) {
        let now = now_ms();
        for rl in body
            .get("resourceLogs")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            let res_attrs = flatten_attrs(rl.pointer("/resource/attributes"));
            for sl in rl
                .get("scopeLogs")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
            {
                for lr in sl
                    .get("logRecords")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                {
                    let attrs = flatten_attrs(lr.get("attributes"));
                    let name =
                        s(attrs.get("event.name")).if_empty(|| s(lr.pointer("/body/stringValue")));
                    let agent_id = s(attrs.get("agent.id").or_else(|| res_attrs.get("agent.id")));
                    if agent_id.is_empty() {
                        continue;
                    }
                    let session_id = s(attrs.get("session.id"));
                    match name.as_str() {
                        "tool_result" => {
                            let span = ToolSpan {
                                agent_id: agent_id.clone(),
                                session_id,
                                ts: now,
                                tool: s(attrs.get("tool_name")).if_empty(|| "tool".into()),
                                success: truthy(attrs.get("success")),
                                duration_ms: f(attrs.get("duration_ms")),
                                decision: None,
                            };
                            let mut spans = self.spans.lock().unwrap();
                            let ring = spans.entry(agent_id.clone()).or_default();
                            ring.push(span.clone());
                            if ring.len() > SPAN_RING_CAP {
                                ring.drain(..ring.len() - SPAN_RING_CAP);
                            }
                            drop(spans);
                            if let Some(app) = app {
                                let _ = app.emit(
                                    "telemetry:event",
                                    serde_json::json!({ "kind": "tool_result", "span": span }),
                                );
                            }
                        }
                        "tool_decision" => {
                            let decision = if s(attrs.get("decision")) == "reject" {
                                "reject"
                            } else {
                                "accept"
                            };
                            if let Some(ring) = self.spans.lock().unwrap().get_mut(&agent_id) {
                                if let Some(last) = ring.last_mut() {
                                    last.decision = Some(decision.into());
                                }
                            }
                        }
                        "api_error" => {
                            self.api_errors
                                .lock()
                                .unwrap()
                                .entry(agent_id.clone())
                                .or_default()
                                .push(now);
                            if let Some(app) = app {
                                let _ = app.emit(
                                    "telemetry:event",
                                    serde_json::json!({
                                        "kind": "api_error", "agentId": agent_id,
                                        "sessionId": session_id, "ts": now,
                                        "error": s(attrs.get("error").or_else(|| attrs.get("message")))
                                    }),
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// tiny_http serve loop — POSTs only, /v1/metrics + /v1/logs; the OTLP
    /// success response is an empty JSON ExportServiceResponse. Binds port 0
    /// (dynamic loopback, same as the HookServer) and returns the bound port
    /// for the OTEL_EXPORTER_OTLP_ENDPOINT env wiring.
    pub fn run(self: std::sync::Arc<Self>, app: tauri::AppHandle) -> anyhow::Result<u16> {
        let server = tiny_http::Server::http(("127.0.0.1", 0))
            .map_err(|e| anyhow::anyhow!("telemetry bind: {e}"))?;
        let bound = match server.server_addr() {
            tiny_http::ListenAddr::IP(a) => a.port(),
            #[allow(unreachable_patterns)]
            _ => 0,
        };
        let me = self;
        std::thread::spawn(move || loop {
            let mut req = match server.recv_timeout(Duration::from_millis(500)) {
                Ok(Some(r)) => r,
                Ok(None) => continue,
                Err(_) => continue,
            };
            if req.method().as_str() != "POST" {
                let _ = req.respond(tiny_http::Response::empty(405));
                continue;
            }
            let mut body = Vec::new();
            let mut take = std::io::Read::take(req.as_reader(), MAX_BODY_BYTES + 1);
            let over = std::io::Read::read_to_end(&mut take, &mut body)
                .map(|_| body.len() as u64 > MAX_BODY_BYTES)
                .unwrap_or(true);
            let url = req.url().to_string();
            if over {
                let _ = req.respond(tiny_http::Response::empty(413));
                continue;
            }
            if let Ok(v) = serde_json::from_slice::<Value>(&body) {
                if url.contains("/v1/metrics") {
                    me.ingest_metrics(&v, Some(&app));
                } else if url.contains("/v1/logs") {
                    me.ingest_logs(&v, Some(&app));
                }
            }
            let _ = req.respond(
                tiny_http::Response::from_string("{}").with_header(
                    "Content-Type: application/json"
                        .parse::<tiny_http::Header>()
                        .unwrap(),
                ),
            );
        });
        Ok(bound)
    }
}

trait IfEmpty {
    fn if_empty(self, f: impl FnOnce() -> String) -> String;
}
impl IfEmpty for String {
    fn if_empty(self, f: impl FnOnce() -> String) -> String {
        if self.is_empty() {
            f()
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A realistic OTLP/HTTP-JSON metrics batch the way Claude Code emits it.
    fn otlp_metrics() -> Value {
        json!({
            "resourceMetrics": [{
                "resource": { "attributes": [
                    {"key": "agent.id", "value": {"stringValue": "w1"}}
                ]},
                "scopeMetrics": [{
                    "metrics": [
                        {"name": "claude_code.token.usage", "sum": {"dataPoints": [
                            {"attributes": [
                                {"key": "session.id", "value": {"stringValue": "s1"}},
                                {"key": "model", "value": {"stringValue": "claude-opus-4-8[1m]"}},
                                {"key": "type", "value": {"stringValue": "input"}}
                            ], "asInt": "1200"},
                            {"attributes": [
                                {"key": "session.id", "value": {"stringValue": "s1"}},
                                {"key": "type", "value": {"stringValue": "output"}}
                            ], "asInt": "300"}
                        ]}},
                        {"name": "claude_code.cost.usage", "sum": {"dataPoints": [
                            {"attributes": [
                                {"key": "session.id", "value": {"stringValue": "s1"}}
                            ], "asDouble": 0.037}
                        ]}}
                    ]
                }]
            }]
        })
    }

    #[test]
    fn metrics_accumulate_into_usage_sample() {
        let t = TelemetryCollector::new();
        t.ingest_metrics(&otlp_metrics(), None);
        let s = t.sample("w1").expect("sample exists");
        assert_eq!(s.input, 1200);
        assert_eq!(s.output, 300);
        assert!((s.usd - 0.037).abs() < 1e-9);
        assert!(s.ts > 0);
        assert!(t.has_telemetry("w1"));
        assert!(t.sample("nobody").is_none());
    }

    #[test]
    fn logs_feed_spans_and_api_errors() {
        let t = TelemetryCollector::new();
        t.ingest_logs(
            &json!({"resourceLogs": [{"scopeLogs": [{"logRecords": [
                {"attributes": [
                    {"key": "event.name", "value": {"stringValue": "tool_result"}},
                    {"key": "agent.id", "value": {"stringValue": "w1"}},
                    {"key": "session.id", "value": {"stringValue": "s1"}},
                    {"key": "tool_name", "value": {"stringValue": "Edit"}},
                    {"key": "success", "value": {"boolValue": true}},
                    {"key": "duration_ms", "value": {"intValue": "42"}}
                ]},
                {"attributes": [
                    {"key": "event.name", "value": {"stringValue": "api_error"}},
                    {"key": "agent.id", "value": {"stringValue": "w1"}},
                    {"key": "error", "value": {"stringValue": "rate_limit"}}
                ]}
            ]}]}]}),
            None,
        );
        let spans = t.spans("w1");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].tool, "Edit");
        assert!((spans[0].duration_ms - 42.0).abs() < 1e-9);
        assert_eq!(t.take_api_errors("w1").len(), 1);
        assert_eq!(t.take_api_errors("w1").len(), 0); // drained
    }
}
