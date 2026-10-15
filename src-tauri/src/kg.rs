//! Knowledge Graph core — port of `src/main/kg-core.cjs` (ingest, index, and
//! keyword-search a local, file-backed enterprise knowledge store).
//!
//! Store layout (rooted at `kg_root`, default `<harness>/knowledge`):
//!   index.jsonl          one JSON line per CHUNK (the search index)
//!   docs/<docId>/
//!     meta.json          artifact metadata
//!     original.<ext>     raw artifact, copied verbatim (images/PDFs/binaries)
//!     text.md            full extracted/normalized searchable text
//!
//! Pure local TF-style scoring — no embeddings, same storage format and ranking
//! as the JS core so a store written by either implementation reads the same.
//!
//! Flag/root parity with knowledge.ts: `knowledgeGraph.enabled` /
//! `knowledgeGraph.rootPath` are read from the raw `<harness>/config.json`.
//! DEVIATION: `enabled` defaults to TRUE when the key is absent (upstream
//! defaults OFF) — the Rust Config struct has no knowledgeGraph field and a
//! config save would drop the key, so an explicit `"enabled": false` is the
//! only durable off-switch for now.

use crate::lock_ext::LockExt;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ─── Tunables ────────────────────────────────────────────────────────────────
pub const DEFAULT_CHUNK_SIZE: usize = 1200; // chars per chunk (approx; broken on boundaries)
pub const DEFAULT_CHUNK_OVERLAP: usize = 150; // chars of overlap between adjacent chunks
const MAX_INDEX_BYTES: usize = 5 * 1024 * 1024; // per-doc text indexed before truncation
const DEFAULT_SEARCH_LIMIT: usize = 8;
const SNIPPET_RADIUS: usize = 160; // chars of context on each side of a match
const PDF_TIMEOUT_MS: u64 = 20_000;

/// Serializes index/search writes — the JS core relied on the event loop.
static KG_LOCK: Mutex<()> = Mutex::new(());

// Tiny English stop-list — enough to stop the most common words from dominating
// term-frequency scores without pulling in an NLP dependency.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "from", "has", "have", "how",
    "in", "is", "it", "its", "of", "on", "or", "that", "the", "their", "this", "to", "was", "were",
    "what", "when", "where", "which", "who", "will", "with", "your", "you", "we", "our", "us",
    "they", "them", "he", "she", "his", "her", "i", "me", "my", "do", "does", "did", "not", "can",
    "could", "would", "should", "about", "into", "over", "than", "then", "there", "here", "so",
    "if", "no", "yes",
];

const IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "tif", "tiff", "heic",
];
const CODE_EXTS: &[&str] = &[
    "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "rs", "java", "rb", "c", "h", "cc", "cpp",
    "hpp", "cs", "php", "swift", "kt", "scala", "sh", "bash", "zsh", "sql",
];
const SHEET_EXTS: &[&str] = &["csv", "tsv"];
const TEXT_EXTS: &[&str] = &[
    "md", "markdown", "mdx", "txt", "text", "rst", "json", "jsonl", "yaml", "yml", "toml", "ini",
    "log", "html", "htm", "css", "xml", "env",
];

fn mime_for_ext(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "md" | "markdown" => "text/markdown",
        "txt" => "text/plain",
        "json" => "application/json",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "yaml" | "yml" => "application/yaml",
        "html" | "htm" => "text/html",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

// ─── Small helpers ───────────────────────────────────────────────────────────
/// A doc id is a single path segment under `docs/` — `[A-Za-z0-9_-]{1,64}`
/// (gen_id emits 16 hex). Anything else (`..`, separators, drive prefixes,
/// absolute paths) would let a renderer-supplied id walk out of the store.
pub fn is_valid_doc_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn gen_id() -> String {
    // crypto.randomBytes(8).toString('hex') → 16 hex chars.
    uuid::Uuid::new_v4().simple().to_string()[..16].to_string()
}

pub fn ext_of(p: &str) -> String {
    Path::new(p)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

fn mime_for(p: &str) -> String {
    mime_for_ext(&ext_of(p))
        .unwrap_or("application/octet-stream")
        .to_string()
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn clamp_limit(n: Option<u32>, fallback: usize) -> usize {
    match n {
        Some(v) if v > 0 => (v as usize).min(100),
        _ => fallback,
    }
}

/// Map a file (by extension) to a coarse modality bucket.
pub fn detect_modality(file_path: &str) -> &'static str {
    let e = ext_of(file_path);
    if IMAGE_EXTS.contains(&e.as_str()) {
        return "image";
    }
    if e == "pdf" {
        return "pdf";
    }
    if SHEET_EXTS.contains(&e.as_str()) {
        return "sheet";
    }
    if CODE_EXTS.contains(&e.as_str()) {
        return "code";
    }
    if TEXT_EXTS.contains(&e.as_str()) {
        return "text";
    }
    "text" // unknown → best-effort treat as text
}

fn is_stopword(t: &str) -> bool {
    STOPWORDS.contains(&t)
}

/// Lowercase alphanumeric tokens, stop-words and 1-char tokens dropped.
pub fn tokenize(s: &str) -> Vec<String> {
    let lower = s.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() {
            cur.push(ch);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.into_iter()
        .filter(|t| t.chars().count() > 1 && !is_stopword(t))
        .collect()
}

/// First markdown heading or first non-empty line, trimmed and length-capped.
pub fn derive_title(text: &str) -> String {
    for raw in text.split('\n') {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let h = line.trim_start_matches('#').trim();
        return if h.chars().count() > 80 {
            let cut: String = h.chars().take(80).collect();
            format!("{cut}…")
        } else {
            h.to_string()
        };
    }
    String::new()
}

/**
 * Split text into ~size-char chunks, preferring to break on paragraph / line /
 * word boundaries, with `overlap` chars carried into the next chunk so a passage
 * straddling a boundary is still findable. Deterministic; always terminates.
 */
pub fn chunk_text(text: &str, size: usize, overlap: usize) -> Vec<String> {
    let size = size.max(200);
    let overlap = overlap.min(size / 2);
    let t = text.replace("\r\n", "\n").trim().to_string();
    if t.is_empty() {
        return vec![];
    }
    let chars: Vec<char> = t.chars().collect();
    if chars.len() <= size {
        return vec![t];
    }
    let mut chunks = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let mut end = (i + size).min(chars.len());
        if end < chars.len() {
            let window = &chars[i..end];
            // max(lastIndexOf '\n\n', lastIndexOf '\n', lastIndexOf ' ')
            let para = window
                .windows(2)
                .rposition(|w| w[0] == '\n' && w[1] == '\n');
            let line = window.iter().rposition(|&c| c == '\n');
            let word = window.iter().rposition(|&c| c == ' ');
            let br = [para, line, word].into_iter().flatten().max().unwrap_or(0);
            if (br as f64) > size as f64 * 0.6 {
                end = i + br; // only break late, never near the start
            }
        }
        let piece: String = chars[i..end].iter().collect::<String>().trim().to_string();
        if !piece.is_empty() {
            chunks.push(piece);
        }
        if end >= chars.len() {
            break;
        }
        i = end.saturating_sub(overlap).max(i + 1);
    }
    chunks
}

/// One index.jsonl line.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IndexRec {
    pub doc_id: String,
    pub title: String,
    pub source: String,
    pub modality: String,
    pub chunk_idx: i64,
    pub text: String,
}

/**
 * Score one index record against the query. Term-frequency (log-damped) over
 * distinct query terms, a title-match boost, a distinct-term coverage bonus, and
 * an exact-phrase bonus. Returns 0 when no query term appears.
 */
pub fn score_chunk(rec: &IndexRec, query_terms: &[String], query_raw: &str) -> f64 {
    if query_terms.is_empty() {
        return 0.0;
    }
    let title_lc = rec.title.to_lowercase();
    let mut tf: HashMap<String, u32> = HashMap::new();
    for tok in tokenize(&rec.text) {
        *tf.entry(tok).or_insert(0) += 1;
    }
    let mut score = 0.0f64;
    let mut matched = 0u32;
    for term in query_terms {
        let c = tf.get(term).copied().unwrap_or(0);
        if c > 0 {
            matched += 1;
            score += 1.0 + (1.0 + c as f64).ln();
        }
        if title_lc.contains(term.as_str()) {
            score += 2.0;
        }
    }
    if matched == 0 {
        return 0.0;
    }
    score += matched as f64 * 0.5; // reward breadth of distinct matches
    let q = query_raw.trim().to_lowercase();
    if q.chars().count() >= 3 && rec.text.to_lowercase().contains(&q) {
        score += 5.0; // exact phrase
    }
    score
}

/// A short snippet of `text` centred on the first query-term hit.
pub fn make_snippet(text: &str, query_terms: &[String]) -> String {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        return String::new();
    }
    let lc = t.to_lowercase();
    let mut at: Option<usize> = None;
    for term in query_terms {
        if let Some(idx) = lc.find(term.as_str()) {
            if at.is_none_or(|a| idx < a) {
                at = Some(idx);
            }
        }
    }
    let t_len = t.chars().count();
    let Some(idx) = at else {
        return if t_len > SNIPPET_RADIUS * 2 {
            let cut: String = t.chars().take(SNIPPET_RADIUS * 2).collect();
            format!("{cut}…")
        } else {
            t
        };
    };
    let at_c = lc[..idx].chars().count();
    let start = at_c.saturating_sub(SNIPPET_RADIUS);
    let end = (at_c + SNIPPET_RADIUS).min(t_len);
    let slice: String = t
        .chars()
        .skip(start)
        .take(end - start)
        .collect::<String>()
        .trim()
        .to_string();
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        slice,
        if end < t_len { "…" } else { "" }
    )
}

// ─── Extraction (per modality) ───────────────────────────────────────────────
pub struct Extracted {
    pub text: String,
    pub title: String,
    pub extractor: String,
    pub mime: String,
}

struct ExtractInput<'a> {
    src_path: Option<&'a str>,
    inline_text: Option<&'a str>,
    modality: &'a str,
    title: Option<&'a str>,
    caption: Option<&'a str>,
    tags: &'a [String],
    source: &'a str,
}

/**
 * Turn an artifact into searchable text. v1 covers text-family (verbatim) and
 * images (metadata-level); PDF is a best-effort poppler hook; everything else is
 * treated as text.
 */
fn extract_text(input: &ExtractInput) -> Extracted {
    if let Some(inline) = input.inline_text.filter(|s| !s.is_empty()) {
        return Extracted {
            text: inline.to_string(),
            title: input
                .title
                .map(|t| t.to_string())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| {
                    let d = derive_title(inline);
                    if d.is_empty() {
                        input.source.to_string()
                    } else {
                        d
                    }
                }),
            extractor: "inline@1".into(),
            mime: "text/plain".into(),
        };
    }

    if input.modality == "image" {
        // No OCR in v1: the searchable surface is the user-supplied metadata + filename.
        let mut parts: Vec<String> = Vec::new();
        for p in [input.title, input.caption] {
            if let Some(s) = p.filter(|s| !s.is_empty()) {
                parts.push(s.to_string());
            }
        }
        parts.extend(input.tags.iter().cloned());
        if !input.source.is_empty() {
            parts.push(input.source.to_string());
        }
        return Extracted {
            text: parts.join("\n"),
            title: input
                .title
                .map(|t| t.to_string())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| input.source.to_string()),
            extractor: "image-meta@1".into(),
            mime: mime_for(input.src_path.unwrap_or("")),
        };
    }

    if input.modality == "pdf" {
        let src = input.src_path.unwrap_or("");
        if let Some(out) = try_pdf_to_text(src) {
            if !out.trim().is_empty() {
                return Extracted {
                    title: input
                        .title
                        .map(|t| t.to_string())
                        .filter(|t| !t.is_empty())
                        .unwrap_or_else(|| {
                            let d = derive_title(&out);
                            if d.is_empty() {
                                input.source.to_string()
                            } else {
                                d
                            }
                        }),
                    text: out,
                    extractor: "pdftotext@1".into(),
                    mime: "application/pdf".into(),
                };
            }
        }
        // poppler absent → store metadata now; a later pass can enrich from the original.
        let mut parts: Vec<String> = Vec::new();
        for p in [input.title, input.caption] {
            if let Some(s) = p.filter(|s| !s.is_empty()) {
                parts.push(s.to_string());
            }
        }
        parts.extend(input.tags.iter().cloned());
        if !input.source.is_empty() {
            parts.push(input.source.to_string());
        }
        return Extracted {
            text: parts.join("\n"),
            title: input
                .title
                .map(|t| t.to_string())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| input.source.to_string()),
            extractor: "pdf-pending@1".into(),
            mime: "application/pdf".into(),
        };
    }

    // text / code / sheet — read UTF-8 verbatim.
    if let Some(src) = input.src_path {
        if Path::new(src).exists() {
            let raw = fs::read_to_string(src).unwrap_or_default();
            return Extracted {
                title: input
                    .title
                    .map(|t| t.to_string())
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| {
                        let d = derive_title(&raw);
                        if d.is_empty() {
                            input.source.to_string()
                        } else {
                            d
                        }
                    }),
                text: raw,
                extractor: "text-utf8@1".into(),
                mime: mime_for(src),
            };
        }
    }

    Extracted {
        text: input.inline_text.unwrap_or("").to_string(),
        title: input
            .title
            .map(|t| t.to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| {
                if input.source.is_empty() {
                    "untitled".to_string()
                } else {
                    input.source.to_string()
                }
            }),
        extractor: "empty@1".into(),
        mime: "text/plain".into(),
    }
}

/// Best-effort PDF → text via poppler's `pdftotext` if it's on PATH; else None.
fn try_pdf_to_text(src_path: &str) -> Option<String> {
    if src_path.is_empty() || !Path::new(src_path).exists() {
        return None;
    }
    let mut child = Command::new("pdftotext")
        .args(["-q", src_path, "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Drain stdout on a thread so a >64KB conversion can't deadlock on a full pipe.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stdout, &mut buf);
        buf
    });
    let deadline = Instant::now() + std::time::Duration::from_millis(PDF_TIMEOUT_MS);
    let exited = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => break None,
        }
    };
    let buf = reader.join().unwrap_or_default();
    let s = exited?;
    if !s.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&buf).to_string())
}

// ─── Types shared with the JS core ───────────────────────────────────────────

/// docs/<id>/meta.json — field names match the JS core exactly (camelCase on disk).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KgMeta {
    pub id: String,
    pub title: String,
    pub source: String,
    pub modality: String,
    pub mime: Option<String>,
    pub orig_ext: String,
    pub bytes: u64,
    pub tags: Vec<String>,
    pub caption: Option<String>,
    pub chunk_count: u64,
    pub added_at: String,
    pub extractor: String,
    pub truncated: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KgHit {
    pub doc_id: String,
    pub title: String,
    pub source: String,
    pub modality: String,
    pub chunk_idx: i64,
    pub score: f64,
    pub snippet: String,
}

/// ingest input — `src_path` OR `text` (JS `input.text`/`inlineText`).
#[derive(Clone, Debug, Default)]
pub struct IngestInput {
    pub src_path: Option<String>,
    pub text: Option<String>,
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub caption: Option<String>,
    pub modality: Option<String>,
    pub source: Option<String>,
    pub id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestOutcome {
    pub doc_id: String,
    pub chunk_count: usize,
    pub meta: KgMeta,
}

// ─── Store operations ────────────────────────────────────────────────────────

/// Ingest one artifact into the store at `kg_root`.
pub fn ingest(kg_root: &Path, input: &IngestInput) -> Result<IngestOutcome, String> {
    let _guard = KG_LOCK.lock_safe();
    let docs_dir = kg_root.join("docs");
    fs::create_dir_all(&docs_dir).map_err(|e| format!("create docs dir: {e}"))?;

    let doc_id = input
        .id
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(gen_id);
    if !is_valid_doc_id(&doc_id) {
        return Err("invalid doc id".to_string());
    }
    let doc_dir = docs_dir.join(&doc_id);
    fs::create_dir_all(&doc_dir).map_err(|e| format!("create doc dir: {e}"))?;

    let src_path = input.src_path.clone().filter(|s| !s.is_empty());
    let inline_text = input.text.clone();
    let source = input
        .source
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            src_path
                .as_deref()
                .and_then(|p| Path::new(p).file_name())
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        })
        .or_else(|| input.title.clone())
        .unwrap_or_else(|| "untitled".to_string());
    let modality = input
        .modality
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            src_path
                .as_deref()
                .map(detect_modality)
                .unwrap_or("text")
                .to_string()
        });
    let orig_ext = src_path
        .as_deref()
        .map(|p| {
            let e = ext_of(p);
            if e.is_empty() {
                "bin".to_string()
            } else {
                e
            }
        })
        .unwrap_or_else(|| "txt".to_string());
    let tags: Vec<String> = input.tags.clone();

    // Copy the raw artifact verbatim (lossless record for future re-extraction).
    let mut bytes = 0u64;
    if let Some(src) = &src_path {
        let p = Path::new(src);
        if p.exists() {
            if let Ok(m) = p.metadata() {
                bytes = m.len();
            }
            let _ = fs::copy(p, doc_dir.join(format!("original.{orig_ext}")));
        }
    }

    let ex = extract_text(&ExtractInput {
        src_path: src_path.as_deref(),
        inline_text: inline_text.as_deref(),
        modality: &modality,
        title: input.title.as_deref(),
        caption: input.caption.as_deref(),
        tags: &tags,
        source: &source,
    });
    let full_text = ex.text;
    fs::write(doc_dir.join("text.md"), &full_text).map_err(|e| format!("write text.md: {e}"))?;

    let mut truncated = false;
    let index_text: String = if full_text.chars().count() > MAX_INDEX_BYTES {
        truncated = true;
        full_text.chars().take(MAX_INDEX_BYTES).collect()
    } else {
        full_text
    };

    let title = input
        .title
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if ex.title.is_empty() {
                source.clone()
            } else {
                ex.title.clone()
            }
        });
    let mut chunks = chunk_text(&index_text, DEFAULT_CHUNK_SIZE, DEFAULT_CHUNK_OVERLAP);
    if chunks.is_empty() {
        chunks = vec![title.clone()]; // images/empties: index the title so it's findable
    }

    let mut lines = String::new();
    for (i, c) in chunks.iter().enumerate() {
        let rec = IndexRec {
            doc_id: doc_id.clone(),
            title: title.clone(),
            source: source.clone(),
            modality: modality.clone(),
            chunk_idx: i as i64,
            text: c.clone(),
        };
        lines.push_str(&serde_json::to_string(&rec).unwrap_or_default());
        lines.push('\n');
    }
    let idx_path = kg_root.join("index.jsonl");
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&idx_path)
        .map_err(|e| format!("append index: {e}"))?;
    f.write_all(lines.as_bytes())
        .map_err(|e| format!("append index: {e}"))?;

    let meta = KgMeta {
        id: doc_id.clone(),
        title,
        source,
        modality,
        mime: Some(ex.mime),
        orig_ext,
        bytes,
        tags,
        caption: input.caption.clone(),
        chunk_count: chunks.len() as u64,
        added_at: now_iso(),
        extractor: ex.extractor,
        truncated,
        extra: serde_json::Map::new(),
    };
    let meta_json = serde_json::to_string_pretty(&meta).unwrap_or_default();
    fs::write(doc_dir.join("meta.json"), meta_json).map_err(|e| format!("write meta: {e}"))?;

    Ok(IngestOutcome {
        doc_id,
        chunk_count: chunks.len(),
        meta,
    })
}

/// Keyword-search the index. Ranked hits: { docId, title, source, modality, chunkIdx, score, snippet }.
pub fn search(kg_root: &Path, query: &str, limit: Option<u32>) -> Vec<KgHit> {
    let limit = clamp_limit(limit, DEFAULT_SEARCH_LIMIT);
    let query_terms = tokenize(query);
    if query_terms.is_empty() {
        return vec![];
    }
    let idx_path = kg_root.join("index.jsonl");
    let Ok(raw) = fs::read_to_string(&idx_path) else {
        return vec![];
    };

    let mut scored: Vec<(IndexRec, f64)> = Vec::new();
    for line in raw.lines() {
        let s = line.trim();
        if s.is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<IndexRec>(s) else {
            continue;
        };
        let sc = score_chunk(&rec, &query_terms, query);
        if sc > 0.0 {
            scored.push((rec, sc));
        }
    }
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.doc_id.cmp(&b.0.doc_id))
            .then_with(|| a.0.chunk_idx.cmp(&b.0.chunk_idx))
    });

    scored
        .into_iter()
        .take(limit)
        .map(|(rec, score)| KgHit {
            doc_id: rec.doc_id,
            title: rec.title,
            source: rec.source,
            modality: rec.modality,
            chunk_idx: rec.chunk_idx,
            score: (score * 1000.0).round() / 1000.0,
            snippet: make_snippet(&rec.text, &query_terms),
        })
        .collect()
}

/// All artifacts' metadata, newest first.
pub fn list(kg_root: &Path) -> Vec<KgMeta> {
    let docs_dir = kg_root.join("docs");
    let Ok(rd) = fs::read_dir(&docs_dir) else {
        return vec![];
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let meta_path = e.path().join("meta.json");
        if let Ok(text) = fs::read_to_string(&meta_path) {
            if let Ok(m) = serde_json::from_str::<KgMeta>(&text) {
                out.push(m);
            }
        }
    }
    out.sort_by(|a, b| b.added_at.cmp(&a.added_at));
    out
}

/// Full record for one artifact: { meta, text } or None.
pub fn get_doc(kg_root: &Path, doc_id: &str) -> Option<(KgMeta, String)> {
    if !is_valid_doc_id(doc_id) {
        return None;
    }
    let doc_dir = kg_root.join("docs").join(doc_id);
    if !doc_dir.exists() {
        return None;
    }
    let meta: KgMeta =
        serde_json::from_str(&fs::read_to_string(doc_dir.join("meta.json")).ok()?).ok()?;
    let text = fs::read_to_string(doc_dir.join("text.md")).unwrap_or_default();
    Some((meta, text))
}

/// Delete one artifact's folder and drop its lines from the index.
pub fn remove_doc(kg_root: &Path, doc_id: &str) -> bool {
    let _guard = KG_LOCK.lock_safe();
    if !is_valid_doc_id(doc_id) {
        return false;
    }
    let doc_dir = kg_root.join("docs").join(doc_id);
    if !doc_dir.exists() {
        return false;
    }
    // A failed delete must not report success (or drop the index lines of a
    // doc that is still on disk).
    if fs::remove_dir_all(&doc_dir).is_err() {
        return false;
    }
    let idx_path = kg_root.join("index.jsonl");
    if let Ok(raw) = fs::read_to_string(&idx_path) {
        let kept: String = raw
            .lines()
            .filter(|line| {
                let s = line.trim();
                if s.is_empty() {
                    return false;
                }
                match serde_json::from_str::<Value>(s) {
                    Ok(v) => v.get("docId").and_then(|d| d.as_str()) != Some(doc_id),
                    Err(_) => false,
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let out = if kept.is_empty() {
            String::new()
        } else {
            format!("{kept}\n")
        };
        let _ = fs::write(&idx_path, out);
    }
    true
}

/// Corpus counts for the UI/CLI status line.
pub fn stats(kg_root: &Path) -> Value {
    let docs = list(kg_root);
    let mut chunk_count = 0u64;
    let mut by_modality: BTreeMap<String, u64> = BTreeMap::new();
    for d in &docs {
        chunk_count += d.chunk_count;
        *by_modality.entry(d.modality.clone()).or_insert(0) += 1;
    }
    json!({
        "docCount": docs.len(),
        "chunkCount": chunk_count,
        "byModality": by_modality,
    })
}

// ─── knowledge.ts façade (config-gated root + status) ───────────────────────

/// Read `knowledgeGraph` from the raw harness config.json (the Rust Config
/// struct has no field for it — DEVIATION noted at top: absent ⇒ enabled).
pub fn knowledge_config(harness_home: &Path) -> (bool, Option<String>) {
    let cfg_path = harness_home.join("config.json");
    let Ok(text) = fs::read_to_string(&cfg_path) else {
        return (true, None);
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return (true, None);
    };
    let kg = v.get("knowledgeGraph").or_else(|| v.get("knowledge_graph"));
    let enabled = kg
        .and_then(|k| k.get("enabled"))
        .and_then(|e| e.as_bool())
        .unwrap_or(true);
    let root = kg
        .and_then(|k| k.get("rootPath").or_else(|| k.get("root_path")))
        .and_then(|r| r.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    (enabled, root)
}

/// The store directory (config override or <harness>/knowledge).
pub fn kg_root(harness_home: &Path) -> PathBuf {
    let (_, root) = knowledge_config(harness_home);
    match root {
        Some(r) => PathBuf::from(r),
        None => harness_home.join("knowledge"),
    }
}

/// knowledge.status() shape: { enabled, root, docCount, chunkCount, byModality }.
/// Counts zero when disabled — same contract as upstream (enabled && existsSync).
pub fn status(harness_home: &Path) -> Value {
    let (enabled, _) = knowledge_config(harness_home);
    let root = kg_root(harness_home);
    let s = if enabled && root.exists() {
        stats(&root)
    } else {
        json!({ "docCount": 0, "chunkCount": 0, "byModality": {} })
    };
    json!({
        "enabled": enabled,
        "root": root.display().to_string(),
        "docCount": s["docCount"].clone(),
        "chunkCount": s["chunkCount"].clone(),
        "byModality": s["byModality"].clone(),
    })
}

// ─── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    struct TmpDir(PathBuf);
    impl TmpDir {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "kgtest-{}-{}",
                name,
                uuid::Uuid::new_v4().simple()
            ));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn tokenize_drops_stopwords_and_short_tokens() {
        let toks = tokenize("The quick brown fox is in a den");
        assert_eq!(toks, vec!["quick", "brown", "fox", "den"]);
        assert!(tokenize("a i of").is_empty());
    }

    #[test]
    fn chunk_text_respects_boundaries_and_overlap() {
        let body = (0..60)
            .map(|i| format!("paragraph number {i} with some filler words to pad it out\n\n"))
            .collect::<String>();
        let chunks = chunk_text(&body, 300, 40);
        assert!(chunks.len() > 1);
        // coverage: rejoin-ish — every chunk nonempty, first starts at 0.
        assert!(chunks[0].starts_with("paragraph number 0"));
        // small text is one chunk, trimmed
        assert_eq!(
            chunk_text("  hello world  ", 1200, 150),
            vec!["hello world"]
        );
        // empty → nothing
        assert!(chunk_text("   ", 1200, 150).is_empty());
        // always terminates on pathological input
        let nasty = "x".repeat(5000);
        assert!(!chunk_text(&nasty, 300, 40).is_empty());
    }

    #[test]
    fn derive_title_uses_first_heading() {
        assert_eq!(derive_title("\n\n# My Doc\nbody"), "My Doc");
        assert_eq!(derive_title("plain line\nsecond"), "plain line");
    }

    #[test]
    fn ingest_search_get_remove_round_trip() {
        let tmp = TmpDir::new("roundtrip");
        let root = tmp.0.join("knowledge");

        let out = ingest(
            &root,
            &IngestInput {
                text: Some(
                    "Rust ownership rules: each value has a single owner. \
                     When the owner goes out of scope the value is dropped. \
                     Borrowing lets you reference without taking ownership."
                        .into(),
                ),
                title: Some("Ownership Notes".into()),
                tags: vec!["rust".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert!(out.chunk_count >= 1);

        // search finds it
        let hits = search(&root, "ownership borrowing", None);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].doc_id, out.doc_id);
        assert!(hits[0].score > 0.0);
        assert!(!hits[0].snippet.is_empty());
        assert!(hits[0].snippet.contains("ownership") || hits[0].snippet.contains("Owner"));

        // list + get
        let docs = list(&root);
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].id, out.doc_id);
        assert_eq!(docs[0].title, "Ownership Notes");
        let (meta, text) = get_doc(&root, &out.doc_id).unwrap();
        assert_eq!(meta.id, out.doc_id);
        assert!(text.contains("single owner"));

        // stats
        let s = stats(&root);
        assert_eq!(s["docCount"], 1);
        assert_eq!(s["byModality"]["text"], 1);

        // remove → gone from list/get/index
        assert!(remove_doc(&root, &out.doc_id));
        assert!(get_doc(&root, &out.doc_id).is_none());
        assert!(search(&root, "ownership borrowing", None).is_empty());
        // idempotent
        assert!(!remove_doc(&root, &out.doc_id));
    }

    #[test]
    fn ingest_file_from_disk_and_second_doc() {
        let tmp = TmpDir::new("file");
        let root = tmp.0.join("knowledge");
        let src = tmp.0.join("notes.md");
        fs::write(
            &src,
            "# Meeting notes\nWe decided to port the broker to Rust.\n",
        )
        .unwrap();

        let a = ingest(
            &root,
            &IngestInput {
                src_path: Some(src.display().to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(a.meta.modality, "text");
        assert_eq!(a.meta.source, "notes.md");
        assert_eq!(a.meta.orig_ext, "md");
        assert!(root
            .join("docs")
            .join(&a.doc_id)
            .join("original.md")
            .exists());

        // a second doc doesn't clobber the index — both searchable
        let b = ingest(
            &root,
            &IngestInput {
                text: Some("Totally different content about gardening tulips.".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(list(&root).len(), 2);
        assert!(search(&root, "broker rust", None)
            .iter()
            .any(|h| h.doc_id == a.doc_id));
        assert!(search(&root, "gardening tulips", None)
            .iter()
            .any(|h| h.doc_id == b.doc_id));
        // cross-doc isolation: broker search must not rank the tulip doc
        assert!(search(&root, "broker rust", None)
            .iter()
            .all(|h| h.doc_id != b.doc_id));
    }

    #[test]
    fn search_ranks_exact_phrase_and_title() {
        let tmp = TmpDir::new("rank");
        let root = tmp.0.join("knowledge");
        let phrase_doc = ingest(
            &root,
            &IngestInput {
                text: Some("the quick brown fox jumps over the lazy dog repeatedly".into()),
                title: Some("unrelated".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let weak = ingest(
            &root,
            &IngestInput {
                text: Some("fox fox fox quick quick dog".into()),
                title: Some("also unrelated".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let hits = search(&root, "quick brown fox", None);
        assert_eq!(hits.len(), 2);
        // exact-phrase bonus pushes the phrase doc ahead
        assert_eq!(hits[0].doc_id, phrase_doc.doc_id);
        assert_ne!(hits[0].doc_id, weak.doc_id);
    }

    #[test]
    fn empty_query_and_missing_index_are_safe() {
        let tmp = TmpDir::new("empty");
        let root = tmp.0.join("knowledge");
        assert!(search(&root, "", None).is_empty());
        assert!(search(&root, "anything", None).is_empty());
        assert!(list(&root).is_empty());
        assert!(get_doc(&root, "nope").is_none());
    }

    #[test]
    fn doc_ids_cannot_traverse_out_of_the_store() {
        for ok in ["abc123", "A_b-9", &"a".repeat(64), "0123456789abcdef"] {
            assert!(is_valid_doc_id(ok), "{ok}");
        }
        for bad in ["", "..", ".", "../x", "a/b", "a\\b", "C:\\Windows", "/etc", "a b", "é", &"a".repeat(65), "a.b", "x\0"] {
            assert!(!is_valid_doc_id(bad), "{bad:?}");
        }
        let tmp = TmpDir::new("traverse");
        let root = tmp.0.join("knowledge");
        // A sibling dir the traversal would otherwise delete / read.
        let victim = tmp.0.join("victim");
        fs::create_dir_all(&victim).unwrap();
        fs::write(victim.join("meta.json"), "{}").unwrap();
        fs::write(victim.join("keep.txt"), "x").unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        assert!(!remove_doc(&root, "../../victim"));
        assert!(!remove_doc(&root, ".."));
        assert!(victim.join("keep.txt").exists());
        assert!(get_doc(&root, "../../victim").is_none());
        // ingest refuses a hostile caller-supplied id and creates nothing.
        let err = ingest(
            &root,
            &IngestInput {
                id: Some("../../victim2".into()),
                text: Some("hello".into()),
                ..Default::default()
            },
        );
        assert!(err.is_err());
        assert!(!tmp.0.join("victim2").exists());
    }
}
