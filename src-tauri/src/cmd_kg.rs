//! IPC surface for the Knowledge Graph — port of index.ts `kg:*` handlers:
//!   kg:status       → kg_status
//!   kg:list         → kg_list
//!   kg:search       → kg_search(query, limit?)
//!   kg:get          → kg_get(id)
//!   kg:remove       → kg_remove(id)
//!   kg:ingestFiles  → kg_ingest_files(payload)
//!   kg:addFiles     → kg_add_files(payload)
//!
//! Upstream `kg:addFiles` opened an Electron file dialog; in Tauri the picker is
//! the renderer's job (dialog plugin), so this command takes explicit paths —
//! everything after selection is identical.

use std::path::PathBuf;

use serde_json::{json, Value};
use tauri::AppHandle;

use crate::commands::state;
use crate::kg;

/// <harness> = parent of the hive root (<harness>/hive).
fn harness_home(app: &AppHandle) -> PathBuf {
    let st = state(app);
    st.hive
        .root
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| st.hive.root.clone())
}

#[tauri::command]
pub fn kg_status(app: AppHandle) -> Result<Value, String> {
    Ok(kg::status(&harness_home(&app)))
}

#[tauri::command]
pub fn kg_list(app: AppHandle) -> Result<Value, String> {
    let root = kg::kg_root(&harness_home(&app));
    if !root.exists() {
        return Ok(json!([]));
    }
    Ok(json!(kg::list(&root)))
}

#[tauri::command]
pub fn kg_search(app: AppHandle, query: String, limit: Option<u32>) -> Result<Value, String> {
    if query.trim().is_empty() {
        return Ok(json!([]));
    }
    let root = kg::kg_root(&harness_home(&app));
    if !root.exists() {
        return Ok(json!([]));
    }
    Ok(json!(kg::search(&root, &query, limit)))
}

#[tauri::command]
pub fn kg_get(app: AppHandle, id: String) -> Result<Value, String> {
    if id.is_empty() {
        return Ok(Value::Null);
    }
    let root = kg::kg_root(&harness_home(&app));
    match kg::get_doc(&root, &id) {
        Some((meta, text)) => Ok(json!({ "meta": meta, "text": text })),
        None => Ok(Value::Null),
    }
}

#[tauri::command]
pub fn kg_remove(app: AppHandle, id: String) -> Result<Value, String> {
    let ok = !id.is_empty() && kg::remove_doc(&kg::kg_root(&harness_home(&app)), &id);
    Ok(json!({ "ok": ok }))
}

/// Ingest files by path and/or inline content. Accepts BOTH shapes:
///   upstream: { paths: ["/a/b.md", ...], tags: [...] }
///   extended: { files: [{ name, path?, content?|text?, title?, tags?, caption? }] }
/// Best-effort per file; per-file results so the UI can report partial success.
#[tauri::command]
pub fn kg_ingest_files(app: AppHandle, payload: Value) -> Result<Value, String> {
    let root = kg::kg_root(&harness_home(&app));
    let shared_tags = payload
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut results: Vec<Value> = Vec::new();

    // Upstream shape: {paths:[...]}
    if let Some(paths) = payload.get("paths").and_then(|p| p.as_array()) {
        for p in paths.iter().filter_map(|v| v.as_str()) {
            results.push(ingest_one(&root, p, None, None, &shared_tags, None));
        }
    }

    // Extended shape: {files:[{name,content|path,...}]}
    if let Some(files) = payload.get("files").and_then(|f| f.as_array()) {
        for f in files {
            let name = f.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let tags: Vec<String> = f
                .get("tags")
                .and_then(|t| t.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_else(|| shared_tags.clone());
            let title = f.get("title").and_then(|t| t.as_str()).map(|s| s.to_string());
            let caption = f
                .get("caption")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string());
            if let Some(path) = f.get("path").and_then(|p| p.as_str()) {
                results.push(ingest_one(&root, path, title, Some(name), &tags, caption));
            } else if let Some(content) = f
                .get("content")
                .or_else(|| f.get("text"))
                .and_then(|c| c.as_str())
            {
                let input = kg::IngestInput {
                    text: Some(content.to_string()),
                    title: title.or_else(|| {
                        if name.is_empty() {
                            None
                        } else {
                            Some(name.to_string())
                        }
                    }),
                    source: if name.is_empty() {
                        None
                    } else {
                        Some(name.to_string())
                    },
                    tags,
                    caption,
                    ..Default::default()
                };
                results.push(match kg::ingest(&root, &input) {
                    Ok(r) => json!({ "ok": true, "name": name, "docId": r.doc_id, "chunkCount": r.chunk_count }),
                    Err(e) => json!({ "ok": false, "name": name, "error": e }),
                });
            } else {
                results.push(json!({ "ok": false, "name": name, "error": "file needs path or content" }));
            }
        }
    }

    Ok(json!({ "results": results }))
}

/// "Add documents" — ingest explicit disk paths (the renderer's file picker
/// supplies them; upstream opened the dialog itself). Same per-file results.
#[tauri::command]
pub fn kg_add_files(app: AppHandle, payload: Option<Value>) -> Result<Value, String> {
    let root = kg::kg_root(&harness_home(&app));
    let p = payload.unwrap_or_else(|| json!({}));
    let mut paths: Vec<String> = p
        .get("paths")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    // Also accept a bare string array or {files:[path,...]} for convenience.
    if paths.is_empty() {
        if let Some(arr) = p.as_array() {
            paths = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        } else if let Some(arr) = p.get("files").and_then(|f| f.as_array()) {
            paths = arr
                .iter()
                .filter_map(|v| {
                    v.as_str()
                        .map(|s| s.to_string())
                        .or_else(|| v.get("path").and_then(|p| p.as_str()).map(|s| s.to_string()))
                })
                .collect();
        }
    };
    if paths.is_empty() {
        return Ok(json!({ "ok": false, "error": "cancelled" }));
    }
    let tags: Vec<String> = p
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let results: Vec<Value> = paths
        .iter()
        .map(|p| ingest_one(&root, p, None, None, &tags, None))
        .collect();
    Ok(json!({ "ok": true, "results": results }))
}

fn ingest_one(
    root: &std::path::Path,
    src_path: &str,
    title: Option<String>,
    source: Option<&str>,
    tags: &[String],
    caption: Option<String>,
) -> Value {
    let input = kg::IngestInput {
        src_path: Some(src_path.to_string()),
        title,
        source: source.map(|s| s.to_string()),
        tags: tags.to_vec(),
        caption,
        ..Default::default()
    };
    match kg::ingest(root, &input) {
        Ok(r) => json!({
            "ok": true,
            "srcPath": src_path,
            "docId": r.doc_id,
            "chunkCount": r.chunk_count,
        }),
        Err(e) => json!({ "ok": false, "srcPath": src_path, "error": e }),
    }
}
