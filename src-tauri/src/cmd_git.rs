//! Tauri command wrappers for the git pane — the IPC surface named git:* in
//! the Electron app (git:branch, git:status, git:log, git:branches,
//! git:aheadBehind, git:isRepo, git:diff, git:mainRepo, git:worktrees,
//! git:checkout, git:commitFiles, git:compareRefs, git:showFile,
//! git:logGraph). Every wrapper delegates to git_ops and translates
//! Option → "not a git repository" / passes through the Err(String) the
//! ops layer already produced.

use std::path::Path;

use serde_json::Value;
use tauri::AppHandle;

fn not_repo() -> String {
    "not a git repository".to_string()
}

#[tauri::command(async)]
pub fn git_is_repo(app: AppHandle, cwd: String) -> Result<Value, String> {
    let _ = app;
    Ok(Value::Bool(crate::git_ops::is_repo(Path::new(&cwd))))
}

#[tauri::command(async)]
pub fn git_branch(app: AppHandle, cwd: String) -> Result<Value, String> {
    let _ = app;
    crate::git_ops::branch(Path::new(&cwd)).ok_or_else(not_repo)
}

#[tauri::command(async)]
pub fn git_status(app: AppHandle, cwd: String) -> Result<Value, String> {
    let _ = app;
    crate::git_ops::status(Path::new(&cwd)).ok_or_else(not_repo)
}

#[tauri::command(async)]
pub fn git_log(app: AppHandle, cwd: String, n: Option<usize>) -> Result<Value, String> {
    let _ = app;
    crate::git_ops::log(Path::new(&cwd), n.unwrap_or(50).min(500))
        .map(Value::from)
        .ok_or_else(not_repo)
}

#[tauri::command(async)]
pub fn git_log_graph(
    app: AppHandle,
    cwd: String,
    n: Option<usize>,
    skip: Option<usize>,
) -> Result<Value, String> {
    let _ = app;
    crate::git_ops::log_graph(
        Path::new(&cwd),
        n.unwrap_or(200).min(2000),
        skip.unwrap_or(0),
    )
    .map(Value::from)
    .ok_or_else(not_repo)
}

#[tauri::command(async)]
pub fn git_branches(app: AppHandle, cwd: String) -> Result<Value, String> {
    let _ = app;
    crate::git_ops::branches(Path::new(&cwd))
        .map(Value::from)
        .ok_or_else(not_repo)
}

#[tauri::command(async)]
pub fn git_ahead_behind(app: AppHandle, cwd: String) -> Result<Value, String> {
    let _ = app;
    crate::git_ops::ahead_behind(Path::new(&cwd)).ok_or_else(not_repo)
}

#[tauri::command(async)]
pub fn git_main_repo(app: AppHandle, cwd: String) -> Result<Value, String> {
    let _ = app;
    crate::git_ops::main_repo_root(Path::new(&cwd))
        .map(Value::from)
        .ok_or_else(not_repo)
}

/// git:diff — upstream semantics: the working-tree diff of ONE file
/// (relPath). The rev-range variant survives as git:diffRefs.
#[tauri::command(async)]
pub fn git_diff(app: AppHandle, cwd: String, rel_path: String) -> Result<Value, String> {
    let _ = app;
    crate::git_ops::diff_file_worktree(Path::new(&cwd), &rel_path).map(Value::from)
}

#[tauri::command(async)]
pub fn git_diff_refs(
    app: AppHandle,
    cwd: String,
    rev_a: String,
    rev_b: String,
    path: Option<String>,
) -> Result<Value, String> {
    let _ = app;
    crate::git_ops::diff(Path::new(&cwd), &rev_a, &rev_b, path.as_deref())
        .map(Value::from)
}

#[tauri::command(async)]
pub fn git_checkout(app: AppHandle, cwd: String, r#ref: String, detach: Option<bool>) -> Result<Value, String> {
    let _ = app;
    crate::git_ops::checkout(Path::new(&cwd), &r#ref, detach.unwrap_or(false))
}

#[tauri::command(async)]
pub fn git_worktrees(app: AppHandle, cwd: String) -> Result<Value, String> {
    let _ = app;
    crate::git_ops::worktrees(Path::new(&cwd))
        .map(Value::from)
        .ok_or_else(not_repo)
}

#[tauri::command(async)]
pub fn git_worktree_add(
    app: AppHandle,
    cwd: String,
    path: String,
    branch: Option<String>,
    base_rev: Option<String>,
) -> Result<Value, String> {
    let _ = app;
    crate::git_ops::add_worktree(
        Path::new(&cwd),
        Path::new(&path),
        branch.as_deref(),
        base_rev.as_deref(),
    )
}

#[tauri::command(async)]
pub fn git_worktree_remove(app: AppHandle, path: String) -> Result<Value, String> {
    let _ = app;
    crate::git_ops::remove_worktree(Path::new(&path))
}

#[tauri::command(async)]
pub fn git_commit_files(app: AppHandle, cwd: String, sha: String) -> Result<Value, String> {
    let _ = app;
    crate::git_ops::commit_files(Path::new(&cwd), &sha).map(Value::from)
}

#[tauri::command(async)]
pub fn git_compare_refs(
    app: AppHandle,
    cwd: String,
    base: String,
    head: String,
) -> Result<Value, String> {
    let _ = app;
    crate::git_ops::compare_refs(Path::new(&cwd), &base, &head)
}

#[tauri::command(async)]
pub fn git_show_file(
    app: AppHandle,
    cwd: String,
    rev: String,
    path: String,
) -> Result<Value, String> {
    let _ = app;
    crate::git_ops::show_file(Path::new(&cwd), &rev, &path)
}
