//! Loopback sync-IPC — the `ipcRenderer.sendSync` surface upstream exposes
//! (`roster:readSync`, `clipboard:readSync`) that Tauri's async-only invoke
//! cannot reproduce. A synchronous XMLHttpRequest against a loopback HTTP
//! server blocks the renderer thread exactly like sendSync — WebView2 still
//! honours sync XHR (deprecated but functional), the same mechanism
//! tauri-invoke-http uses for its invoke transport.
//!
//! GET /sync/roster    → the roster.json mirror (rosterReadSync)
//! GET /sync/clipboard → the OS clipboard text (readClipboardSync)

use serde_json::json;
use std::io::Read;
use tauri::AppHandle;

/// Bind on an ephemeral loopback port and serve forever. The port goes back
/// to the caller, which hands it to the renderer (the shim's sync calls need
/// it before they can block).
pub fn start(app: AppHandle) -> Option<u16> {
    let server = tiny_http::Server::http(("127.0.0.1", 0)).ok()?;
    let port = server.server_addr().to_ip().map(|a| a.port())?;
    std::thread::spawn(move || loop {
        let Ok(mut req) = server.recv() else {
            continue;
        };
        if req.method().as_str() != "GET" {
            let _ = req.respond(tiny_http::Response::from_string("").with_status_code(405));
            continue;
        }
        // Discard any request body (a sync XHR GET shouldn't carry one, but
        // an unread body would wedge the connection).
        let mut buf = Vec::new();
        let _ = req.as_reader().take(4096).read_to_end(&mut buf);
        let st = crate::commands::state(&app);
        let (status, body) = match req.url() {
            "/sync/roster" => match st.roster.read() {
                Some(v) => (200, v.to_string()),
                None => (200, "null".to_string()),
            },
            "/sync/clipboard" => {
                use tauri_plugin_clipboard_manager::ClipboardExt;
                let text = app.clipboard().read_text().unwrap_or_default();
                (200, json!({ "text": text }).to_string())
            }
            _ => (404, "null".to_string()),
        };
        let _ = req.respond(
            tiny_http::Response::from_string(body)
                .with_status_code(status)
                .with_header(
                    tiny_http::Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json"[..],
                    )
                    .unwrap(),
                ),
        );
    });
    Some(port)
}
