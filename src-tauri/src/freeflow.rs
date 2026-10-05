//! Free Flow — Groq Whisper speech-to-text, run from the Tauri backend
//! (port of `src/main/freeflow.ts`).
//!
//! The frontend captures mic audio (getUserMedia → MediaRecorder) and hands the
//! bytes here over IPC as base64; this module does the multipart upload to
//! Groq's OpenAI-compatible transcription endpoint and returns the transcript.
//! Doing the HTTP call in the backend (like `slack.rs` / `webhook.rs`) keeps
//! the user's Groq key out of the renderer.
//!
//! ureq 3 has a `multipart` feature but it isn't enabled in Cargo.toml, so the
//! `multipart/form-data` body is assembled by hand with a boundary string —
//! fields in the same order upstream's FormData emitted them (model,
//! response_format, language?, file).
//!
//! The API key comes from the OS credential store (keyring service
//! "agent-office", key `GROQ_API_KEY`), is used ONLY for the Authorization
//! header, and is NEVER logged.

use std::time::Duration;

use serde::Serialize;

/// Groq's OpenAI-compatible transcription endpoint.
const GROQ_TRANSCRIBE_URL: &str = "https://api.groq.com/openai/v1/audio/transcriptions";

/// Default model — fast, multilingual; ~216× real-time. The other option is
/// the higher-accuracy `whisper-large-v3`.
pub const DEFAULT_GROQ_MODEL: &str = "whisper-large-v3-turbo";

/// Groq free-tier upload cap is 25 MB; reject larger payloads before spending
/// a network round-trip (our clips are seconds long / tens of KB in practice).
const MAX_AUDIO_BYTES: usize = 25 * 1024 * 1024;

/// Don't let a hung request wedge the feature — bound the call. (Upstream used
/// 60s; 30s per the port spec — either way the feature can't wedge.)
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Default)]
pub struct TranscribeOptions {
    /// User's Groq API key. Used only for the Authorization header; never logged.
    pub api_key: String,
    /// Raw audio bytes captured in the renderer (e.g. webm/opus).
    pub audio: Vec<u8>,
    /// MIME type of `audio` (e.g. 'audio/webm'). Defaults to 'audio/webm'.
    pub mime_type: Option<String>,
    /// Upload filename (Groq infers format from the extension). Defaults to a webm name.
    pub filename: Option<String>,
    /// Groq model id. Defaults to DEFAULT_GROQ_MODEL.
    pub model: Option<String>,
    /// Optional ISO-639-1 language hint to improve accuracy/latency.
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TranscribeResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl TranscribeResult {
    fn err(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            text: None,
            error: Some(error.into()),
        }
    }
}

/// Transcribe a single audio clip via Groq Whisper. Returns `{ ok, text }` on
/// success or `{ ok: false, error }` otherwise. Never panics; never logs the key.
pub fn transcribe_with_groq(opts: &TranscribeOptions) -> TranscribeResult {
    if opts.api_key.is_empty() {
        return TranscribeResult::err("missing Groq API key");
    }
    if opts.audio.is_empty() {
        return TranscribeResult::err("empty audio");
    }
    if opts.audio.len() > MAX_AUDIO_BYTES {
        return TranscribeResult::err("audio too large (Groq free-tier cap is 25 MB)");
    }

    let mime_type = opts.mime_type.as_deref().unwrap_or("audio/webm");
    let filename = opts.filename.as_deref().unwrap_or("dictation.webm");
    let model = opts.model.as_deref().unwrap_or(DEFAULT_GROQ_MODEL);

    // `response_format=text` returns the bare transcript, but JSON is more
    // robust to parse defensively; we ask for json and read `.text`.
    let mut fields: Vec<(&str, &str)> = vec![("model", model), ("response_format", "json")];
    if let Some(lang) = opts.language.as_deref().filter(|l| !l.is_empty()) {
        fields.push(("language", lang));
    }
    let (body, boundary) = multipart_body(&fields, "file", filename, mime_type, &opts.audio);

    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        // Read the body even on 4xx/5xx so Groq's error message survives.
        .http_status_as_error(false)
        .build()
        .new_agent();

    let result = agent
        .post(GROQ_TRANSCRIBE_URL)
        .header("authorization", &format!("Bearer {}", opts.api_key))
        .content_type(&format!("multipart/form-data; boundary={boundary}"))
        .send(&body);

    match result {
        Err(e) => TranscribeResult::err(e.to_string()),
        Ok(mut resp) => {
            let status = resp.status().as_u16();
            let raw = resp.body_mut().read_to_string().unwrap_or_default();
            if !(200..300).contains(&status) {
                // Surface Groq's error message (NOT the key) — e.g. 401
                // invalid_api_key, 413 too large, 429 rate limited.
                let detail = extract_error(&raw);
                return TranscribeResult::err(format!(
                    "Groq {status}: {}",
                    if detail.is_empty() {
                        "request failed"
                    } else {
                        &detail
                    }
                ));
            }
            let text = serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .and_then(|j| {
                    j.get("text")
                        .and_then(|t| t.as_str())
                        .map(|s| s.trim().to_string())
                })
                // response_format fallback: a bare-text body.
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| raw.trim().to_string());
            if text.is_empty() {
                return TranscribeResult::err("no speech detected");
            }
            TranscribeResult {
                ok: true,
                text: Some(text),
                error: None,
            }
        }
    }
}

/// Assemble a `multipart/form-data` body: text fields first (in caller order),
/// then the file part, then the closing boundary. Returns (body, boundary).
fn multipart_body(
    fields: &[(&str, &str)],
    file_field: &str,
    filename: &str,
    mime: &str,
    bytes: &[u8],
) -> (Vec<u8>, String) {
    let boundary = format!("----agent-office-{}", uuid::Uuid::new_v4().simple());
    let mut out = Vec::with_capacity(bytes.len() + 512);
    for (name, value) in fields {
        let safe_name: String = name
            .chars()
            .filter(|c| !matches!(c, '"' | '\r' | '\n'))
            .collect();
        out.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{safe_name}\"\r\n\r\n{value}\r\n")
                .as_bytes(),
        );
    }
    let safe_filename: String = filename
        .chars()
        .filter(|c| !matches!(c, '"' | '\r' | '\n'))
        .collect();
    out.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{file_field}\"; filename=\"{safe_filename}\"\r\nContent-Type: {mime}\r\n\r\n"
        )
        .as_bytes(),
    );
    out.extend_from_slice(bytes);
    out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (out, boundary)
}

/// Pull a human-readable message out of Groq's JSON error envelope, if present.
fn extract_error(raw: &str) -> String {
    let Ok(j) = serde_json::from_str::<serde_json::Value>(raw) else {
        return String::new();
    };
    match j.get("error") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(e) => e
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or_default()
            .to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_inputs_fail_closed() {
        let r = transcribe_with_groq(&TranscribeOptions::default());
        assert!(!r.ok);
        assert_eq!(r.error.as_deref(), Some("missing Groq API key"));
        let r = transcribe_with_groq(&TranscribeOptions {
            api_key: "k".into(),
            ..Default::default()
        });
        assert_eq!(r.error.as_deref(), Some("empty audio"));
    }

    #[test]
    fn multipart_has_all_parts() {
        let (body, b) = multipart_body(
            &[
                ("model", "m"),
                ("response_format", "json"),
                ("language", "en"),
            ],
            "file",
            "clip.webm",
            "audio/webm",
            b"AUDIO",
        );
        let s = String::from_utf8_lossy(&body);
        assert!(s.contains(&format!("--{b}\r\n")));
        assert!(s.contains("name=\"model\"\r\n\r\nm"));
        assert!(s.contains("name=\"language\"\r\n\r\nen"));
        assert!(s.contains("filename=\"clip.webm\"\r\nContent-Type: audio/webm\r\n\r\nAUDIO"));
        assert!(s.ends_with(&format!("--{b}--\r\n")));
    }
}
