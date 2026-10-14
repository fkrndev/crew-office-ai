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
        Some(h) if h.is_branch() => (h.shorthand().unwrap_or("HEAD").to_string(), false),
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
        .and_then(|n| {
            repo.find_branch(n.as_str().unwrap_or(""), git2::BranchType::Remote)
                .ok()
        })
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
    if ab["ahead"].as_u64().unwrap_or(0) > 0 {
        return Some(true);
    }
    // No upstream reports ahead=0, which proves nothing for a DETACHED head:
    // its commits live only in this worktree's HEAD. Keep it unless some ref
    // already contains HEAD (fails safe — any doubt says "has work").
    if ab["hasUpstream"].as_bool() != Some(true) {
        return Some(detached_head_unreferenced(cwd).unwrap_or(true));
    }
    Some(false)
}

/// True when HEAD is detached AND no branch/remote/tag ref contains it —
/// i.e. deleting the worktree would orphan its commits. A branch HEAD is
/// always referenced (the branch survives the worktree). `None` = unknown.
fn detached_head_unreferenced(cwd: &Path) -> Option<bool> {
    let repo = git2::Repository::discover(cwd).ok()?;
    let head = repo.head().ok()?;
    if head.is_branch() {
        return Some(false);
    }
    let oid = head.target()?;
    for r in repo.references().ok()?.flatten() {
        let Ok(n) = r.name() else { continue };
        if !(n.starts_with("refs/heads/")
            || n.starts_with("refs/remotes/")
            || n.starts_with("refs/tags/"))
        {
            continue;
        }
        if let Some(t) = r.target() {
            if t == oid || repo.graph_descendant_of(t, oid).unwrap_or(false) {
                return Some(false);
            }
        }
    }
    Some(true)
}

// ─── v0.3.4 port: history / compare / checkout plumbing ─────────────────────

/// Upstream caps text payloads at 2 MB (MAX_DIFF_BYTES / MAX_SHOW_BYTES).
const MAX_GIT_TEXT_BYTES: usize = 2 * 1024 * 1024;

/// Renderer-supplied revs/refs are untrusted — upstream isSafeRev() plus the
/// extra `..`/`/`/` ` rejections the Rust port requires. Conservative charset
/// (the one git refs actually use, plus `^~@{}` rev syntax), nonempty, ≤256,
/// no leading '-' (option injection), no `..` (range syntax), no
/// leading/trailing '/', no space.
pub fn is_safe_rev(rev: &str) -> bool {
    if rev.is_empty() || rev.len() > 256 || rev.starts_with('-') {
        return false;
    }
    if rev.contains("..") || rev.starts_with('/') || rev.ends_with('/') || rev.contains(' ') {
        return false;
    }
    rev.bytes().all(|b| {
        b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'.' | b'_' | b'/' | b'-' | b'^' | b'~' | b'@' | b'{' | b'}'
            )
    })
}

/// A repo-relative path the renderer hands us for rev-scoped lookups —
/// reject `..` components and absolute/leading-root forms before it ever
/// reaches a `rev:path` spec.
fn safe_rel_path(path: &str) -> Option<String> {
    let norm = path.replace('\\', "/");
    if norm.is_empty() || norm.starts_with('/') || norm.ends_with('/') {
        return None;
    }
    if norm
        .split('/')
        .any(|c| c.is_empty() || c == "." || c == "..")
    {
        return None;
    }
    Some(norm)
}

/// Local + remote branch list — getBranches(), extended with per-branch
/// upstream/ahead/behind for the pane's branch picker.
pub fn branches(cwd: &Path) -> Option<Vec<Value>> {
    let repo = git2::Repository::discover(cwd).ok()?;
    let mut out = Vec::new();
    for bt in [git2::BranchType::Local, git2::BranchType::Remote] {
        let is_remote = bt == git2::BranchType::Remote;
        for pair in repo.branches(Some(bt)).ok()?.flatten() {
            let (b, _kind) = pair;
            let Some(name) = b.name().ok().flatten().map(str::to_string) else {
                continue;
            };
            if is_remote && name.ends_with("/HEAD") {
                continue; // origin/HEAD symref — not a real branch entry
            }
            let mut upstream: Option<String> = None;
            let (mut ahead, mut behind) = (0u64, 0u64);
            if !is_remote {
                if let Ok(up) = b.upstream() {
                    upstream = up.name().ok().flatten().map(str::to_string);
                    if let (Some(l), Some(u)) = (b.get().target(), up.get().target()) {
                        if let Ok((a, d)) = repo.graph_ahead_behind(l, u) {
                            ahead = a as u64;
                            behind = d as u64;
                        }
                    }
                }
            }
            out.push(json!({
                "name": name,
                "isHead": b.is_head(),
                "isRemote": is_remote,
                "upstream": upstream,
                "ahead": ahead,
                "behind": behind,
            }));
        }
    }
    Some(out)
}

/// The MAIN working tree of the repo `cwd` belongs to — upstream
/// mainRepoRoot(): `git rev-parse --path-format=absolute --git-common-dir`
/// then strip a trailing `/.git`. For a linked worktree, repo.commondir()
/// resolves to the main checkout's `.git`, not `.git/worktrees/<name>`.
/// A bare repo has no workdir — its own path is the answer.
pub fn main_repo_root(cwd: &Path) -> Option<String> {
    let repo = git2::Repository::discover(cwd).ok()?;
    let git_dir = repo.commondir().to_string_lossy().replace('\\', "/");
    let trimmed = git_dir.trim_end_matches('/');
    let stripped = trimmed.strip_suffix("/.git").unwrap_or(trimmed);
    if stripped.is_empty() {
        return Some(trimmed.to_string());
    }
    Some(stripped.to_string())
}

fn map_err(e: &git2::Error) -> String {
    let m = e.message().trim_end();
    if m.is_empty() {
        e.to_string()
    } else {
        m.to_string()
    }
}

/// Unified diff text between two revs — `git diff <a> <b> [-- <path>]` as
/// upstream runs it, printed by libgit2 to patch format. Both revs gated by
/// is_safe_rev; `path` (optional) confines the diff to one tree entry.
pub fn diff(cwd: &Path, rev_a: &str, rev_b: &str, path: Option<&str>) -> Result<String, String> {
    if !is_safe_rev(rev_a) || !is_safe_rev(rev_b) {
        return Err("invalid revision".into());
    }
    let repo = git2::Repository::discover(cwd).map_err(|e| map_err(&e))?;
    let tree_of = |rev: &str| -> Result<git2::Tree<'_>, String> {
        repo.revparse_single(rev)
            .and_then(|o| o.peel_to_tree())
            .map_err(|e| map_err(&e))
    };
    let (ta, tb) = (tree_of(rev_a)?, tree_of(rev_b)?);
    let mut opts = git2::DiffOptions::new();
    if let Some(p) = path {
        let p = safe_rel_path(p).ok_or_else(|| "path escapes repository root".to_string())?;
        opts.pathspec(p);
    }
    let mut d = repo
        .diff_tree_to_tree(Some(&ta), Some(&tb), Some(&mut opts))
        .map_err(|e| map_err(&e))?;
    d.find_similar(None).map_err(|e| map_err(&e))?; // -M rename detection
    let mut buf = String::new();
    let mut too_big = false;
    let res = d.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        let bytes = line.content();
        if buf.len() + bytes.len() > MAX_GIT_TEXT_BYTES {
            too_big = true;
            return false;
        }
        buf.push_str(&String::from_utf8_lossy(bytes));
        true
    });
    if too_big {
        return Err("diff exceeds the 2 MB limit".into());
    }
    res.map_err(|e| map_err(&e))?;
    Ok(buf)
}

/// Upstream git:diff — the working-tree diff of ONE file (index vs workdir
/// with HEAD baseline), not a rev-range diff. Untracked files diff as
/// /dev/null → new file.
pub fn diff_file_worktree(cwd: &Path, rel_path: &str) -> Result<String, String> {
    let rel = safe_rel_path(rel_path).ok_or_else(|| "path escapes repository root".to_string())?;
    let repo = git2::Repository::discover(cwd).map_err(|e| map_err(&e))?;
    let mut opts = git2::DiffOptions::new();
    opts.pathspec(&rel)
        .include_untracked(true)
        .recurse_untracked_dirs(true);
    let head_tree = repo.head().and_then(|h| h.peel_to_tree()).ok();
    let mut d = repo
        .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))
        .map_err(|e| map_err(&e))?;
    d.find_similar(None).map_err(|e| map_err(&e))?;
    let mut buf = String::new();
    let mut too_big = false;
    let res = d.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        let bytes = line.content();
        if buf.len() + bytes.len() > MAX_GIT_TEXT_BYTES {
            too_big = true;
            return false;
        }
        buf.push_str(&String::from_utf8_lossy(bytes));
        true
    });
    if too_big {
        return Err("diff exceeds the 2 MB limit".into());
    }
    res.map_err(|e| map_err(&e))?;
    Ok(buf)
}

/// Is `branch` currently checked out in a linked worktree? `git switch` would
/// fail with "already checked out"; libgit2 won't — so we check ourselves and
/// name the holder like upstream does.
/// Linked worktree names — git2 0.21 yields Result<Option<&[u8]>>.
fn worktree_names(repo: &git2::Repository) -> Vec<String> {
    repo.worktrees()
        .map(|arr| arr.iter().flatten().flatten().map(str::to_string).collect())
        .unwrap_or_default()
}

fn branch_checked_out_elsewhere(repo: &git2::Repository, branch: &str) -> Option<String> {
    for name in worktree_names(repo) {
        if let Ok(wt) = repo.find_worktree(&name) {
            if let Ok(wrepo) = git2::Repository::open_from_worktree(&wt) {
                if let Ok(h) = wrepo.head() {
                    if h.is_branch() && h.shorthand().ok() == Some(branch) {
                        return Some(wt.path().to_string_lossy().into_owned());
                    }
                }
            }
        }
    }
    None
}

/// Guarded checkout — upstream checkoutRef(): refuse an unsafe rev, refuse a
/// dirty tree (staged+unstaged; untracked alone doesn't block), then switch.
/// `detach=true` forces detached HEAD even for a local branch name.
pub fn checkout(cwd: &Path, rev: &str, detach: bool) -> Result<Value, String> {
    if !is_safe_rev(rev) {
        return Err("invalid revision".into());
    }
    let repo = git2::Repository::discover(cwd).map_err(|e| map_err(&e))?;
    let s = status(cwd).ok_or_else(|| "could not verify a clean tree".to_string())?;
    let dirty = s["staged"].as_u64().unwrap_or(1) + s["unstaged"].as_u64().unwrap_or(1);
    if dirty > 0 {
        return Err(format!(
            "working tree has {dirty} uncommitted change{} — commit or stash first",
            if dirty == 1 { "" } else { "s" }
        ));
    }
    let obj = repo.revparse_single(rev).map_err(|e| map_err(&e))?;
    let commit = obj.peel_to_commit().map_err(|e| map_err(&e))?;
    let branch_ref = (!detach)
        .then(|| repo.find_branch(rev, git2::BranchType::Local).ok())
        .flatten();
    if branch_ref.is_some() {
        if let Some(holder) = branch_checked_out_elsewhere(&repo, rev) {
            return Err(format!(
                "'{rev}' is checked out in another worktree: {holder}"
            ));
        }
    }
    repo.checkout_tree(&obj, Some(git2::build::CheckoutBuilder::new().safe()))
        .map_err(|e| map_err(&e))?;
    let detached = match &branch_ref {
        Some(b) => {
            repo.set_head(b.get().name().unwrap_or("HEAD"))
                .map_err(|e| map_err(&e))?;
            false
        }
        None => {
            repo.set_head_detached(commit.id())
                .map_err(|e| map_err(&e))?;
            true
        }
    };
    Ok(json!({ "ok": true, "detached": detached }))
}

/// All worktrees in the family — `git worktree list --porcelain` equivalent.
/// Main worktree first (upstream's ordering), each entry
/// {path, head, branch|null, isMain}.
pub fn worktrees(cwd: &Path) -> Option<Vec<Value>> {
    let repo = git2::Repository::discover(cwd).ok()?;
    let mut out = Vec::new();
    let main_path = main_repo_root(cwd)?;
    let head_of = |r: &git2::Repository| -> (Option<String>, Option<String>) {
        match r.head() {
            Ok(h) => (
                h.target().map(|t| t.to_string()),
                if h.is_branch() {
                    h.shorthand().ok().map(str::to_string)
                } else {
                    None
                },
            ),
            Err(_) => (None, None),
        }
    };
    let (head, branch) = head_of(&repo);
    out.push(json!({
        "path": main_path, "head": head, "branch": branch, "isMain": true,
    }));
    for name in worktree_names(&repo) {
        let Ok(wt) = repo.find_worktree(&name) else {
            continue;
        };
        let wrepo = git2::Repository::open_from_worktree(&wt)
            .or_else(|_| git2::Repository::open(wt.path()));
        let (head, branch) = wrepo.as_ref().map(|r| head_of(r)).unwrap_or((None, None));
        out.push(json!({
            "path": wt.path().to_string_lossy().replace('\\', "/"),
            "head": head, "branch": branch, "isMain": false,
        }));
    }
    Some(out)
}

fn worktree_name_for(wt_path: &Path) -> String {
    let base = wt_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "agent".into());
    let slug: String = base
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "agent".into()
    } else {
        slug.to_string()
    }
}

fn add_wt_with_ref(
    repo: &git2::Repository,
    name: &str,
    wt_path: &Path,
    reference: &git2::Reference,
) -> Result<Value, String> {
    let mut opts = git2::WorktreeAddOptions::new();
    opts.reference(Some(reference));
    let wt = repo
        .worktree(name, wt_path, Some(&mut opts))
        .map_err(|e| map_err(&e))?;
    Ok(json!({
        "ok": true,
        "name": name,
        "path": wt.path().to_string_lossy().replace('\\', "/"),
    }))
}

/// Worktree add with HEAD detached at `commit`: libgit2 requires a reference
/// for the new worktree HEAD, so ride a throwaway branch, detach, delete it.
fn add_wt_detached(
    repo: &git2::Repository,
    name: &str,
    wt_path: &Path,
    commit: &git2::Commit,
) -> Result<Value, String> {
    let tmp = format!("wt-detached-{name}");
    let br = repo.branch(&tmp, commit, true).map_err(|e| map_err(&e))?;
    let res = add_wt_with_ref(repo, name, wt_path, br.get());
    match res {
        Ok(v) => {
            if let Ok(wt) = repo.find_worktree(name) {
                if let Ok(wrepo) = git2::Repository::open_from_worktree(&wt) {
                    let _ = wrepo.set_head_detached(commit.id());
                }
            }
            if let Ok(mut b) = repo.find_branch(&tmp, git2::BranchType::Local) {
                let _ = b.delete();
            }
            Ok(v)
        }
        Err(e) => {
            if let Ok(mut b) = repo.find_branch(&tmp, git2::BranchType::Local) {
                let _ = b.delete();
            }
            Err(e)
        }
    }
}

/// Provision a worktree — upstream addWorktree(): create `branch` off
/// `base_rev` and attach it; if the branch already exists (or -b fails),
/// fall back to checking `base_rev` out directly (branch ref if it names
/// one, detached at the commit otherwise — `git worktree add <path> <base>`).
pub fn add_worktree(
    repo_dir: &Path,
    wt_path: &Path,
    branch: Option<&str>,
    base_rev: Option<&str>,
) -> Result<Value, String> {
    let base_rev = base_rev.unwrap_or("HEAD");
    if !is_safe_rev(base_rev) {
        return Err("invalid revision".into());
    }
    if let Some(b) = branch {
        if !is_safe_rev(b) {
            return Err("invalid branch name".into());
        }
    }
    let repo = git2::Repository::discover(repo_dir).map_err(|e| map_err(&e))?;
    let commit = repo
        .revparse_single(base_rev)
        .and_then(|o| o.peel_to_commit())
        .map_err(|e| map_err(&e))?;
    let name = worktree_name_for(wt_path);
    if let Some(b) = branch {
        if let Ok(br) = repo.branch(b, &commit, false) {
            return add_wt_with_ref(&repo, &name, wt_path, &br.into_reference());
        }
        // Branch already exists (a respawn after teardown): resume ON it, so
        // the agent's earlier commits stay attached instead of the new
        // worktree silently landing on the base. If it can't be attached
        // (checked out elsewhere) → upstream's plain-add fallback below.
        if let Ok(br) = repo.find_branch(b, git2::BranchType::Local) {
            if let Ok(v) = add_wt_with_ref(&repo, &name, wt_path, &br.into_reference()) {
                return Ok(v);
            }
        }
    }
    if let Ok(r) = repo.resolve_reference_from_short_name(base_rev) {
        if r.is_branch() || r.is_remote() || r.is_tag() {
            return add_wt_with_ref(&repo, &name, wt_path, &r);
        }
    }
    add_wt_detached(&repo, &name, wt_path, &commit)
}

/// Remove a worktree — guarded like upstream's teardown: refuse while the
/// tree holds unintegrated work, then `git worktree remove --force`
/// semantics (prune metadata + rm -rf the directory).
pub fn remove_worktree(wt_path: &Path) -> Result<Value, String> {
    match worktree_has_unintegrated_work(wt_path) {
        Some(false) => {}
        Some(true) => return Err("worktree holds unintegrated work — refusing removal".into()),
        None => return Err("not a git worktree".into()),
    }
    let wt_repo = git2::Repository::discover(wt_path).map_err(|e| map_err(&e))?;
    // repo.path() of a linked worktree is <main>/.git/worktrees/<name>.
    let is_linked = wt_repo
        .path()
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n == "worktrees")
        .unwrap_or(false);
    if is_linked {
        if let Some(name) = wt_repo
            .path()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
        {
            if let Ok(main) = git2::Repository::open(wt_repo.commondir()) {
                if let Ok(wt) = main.find_worktree(&name) {
                    let mut po = git2::WorktreePruneOptions::new();
                    po.valid(true).locked(true).working_tree(true);
                    let _ = wt.prune(Some(&mut po));
                }
            }
        }
    } else {
        return Err("refusing to remove the main working tree".into());
    }
    std::fs::remove_dir_all(wt_path)
        .map_err(|e| format!("git metadata pruned but tree removal failed: {e}"))?;
    Ok(json!({ "ok": true }))
}

/// GC-safety verdict for a preserved worker worktree — worktreeIsGcSafe():
/// clean tree AND nothing unpushed (ahead == 0 vs upstream). Fails SAFE:
/// a dirty tree, unpushed commits, no upstream to prove against, or any
/// failed query all say "keep it".
/// Upstream worktree GC gate (hive.ts cleanupOrphanedWorktrees) — kept for the
/// scheduled-GC workstream; not wired to a command yet.
#[allow(dead_code)]
pub fn worktree_is_gc_safe(cwd: &Path) -> Option<Value> {
    let s = status(cwd)?;
    if s["clean"].as_bool() != Some(true) {
        return Some(json!({ "gc": false, "detail": "working tree dirty" }));
    }
    let ab = ahead_behind(cwd)?;
    if ab["hasUpstream"].as_bool() == Some(true) && ab["ahead"].as_u64() == Some(0) {
        return Some(json!({ "gc": true, "detail": "clean + 0 commits ahead of upstream" }));
    }
    Some(json!({
        "gc": false,
        "detail": "clean but commits not provably pushed (or no upstream configured)",
    }))
}

/// agentBranchFor (git.ts:254) — derive a safe `agent/<id>` branch name from
/// a worktree path's basename.
pub fn agent_branch_for(wt_path: &Path) -> String {
    let base = wt_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "agent".into());
    let slug: String = base
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    format!("agent/{}", if slug.is_empty() { "agent" } else { slug })
}

/// worktreeHasUnintegratedWork with an explicit BASE ref (upstream compares
/// `baseBranch..HEAD`, not the upstream tracking branch — a worker's
/// `agent/<id>` branch usually has none). keep = dirty OR ahead>0 OR any
/// unknown (fails safe: uncertainty keeps the worktree).
pub fn worktree_unintegrated_details(
    wt_path: &Path,
    base_branch: &str,
) -> Option<(bool, String, String)> {
    let repo = git2::Repository::discover(wt_path).ok()?;
    let branch = repo
        .head()
        .ok()
        .and_then(|h| {
            if h.is_branch() {
                h.shorthand().ok().map(str::to_string)
            } else {
                None
            }
        })
        .unwrap_or_else(|| "(detached)".into());
    let s = status(wt_path);
    let dirty = s
        .and_then(|v| v["clean"].as_bool().map(|c| !c))
        .unwrap_or(true); // unknown → assume dirty
    let ahead: Option<u64> = (|| {
        let base = repo
            .revparse_single(base_branch)
            .ok()?
            .peel_to_commit()
            .ok()?;
        let head = repo.head().ok()?.peel_to_commit().ok()?;
        let (ahead, _behind) = repo.graph_ahead_behind(head.id(), base.id()).ok()?;
        Some(ahead as u64)
    })();
    let keep = dirty || ahead.map(|a| a > 0).unwrap_or(true);
    let detail = format!(
        "dirty={dirty}, commitsAheadOf({base_branch})={}",
        ahead
            .map(|a| a.to_string())
            .unwrap_or_else(|| "unknown".into())
    );
    Some((keep, detail, branch))
}

/// worktreeIsGcSafe against an explicit base (the GC sweep's proof the
/// worker's content already landed): clean tree AND (HEAD reachable from
/// base OR `diff base HEAD` empty — squash/merge parity).
pub fn worktree_integrated_against(wt_path: &Path, base_branch: &str) -> Option<bool> {
    let s = status(wt_path)?;
    if s["clean"].as_bool() != Some(true) {
        return Some(false);
    }
    let repo = git2::Repository::discover(wt_path).ok()?;
    let base = repo
        .revparse_single(base_branch)
        .ok()?
        .peel_to_commit()
        .ok()?;
    let head = repo.head().ok()?.peel_to_commit().ok()?;
    if let Ok((ahead, _)) = repo.graph_ahead_behind(head.id(), base.id()) {
        if ahead == 0 {
            return Some(true);
        }
    }
    // Squash-proof: content identical even if history differs.
    let diff = repo
        .diff_tree_to_tree(Some(&base.tree().ok()?), Some(&head.tree().ok()?), None)
        .ok()?;
    Some(diff.deltas().len() == 0)
}

/// linkWorktreeDeps — junction (win32) / symlink the base checkout's
/// node_modules into a fresh worktree so installs aren't duplicated.
pub fn link_worktree_deps(base_dir: &Path, worktree_dir: &Path) -> Result<(), String> {
    let base_nm = base_dir.join("node_modules");
    let wt_nm = worktree_dir.join("node_modules");
    if !base_nm.is_dir() || wt_nm.exists() {
        return Ok(());
    }
    #[cfg(windows)]
    {
        // Direct junction syscall (no `cmd /c mklink`): a path with `&`/`%`
        // in a folder name must never reach cmd.exe's parser.
        junction::create(&base_nm, &wt_nm).map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(&base_nm, &wt_nm).map_err(|e| e.to_string())
    }
}

/// unlinkWorktreeDeps — remove the link ONLY (never the real dir): verify
/// the entry is a symlink/junction pointing back at the base node_modules.
pub fn unlink_worktree_deps(base_dir: &Path, worktree_dir: &Path) -> Result<(), String> {
    let base_nm = base_dir.join("node_modules");
    let wt_nm = worktree_dir.join("node_modules");
    let Ok(meta) = std::fs::symlink_metadata(&wt_nm) else {
        return Ok(()); // absent or not a link → nothing to do
    };
    if !meta.file_type().is_symlink() {
        return Ok(());
    }
    let target = std::fs::read_link(&wt_nm).map_err(|e| e.to_string())?;
    let resolved = worktree_dir.join(&target);
    let a = std::fs::canonicalize(&base_nm).ok();
    let b = std::fs::canonicalize(&resolved).ok();
    if a.is_some() && a == b {
        std::fs::remove_dir(&wt_nm).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Topological history page for the commit-graph renderer — getLogGraph():
/// `--all --topo-order`, pageable via n + skip, each commit carrying its
/// parent edges for DAG layout.
pub fn log_graph(cwd: &Path, n: usize, skip: usize) -> Option<Vec<Value>> {
    let repo = git2::Repository::discover(cwd).ok()?;
    let mut walk = repo.revwalk().ok()?;
    walk.set_sorting(git2::Sort::TOPOLOGICAL).ok()?;
    for r in repo.references().ok()?.flatten() {
        if let Some(oid) = r.target() {
            let _ = walk.push(oid);
        }
    }
    let _ = walk.push_head();
    let mut out = Vec::new();
    for oid in walk.skip(skip).take(n).flatten() {
        if let Ok(c) = repo.find_commit(oid) {
            out.push(json!({
                "sha": oid.to_string(),
                "subject": c.summary().ok().flatten().unwrap_or(""),
                "author": c.author().name().unwrap_or(""),
                "ts": c.time().seconds() * 1000,
                "parents": c.parent_ids().map(|p| p.to_string()).collect::<Vec<_>>(),
            }));
        }
    }
    Some(out)
}

fn delta_files(d: &git2::Diff, with_stats: bool) -> Vec<Value> {
    let mut files = Vec::new();
    for i in 0..d.deltas().len() {
        let Some(delta) = d.get_delta(i) else {
            continue;
        };
        let status = match delta.status() {
            git2::Delta::Added => "A",
            git2::Delta::Deleted => "D",
            git2::Delta::Modified => "M",
            git2::Delta::Renamed => "R",
            git2::Delta::Copied => "C",
            git2::Delta::Typechange => "T",
            _ => continue, // unmodified/untracked/conflicted — not a changed file
        };
        let path = delta
            .new_file()
            .path()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let old_path = delta
            .old_file()
            .path()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .filter(|p| p != &path);
        let (mut adds, mut dels) = (0u64, 0u64);
        if with_stats {
            if let Ok(Some(patch)) = git2::Patch::from_diff(d, i) {
                if let Ok((_ctx, a, del)) = patch.line_stats() {
                    adds = a as u64;
                    dels = del as u64;
                }
            }
        }
        let mut f = json!({ "path": path, "status": status });
        if let Some(op) = old_path {
            f["oldPath"] = json!(op);
        }
        if with_stats {
            f["additions"] = json!(adds);
            f["deletions"] = json!(dels);
        }
        files.push(f);
    }
    files
}

/// Files touched by one commit — getCommitFiles(): diff-tree -r --root -M
/// --name-status, plus per-file additions/deletions for the pane.
pub fn commit_files(cwd: &Path, sha: &str) -> Result<Vec<Value>, String> {
    if !is_safe_rev(sha) {
        return Err("invalid revision".into());
    }
    let repo = git2::Repository::discover(cwd).map_err(|e| map_err(&e))?;
    let commit = repo
        .revparse_single(sha)
        .and_then(|o| o.peel_to_commit())
        .map_err(|e| map_err(&e))?;
    let tree = commit.tree().map_err(|e| map_err(&e))?;
    let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
    let mut d = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)
        .map_err(|e| map_err(&e))?;
    d.find_similar(None).map_err(|e| map_err(&e))?;
    Ok(delta_files(&d, true))
}

/// Compare two refs — compareRefs() three-dot semantics: ahead = commits in
/// `head` not `base`, behind = the reverse, files = merge-base…head
/// name-status (falls back to the literal base…head diff when unrelated).
pub fn compare_refs(cwd: &Path, base: &str, head: &str) -> Result<Value, String> {
    if !is_safe_rev(base) || !is_safe_rev(head) {
        return Err("invalid revision".into());
    }
    let repo = git2::Repository::discover(cwd).map_err(|e| map_err(&e))?;
    let oid_of = |rev: &str| -> Result<git2::Oid, String> {
        repo.revparse_single(rev)
            .and_then(|o| o.peel_to_commit())
            .map(|c| c.id())
            .map_err(|e| map_err(&e))
    };
    let (b_oid, h_oid) = (oid_of(base)?, oid_of(head)?);
    let (ahead, behind) = repo
        .graph_ahead_behind(h_oid, b_oid)
        .map_err(|e| map_err(&e))?;
    let mb = repo.merge_base(b_oid, h_oid).ok();
    let tree_of = |oid: git2::Oid| repo.find_commit(oid).and_then(|c| c.tree());
    let from = mb
        .and_then(|o| tree_of(o).ok())
        .or_else(|| tree_of(b_oid).ok());
    let to = tree_of(h_oid).ok();
    let mut d = repo
        .diff_tree_to_tree(from.as_ref(), to.as_ref(), None)
        .map_err(|e| map_err(&e))?;
    d.find_similar(None).map_err(|e| map_err(&e))?;
    Ok(json!({
        "ahead": ahead,
        "behind": behind,
        "mergeBase": mb.map(|o| o.to_string()),
        "files": delta_files(&d, true),
    }))
}

/// File content pinned at a rev — getFileAtRev(): `{rev}:{path}` resolved
/// through libgit2 with the same path confinement + 2 MB / binary rules.
/// Absent blob (or unresolvable rev) → exists:false, matching upstream
/// cat-file failure semantics.
pub fn show_file(cwd: &Path, rev: &str, path: &str) -> Result<Value, String> {
    if !is_safe_rev(rev) {
        return Err("invalid revision".into());
    }
    let rel = safe_rel_path(path).ok_or_else(|| "path escapes repository root".to_string())?;
    let repo = git2::Repository::discover(cwd).map_err(|e| map_err(&e))?;
    let spec = format!("{rev}:{rel}");
    let Ok(obj) = repo.revparse_single(&spec) else {
        return Ok(json!({ "ok": true, "exists": false, "isBinary": false, "content": "" }));
    };
    let Ok(blob) = obj.peel_to_blob() else {
        return Ok(json!({ "ok": true, "exists": false, "isBinary": false, "content": "" }));
    };
    if blob.size() > MAX_GIT_TEXT_BYTES {
        return Err("file too large to diff (>2 MB)".into());
    }
    let content = blob.content();
    if blob.is_binary() || content.contains(&0) {
        return Ok(json!({ "ok": true, "exists": true, "isBinary": true, "content": "" }));
    }
    Ok(json!({
        "ok": true,
        "exists": true,
        "isBinary": false,
        "content": String::from_utf8_lossy(content),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oa-git-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn init_repo(dir: &Path) -> git2::Repository {
        let repo = git2::Repository::init(dir).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "t").unwrap();
        cfg.set_str("user.email", "t@t").unwrap();
        repo
    }

    fn commit_file(
        repo: &git2::Repository,
        dir: &Path,
        name: &str,
        content: &str,
        msg: &str,
    ) -> git2::Oid {
        std::fs::write(dir.join(name), content).unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(Path::new(name)).unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = repo.signature().unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents)
            .unwrap()
    }

    #[test]
    fn safe_rev_charset() {
        for ok in [
            "main",
            "feature/x.y_1-2",
            "HEAD",
            "HEAD~2",
            "HEAD^",
            "v1.0",
            "a@{1}",
            &"a".repeat(40),
        ] {
            assert!(is_safe_rev(ok), "{ok}");
        }
        for bad in [
            "",
            "-x",
            "--force",
            "a..b",
            "a b",
            "/a",
            "a/",
            "a;b",
            "a:b",
            "a\\b",
            "a!b",
            &"x".repeat(257),
        ] {
            assert!(!is_safe_rev(bad), "{bad}");
        }
    }

    #[test]
    fn status_counts() {
        let d = tmpdir("status");
        let repo = init_repo(&d);
        commit_file(&repo, &d, "a.txt", "one", "init");
        let s = status(&d).unwrap();
        assert_eq!(s["clean"].as_bool(), Some(true));
        std::fs::write(d.join("new.txt"), "x").unwrap();
        std::fs::write(d.join("a.txt"), "changed").unwrap();
        let s = status(&d).unwrap();
        assert_eq!(s["untracked"].as_u64(), Some(1));
        assert_eq!(s["unstaged"].as_u64(), Some(1));
        assert_eq!(s["clean"].as_bool(), Some(false));
        drop(repo);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn detached_unreferenced_head_counts_as_unintegrated() {
        let d = tmpdir("detached");
        let repo = init_repo(&d);
        let first = commit_file(&repo, &d, "a.txt", "one", "init");
        // Branch HEAD, clean, no upstream → safe to remove.
        assert_eq!(worktree_has_unintegrated_work(&d), Some(false));
        // Detached at a commit a branch contains → still safe.
        repo.set_head_detached(first).unwrap();
        assert_eq!(worktree_has_unintegrated_work(&d), Some(false));
        // New commit on the detached head: no ref holds it → must be kept.
        commit_file(&repo, &d, "b.txt", "two", "orphan");
        assert_eq!(worktree_has_unintegrated_work(&d), Some(true));
        drop(repo);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn agent_branch_slug() {
        assert_eq!(
            agent_branch_for(std::path::Path::new("/x/worktrees/worker-abc123")),
            "agent/worker-abc123"
        );
        // unsafe basename chars → '-' (injection-proof like upstream slugify)
        assert_eq!(
            agent_branch_for(std::path::Path::new("/x/wt/../evil name")),
            "agent/evil-name"
        );
        assert_eq!(
            agent_branch_for(std::path::Path::new("/x/---")),
            "agent/agent"
        );
    }

    #[test]
    fn unintegrated_details_repo() {
        let dir = std::env::temp_dir().join(format!("ao-git-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let repo = git2::Repository::init(&dir).unwrap();
        // Seed commit so rev-list/dirty checks have something to compare.
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let mut idx = repo.index().unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        idx.add_path(std::path::Path::new("a.txt")).unwrap();
        idx.write().unwrap();
        let oid = repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            "init",
            &repo.find_tree(idx.write_tree().unwrap()).unwrap(),
            &[],
        );
        assert!(oid.is_ok());
        let (keep, detail, _branch) =
            worktree_unintegrated_details(&dir, "HEAD").expect("repo must answer");
        assert!(!keep, "clean repo at HEAD must not be kept ({detail})");
        // Dirty file → keep.
        std::fs::write(dir.join("a.txt"), "changed").unwrap();
        let (keep, _, _) = worktree_unintegrated_details(&dir, "HEAD").unwrap();
        assert!(keep);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ahead_behind_no_upstream() {
        let d = tmpdir("ab");
        let repo = init_repo(&d);
        commit_file(&repo, &d, "a.txt", "one", "init");
        let ab = ahead_behind(&d).unwrap();
        assert_eq!(ab["hasUpstream"].as_bool(), Some(false));
        assert_eq!(ab["ahead"].as_u64(), Some(0));
        assert_eq!(ab["behind"].as_u64(), Some(0));
        drop(repo);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn log_graph_edges() {
        let d = tmpdir("graph");
        let repo = init_repo(&d);
        let c1 = commit_file(&repo, &d, "a.txt", "one", "init");
        let c2 = commit_file(&repo, &d, "b.txt", "two", "second");
        let page = log_graph(&d, 10, 0).unwrap();
        assert_eq!(page.len(), 2);
        // topological: child before parent
        assert_eq!(page[0]["sha"].as_str(), Some(c2.to_string().as_str()));
        assert_eq!(
            page[0]["parents"][0].as_str(),
            Some(c1.to_string().as_str())
        );
        assert_eq!(page[1]["sha"].as_str(), Some(c1.to_string().as_str()));
        assert!(page[1]["parents"].as_array().unwrap().is_empty());
        // paging
        assert_eq!(log_graph(&d, 10, 1).unwrap().len(), 1);
        assert_eq!(log_graph(&d, 1, 0).unwrap().len(), 1);
        drop(repo);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn commit_files_and_show_file() {
        let d = tmpdir("cf");
        let repo = init_repo(&d);
        let c1 = commit_file(&repo, &d, "a.txt", "one\n", "init");
        let files = commit_files(&d, &c1.to_string()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["status"].as_str(), Some("A"));
        assert_eq!(files[0]["path"].as_str(), Some("a.txt"));
        assert_eq!(files[0]["additions"].as_u64(), Some(1));
        let c2 = commit_file(&repo, &d, "a.txt", "one\ntwo\n", "grow");
        let files = commit_files(&d, &c2.to_string()).unwrap();
        assert_eq!(files[0]["status"].as_str(), Some("M"));
        assert_eq!(files[0]["additions"].as_u64(), Some(1));
        let f = show_file(&d, &c1.to_string(), "a.txt").unwrap();
        assert_eq!(f["exists"].as_bool(), Some(true));
        assert_eq!(f["content"].as_str(), Some("one\n"));
        assert_eq!(
            show_file(&d, &c1.to_string(), "missing.rs").unwrap()["exists"].as_bool(),
            Some(false)
        );
        assert!(show_file(&d, &c1.to_string(), "../escape").is_err());
        drop(repo);
        std::fs::remove_dir_all(&d).ok();
    }
}
