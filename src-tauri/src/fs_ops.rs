//! Confined filesystem access — port of the `safeResolve`/`openForRead` core
//! of `fs.ts` (the git pane + file browser's one path-escape policy).
//!
//! The boundary is the WORKSPACE, not "no symlinks at all": a link whose
//! target exists and is inside the workspace is followed (an ordinary
//! node_modules checkout stays browsable); what is refused is a link leaving
//! the workspace, and a DANGLING link whose unborn target can't be proven to
//! land inside it.

use std::fs;
use std::path::{Component, Path, PathBuf};

/// A read result the renderer can consume directly.
pub enum FsRead {
    Text(String),
    /// image/* bytes, base64 — preview pane data.
    Image { mime: String, base64: String },
    TooLarge(u64),
}

/// Max bytes a UI read loads — upstream's cap keeps a renderer IPC from
/// pulling a multi-GB file into memory.
const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;

const IMAGE_EXTS: [(&str, &str); 8] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("svg", "image/svg+xml"),
    ("bmp", "image/bmp"),
    ("ico", "image/x-icon"),
];

fn image_mime(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    IMAGE_EXTS.iter().find(|(e, _)| *e == ext).map(|(_, m)| *m)
}

/// Windows canonicalize() returns `\\?\D:\…` verbatim paths — strip the
/// prefix so lexical comparison against ordinary `D:\` forms works.
fn clean(p: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    p
}

/// Lexical containment: join root+rel, reject `..` escapes without touching
/// the filesystem (absolute rel is normalized first).
fn lexical_join(root: &Path, rel: &str) -> Option<PathBuf> {
    let abs_root = std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join(root))
        .unwrap_or_else(|| root.to_path_buf());
    let abs_root = clean(abs_root.canonicalize().unwrap_or_else(|_| abs_root));
    let rel_path = PathBuf::from(rel);
    let abs = if rel_path.is_absolute() {
        // An absolute rel must STILL land inside root — the canonicalize()
        // re-check below is the real gate (lexical prefix comparison can't
        // survive 8.3 short-name forms on Windows).
        normalize_lexical(&rel_path)
    } else {
        normalize_lexical(&abs_root.join(rel_path))
    };
    // Fast lexical escape rejection only when both sides compare cleanly;
    // anything else defers to canonicalize()'s real containment check.
    if let Ok(rel2) = abs.strip_prefix(&abs_root) {
        if rel2.components().any(|c| matches!(c, Component::ParentDir)) {
            return None;
        }
    }
    Some(abs)
}

/// Collapse `.`/`..` lexically (Component::ParentDir pops).
fn normalize_lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Canonicalize `abs` and re-check containment against `real_root`. The
/// deepest EXISTING ancestor is canonicalized and the unborn tail
/// re-attached — a to-be-created file inside a real workspace still works.
fn canonicalize(real_root: &Path, abs: &Path) -> Option<PathBuf> {
    let mut probe = abs.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match probe.canonicalize() {
            Ok(real) => {
                let mut full = clean(real);
                for t in tail.iter().rev() {
                    full.push(t);
                }
                let rel = full.strip_prefix(real_root).ok()?;
                if rel.components().any(|c| matches!(c, Component::ParentDir)) {
                    return None;
                }
                return Some(full);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let parent = probe.parent()?.to_path_buf();
                tail.push(probe.file_name()?.to_os_string());
                if parent == probe {
                    return None;
                }
                probe = parent;
            }
            Err(_) => return None,
        }
    }
}

/// Any symlink component of `abs` below `real_root`? Runs on the canonical
/// path — the only links left to catch are DANGLING ones (a write would
/// follow the link and create the file at its external target).
fn has_symlink_component(real_root: &Path, abs: &Path) -> bool {
    let Ok(rel) = abs.strip_prefix(real_root) else { return true };
    let mut cur = real_root.to_path_buf();
    for part in rel.components() {
        cur.push(part.as_os_str());
        match cur.symlink_metadata() {
            Ok(m) if m.file_type().is_symlink() => return true,
            Ok(_) => {}
            // NotFound: nothing deeper can exist → no link to find.
            // Anything else is unreadable metadata: fail closed.
            Err(e) => return e.kind() != std::io::ErrorKind::NotFound,
        }
    }
    false
}

/// Confine `rel` inside `root`, return the canonical absolute path — the ONE
/// path-escape policy (git.ts and the browser both resolve through this).
pub fn safe_resolve(root: &Path, rel: &str) -> Option<PathBuf> {
    let lex = lexical_join(root, rel)?;
    let real_root = root.canonicalize().ok().map(clean)?;
    let abs = canonicalize(&real_root, &lex)?;
    if has_symlink_component(&real_root, &abs) {
        return None;
    }
    Some(abs)
}

/// List a confined directory — entries carry name/kind/size/mtime/link flag.
pub fn list_dir(root: &Path, rel: &str) -> Option<Vec<serde_json::Value>> {
    let dir = safe_resolve(root, rel)?;
    let rd = fs::read_dir(&dir).ok()?;
    let mut out = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        let meta = p.symlink_metadata().ok()?;
        let ft = meta.file_type();
        let kind = if ft.is_symlink() {
            "link"
        } else if ft.is_dir() {
            "dir"
        } else {
            "file"
        };
        out.push(serde_json::json!({
            "name": e.file_name().to_string_lossy(),
            "kind": kind,
            "size": meta.len(),
            "mtime": meta.modified().ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        }));
    }
    out.sort_by(|a, b| {
        let ka = if a["kind"] == "dir" { 0 } else { 1 };
        let kb = if b["kind"] == "dir" { 0 } else { 1 };
        (ka, a["name"].to_string()).cmp(&(kb, b["name"].to_string()))
    });
    Some(out)
}

/// Read a confined file — text or image preview; over-cap → TooLarge.
pub fn read_file(root: &Path, rel: &str) -> Option<FsRead> {
    let p = safe_resolve(root, rel)?;
    let meta = fs::metadata(&p).ok()?;
    if meta.len() > MAX_READ_BYTES {
        return Some(FsRead::TooLarge(meta.len()));
    }
    if let Some(mime) = image_mime(&p) {
        let bytes = fs::read(&p).ok()?;
        use base64::Engine;
        return Some(FsRead::Image {
            mime: mime.to_string(),
            base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        });
    }
    let text = fs::read_to_string(&p).ok()?;
    Some(FsRead::Text(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oa-fs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("file.txt"), "hello").unwrap();
        std::fs::write(d.join("sub/deep.txt"), "deep").unwrap();
        d
    }

    #[test]
    fn traversal_and_abs_are_confined() {
        let d = ws("conf");
        assert!(safe_resolve(&d, "file.txt").is_some());
        assert!(safe_resolve(&d, "sub/../file.txt").is_some());
        assert!(safe_resolve(&d, "../outside.txt").is_none());
        assert!(safe_resolve(&d, "sub/../../x").is_none());
        // Absolute path inside root passes; outside root is refused.
        assert!(safe_resolve(&d, &d.join("file.txt").display().to_string()).is_some());
        assert!(safe_resolve(&d, r"C:\Windows\System32\drivers\etc\hosts").is_none());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn dangling_symlink_refused() {
        let d = ws("link");
        // A link to a not-yet-existing external target: canonicalize can't
        // prove it lands inside → the lstat walk must refuse it.
        #[cfg(unix)]
        std::os::unix::fs::symlink("/tmp/definitely-outside-oa-fs", d.join("dangling")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file("C:\\does-not-exist-oa-fs", d.join("dangling")).unwrap();
        assert!(safe_resolve(&d, "dangling").is_none());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn list_and_read() {
        let d = ws("lr");
        let entries = list_dir(&d, "").unwrap();
        assert!(entries.iter().any(|e| e["name"] == "sub" && e["kind"] == "dir"));
        match read_file(&d, "file.txt") {
            Some(FsRead::Text(t)) => assert_eq!(t, "hello"),
            _ => panic!("expected text"),
        }
        std::fs::remove_dir_all(&d).ok();
    }
}
