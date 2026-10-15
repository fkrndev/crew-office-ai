//! Tauri command wrappers for the git pane — the IPC surface named git:* in
//! the Electron app (git:branch, git:status, git:log, git:branches,
//! git:aheadBehind, git:isRepo, git:diff, git:mainRepo, git:worktrees,
//! git:checkout, git:commitFiles, git:compareRefs, git:showFile,
//! git:logGraph). Every wrapper delegates to git_ops and translates
//! Option → "not a git repository" / passes through the Err(String) the
//! ops layer already produced.

use std::path::Path;

use crate::lock_ext::LockExt;
use serde_json::Value;
use tauri::AppHandle;

fn not_repo() -> String {
    "not a git repository".to_string()
}

/// Compare-ready form of a path: forward slashes, no trailing slash, and
/// case-folded on Windows (paths there are case-insensitive).
fn norm_path(p: &str) -> String {
    let s = p.replace('\\', "/");
    let s = s.trim_end_matches('/');
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s.to_string()
    }
}

/// Is `pty_cwd` the repo dir or somewhere beneath it?
fn cwd_within(pty_cwd: &str, root: &str) -> bool {
    let (p, r) = (norm_path(pty_cwd), norm_path(root));
    p == r || p.starts_with(&format!("{r}/"))
}

/// The id of a live pty working inside `cwd` that printed in the last 10s.
fn busy_pty_in(app: &AppHandle, cwd: &str) -> Option<String> {
    let st = crate::commands::state(app);
    let now = crate::misc::now_ms();
    let ptys = st.ptys.lock_safe();
    ptys.iter()
        .find(|(_, h)| {
            cwd_within(&h.cwd, cwd)
                && now.saturating_sub(h.last_output_at.load(std::sync::atomic::Ordering::Relaxed))
                    < 10_000
        })
        .map(|(id, _)| id.clone())
}

#[tauri::command(async)]
pub fn git_is_repo(app: AppHandle, cwd: String) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("git_is_repo", || {
    let _ = app;
    Ok(Value::Bool(crate::git_ops::is_repo(Path::new(&cwd))))
    })
}

#[tauri::command(async)]
pub fn git_branch(app: AppHandle, cwd: String) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("git_branch", || {
    let _ = app;
    crate::git_ops::branch(Path::new(&cwd)).ok_or_else(not_repo)
    })
}

#[tauri::command(async)]
pub fn git_status(app: AppHandle, cwd: String) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("git_status", || {
    let _ = app;
    crate::git_ops::status_entries(Path::new(&cwd)).ok_or_else(not_repo)
    })
}

#[tauri::command(async)]
pub fn git_log(app: AppHandle, cwd: String, n: Option<usize>) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("git_log", || {
    let _ = app;
    crate::git_ops::log(Path::new(&cwd), n.unwrap_or(50).min(500))
        .map(Value::from)
        .ok_or_else(not_repo)
    })
}

#[tauri::command(async)]
pub fn git_log_graph(
    app: AppHandle,
    cwd: String,
    n: Option<usize>,
    skip: Option<usize>,
) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("git_log_graph", || {
    let _ = app;
    crate::git_ops::log_graph(
        Path::new(&cwd),
        n.unwrap_or(200).min(2000),
        skip.unwrap_or(0),
    )
    .map(Value::from)
    .ok_or_else(not_repo)
    })
}

#[tauri::command(async)]
pub fn git_branches(app: AppHandle, cwd: String) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("git_branches", || {
    let _ = app;
    crate::git_ops::branches_payload(Path::new(&cwd)).ok_or_else(not_repo)
    })
}

#[tauri::command(async)]
pub fn git_ahead_behind(app: AppHandle, cwd: String) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("git_ahead_behind", || {
    let _ = app;
    crate::git_ops::ahead_behind(Path::new(&cwd)).ok_or_else(not_repo)
    })
}

#[tauri::command(async)]
pub fn git_main_repo(app: AppHandle, cwd: String) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("git_main_repo", || {
    let _ = app;
    crate::git_ops::main_repo_root(Path::new(&cwd))
        .map(Value::from)
        .ok_or_else(not_repo)
    })
}

/// git:diff — upstream semantics: the working-tree diff of ONE file
/// (relPath). The rev-range variant survives as git:diffRefs.
#[tauri::command(async)]
pub fn git_diff(app: AppHandle, cwd: String, rel_path: String) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("git_diff", || {
    let _ = app;
    crate::git_ops::diff_file_worktree(Path::new(&cwd), &rel_path).map(Value::from)
    })
}

#[tauri::command(async)]
pub fn git_diff_refs(
    app: AppHandle,
    cwd: String,
    rev_a: String,
    rev_b: String,
    path: Option<String>,
) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("git_diff_refs", || {
    let _ = app;
    crate::git_ops::diff(Path::new(&cwd), &rev_a, &rev_b, path.as_deref()).map(Value::from)
    })
}

#[tauri::command(async)]
pub fn git_checkout(
    app: AppHandle,
    cwd: String,
    r#ref: String,
    detach: Option<bool>,
) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("git_checkout", || {
    // Guard: never swap files under an actively-working agent. Any live pty
    // whose cwd sits in this tree and emitted output in the last 10s is
    // treated as mid-run (upstream git:checkout). Idle terminals are fine —
    // checkout also requires a clean tree.
    if let Some(busy) = busy_pty_in(&app, &cwd) {
        return Err(format!(
            "an agent is actively working in this repo ({busy}) — try again when it goes quiet"
        ));
    }
    crate::git_ops::checkout(Path::new(&cwd), &r#ref, detach.unwrap_or(false))
    })
}

#[tauri::command(async)]
pub fn git_worktrees(app: AppHandle, cwd: String) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("git_worktrees", || {
    let _ = app;
    crate::git_ops::worktrees(Path::new(&cwd))
        .map(Value::from)
        .ok_or_else(not_repo)
    })
}

#[tauri::command(async)]
pub fn git_worktree_add(
    app: AppHandle,
    cwd: String,
    path: String,
    branch: Option<String>,
    base_rev: Option<String>,
) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("git_worktree_add", || {
    let _ = app;
    crate::git_ops::add_worktree(
        Path::new(&cwd),
        Path::new(&path),
        branch.as_deref(),
        base_rev.as_deref(),
    )
    })
}

#[tauri::command(async)]
pub fn git_worktree_remove(app: AppHandle, path: String) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("git_worktree_remove", || {
    let _ = app;
    crate::git_ops::remove_worktree(Path::new(&path))
    })
}

#[tauri::command(async)]
pub fn git_commit_files(app: AppHandle, cwd: String, sha: String) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("git_commit_files", || {
    let _ = app;
    crate::git_ops::commit_files(Path::new(&cwd), &sha).map(Value::from)
    })
}

#[tauri::command(async)]
pub fn git_compare_refs(
    app: AppHandle,
    cwd: String,
    base: String,
    head: String,
    mode: Option<String>,
) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("git_compare_refs", || {
    let _ = app;
    // 'two' = literal base→head; anything else is the PR-style three-dot.
    crate::git_ops::compare_refs(Path::new(&cwd), &base, &head, mode.as_deref() == Some("two"))
    })
}

#[tauri::command(async)]
pub fn git_show_file(
    app: AppHandle,
    cwd: String,
    rev: String,
    path: String,
) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("git_show_file", || {
    let _ = app;
    crate::git_ops::show_file(Path::new(&cwd), &rev, &path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_guard_matches_the_repo_dir_and_its_subdirs_only() {
        assert!(cwd_within("C:/repo", "C:/repo"));
        assert!(cwd_within("C:/repo/sub/dir", "C:/repo"));
        assert!(cwd_within("C:/repo/sub", "C:/repo/"));
        // sibling with a shared prefix is NOT inside
        assert!(!cwd_within("C:/repo-other", "C:/repo"));
        assert!(!cwd_within("C:/other", "C:/repo"));
        // separators are normalised
        assert!(cwd_within("C:\\repo\\sub", "C:/repo"));
        #[cfg(windows)]
        assert!(cwd_within("c:/REPO/sub", "C:/repo"));
    }
}
