//! Tauri commands — the `github:*` IPC surface (upstream `github:ciRuns` /
//! `github:issues` took a `cwd` and shelled out to `gh`; the port takes an
//! `owner/repo` string, falling back to the persisted default `github_repo`).

use crate::lock_ext::LockExt;
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::commands::state;

/// Resolve the repo arg against the persisted default.
fn resolve_repo(app: &AppHandle, repo: Option<String>) -> Result<String, String> {
    if let Some(r) = repo.as_deref().and_then(crate::github::valid_repo) {
        return Ok(r);
    }
    // The Command Center sends a local project FOLDER, not `owner/repo`:
    // derive it from that folder's git remote.
    if let Some(folder) = repo.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
        let p = std::path::PathBuf::from(crate::misc::expand_tilde(folder));
        if p.is_dir() {
            return crate::github::repo_from_folder(&p)
                .ok_or_else(|| format!("no GitHub remote found in {}", p.display()));
        }
    }
    let stored = state(&app)
        .persist
        .lock_safe()
        .get_kv(crate::github::KV_REPO)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .and_then(|s| crate::github::valid_repo(&s));
    stored.ok_or_else(|| "no repo — pass owner/repo or set one with github_set_repo".to_string())
}

/// github:ciRuns — recent workflow runs for the repo.
#[tauri::command(async)]
pub fn github_ci_runs(app: AppHandle, repo: Option<String>) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("github_ci_runs", || {
    match resolve_repo(&app, repo).and_then(|r| crate::github::list_ci_runs(&r)) {
        Ok(runs) => Ok(json!({ "ok": true, "runs": runs })),
        Err(e) => Ok(json!({ "ok": false, "error": e })),
    }
    })
}

/// github:issues — up to 30 open issues for the repo.
#[tauri::command(async)]
pub fn github_issues(app: AppHandle, repo: Option<String>) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("github_issues", || {
    match resolve_repo(&app, repo).and_then(|r| crate::github::list_issues(&r)) {
        Ok(issues) => Ok(json!({ "ok": true, "issues": issues })),
        Err(e) => Ok(json!({ "ok": false, "error": e })),
    }
    })
}

/// Persist the default `owner/repo` (persist kv `github_repo`). An empty
/// string clears it.
#[tauri::command(async)]
pub fn github_set_repo(app: AppHandle, owner_repo: String) -> Result<Value, String> {
    crate::panic_guard::cmd_guard("github_set_repo", || {
    let st = state(&app);
    let trimmed = owner_repo.trim();
    if trimmed.is_empty() {
        st.persist
            .lock_safe()
            .set_kv(crate::github::KV_REPO, &json!(null));
        return Ok(json!({ "ok": true, "repo": null }));
    }
    let Some(valid) = crate::github::valid_repo(trimmed) else {
        return Err(format!("bad repo {owner_repo:?} (want owner/repo)"));
    };
    st.persist
        .lock_safe()
        .set_kv(crate::github::KV_REPO, &json!(valid));
    Ok(json!({ "ok": true, "repo": valid }))
    })
}
