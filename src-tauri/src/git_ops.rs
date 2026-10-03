//! Git pane backend — the read surface of `git.ts` (branch, status,
//! ahead/behind, log, worktree safety). Spawned `git` processes upstream are
//! replaced by git2 — same answers, no process per call.
//!
//! `safeResolve` stays the path policy: `cwd` is the agent's workspace
//! directory, resolved through the same confinement check the fs browser
//! uses before we open the repo.

use std::path::Path;

use serde_json::{json, Value};

/// Is the cwd inside a git worktree at all?
pub fn is_repo(cwd: &Path) -> bool {
    git2::Repository::discover(cwd).is_ok()
}

/// HEAD branch + whether it is detached — getBranch().
pub fn branch(cwd: &Path) -> Option<Value> {
    let repo = git2::Repository::discover(cwd).ok()?;
    let head = repo.head().ok();
    let (name, detached) = match &head {
        Some(h) if h.is_branch() => (
            h.shorthand().unwrap_or("HEAD").to_string(),
            false,
        ),
        Some(_) => (
            head.as_ref()
                .and_then(|h| h.target())
                .map(|oid| oid.to_string()[..7].to_string())
                .unwrap_or_else(|| "HEAD".into()),
            true,
        ),
        None => ("(no commits yet)".to_string(), false),
    };
    Some(json!({ "branch": name, "detached": detached }))
}

/// Working-tree counters — getStatus(): staged/unstaged/untracked counts +
/// whether the tree is clean.
pub fn status(cwd: &Path) -> Option<Value> {
    let repo = git2::Repository::discover(cwd).ok()?;
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(false)
        .exclude_submodules(true);
    let statuses = repo.statuses(Some(&mut opts)).ok()?;
    let (mut staged, mut unstaged, mut untracked) = (0u32, 0u32, 0u32);
    for s in statuses.iter() {
        let st = s.status();
        if st.contains(git2::Status::WT_NEW) && !st.intersects(git2::Status::INDEX_NEW) {
            untracked += 1;
            continue;
        }
        if st.intersects(
            git2::Status::INDEX_NEW
                | git2::Status::INDEX_MODIFIED
                | git2::Status::INDEX_DELETED
                | git2::Status::INDEX_RENAMED
                | git2::Status::INDEX_TYPECHANGE,
        ) {
            staged += 1;
        }
        if st.intersects(
            git2::Status::WT_MODIFIED | git2::Status::WT_DELETED | git2::Status::WT_TYPECHANGE,
        ) {
            unstaged += 1;
        }
    }
    Some(json!({
        "staged": staged, "unstaged": unstaged, "untracked": untracked,
        "clean": staged + unstaged + untracked == 0,
    }))
}

/// Ahead/behind the upstream tracking branch — getAheadBehind().
pub fn ahead_behind(cwd: &Path) -> Option<Value> {
    let repo = git2::Repository::discover(cwd).ok()?;
    let head = repo.head().ok()?;
    let local = head.target()?;
    let upstream = repo
        .branch_upstream_name(head.shorthand().unwrap_or(""))
        .ok()
        .and_then(|n| repo.find_branch(n.as_str().unwrap_or(""), git2::BranchType::Remote).ok())
        .and_then(|b| b.get().target());
    let Some(up) = upstream else {
        return Some(json!({ "ahead": 0, "behind": 0, "hasUpstream": false }));
    };
    let (a, b) = repo.graph_ahead_behind(local, up).ok()?;
    Some(json!({ "ahead": a, "behind": b, "hasUpstream": true }))
}

/// Recent commits — getLog(n): sha, subject, author, timestamp.
pub fn log(cwd: &Path, n: usize) -> Option<Vec<Value>> {
    let repo = git2::Repository::discover(cwd).ok()?;
    let mut walk = repo.revwalk().ok()?;
    walk.push_head().ok()?;
    walk.set_sorting(git2::Sort::TIME).ok()?;
    let mut out = Vec::new();
    for oid in walk.take(n).flatten() {
        if let Ok(c) = repo.find_commit(oid) {
            out.push(json!({
                "sha": oid.to_string(),
                "subject": c.summary().ok().flatten().unwrap_or(""),
                "author": c.author().name().unwrap_or(""),
                "ts": c.time().seconds() * 1000,
            }));
        }
    }
    Some(out)
}

/// Uncommitted-work guard for removing an agent worktree —
/// worktreeHasUnintegratedWork(): anything dirty OR unpushed means the
/// worktree still holds work a plain `rm` would lose.
pub fn worktree_has_unintegrated_work(cwd: &Path) -> Option<bool> {
    let s = status(cwd)?;
    if s["clean"].as_bool() != Some(true) {
        return Some(true);
    }
    let ab = ahead_behind(cwd)?;
    Some(ab["ahead"].as_u64().unwrap_or(0) > 0)
}
