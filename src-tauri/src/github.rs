//! GitHub REST adapter — port of upstream's `github:ciRuns` / `github:issues`
//! ipcMain handlers (which shelled out to the `gh` CLI). The Tauri port calls
//! the REST API directly via ureq: no `gh` install required, and a stored
//! `GITHUB_TOKEN` (keyring service `agent-office`) upgrades the calls to
//! authenticated. With no token the calls still run — public repos answer
//! fine, just under the tighter anonymous rate limit (403-limited).
//!
//! The repo is `owner/name`: passed per call or defaulted from the persist kv
//! `github_repo` (set via `github_set_repo`).

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

const API_BASE: &str = "https://api.github.com";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const KEYRING_SERVICE: &str = "agent-office";
const TOKEN_KEY: &str = "GITHUB_TOKEN";
/// persist kv key for the default `owner/repo`.
pub const KV_REPO: &str = "github_repo";

/// A GitHub issue, normalized for the renderer (labels/assignees flattened to
/// names) — same shape as upstream's `GHIssue`.
#[derive(Debug, Clone, Serialize)]
pub struct GhIssue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub url: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
}

/// A CI (GitHub Actions) workflow run — the REST field set the spec fixes
/// (superset of upstream's `CIRun`, which only kept name/status/conclusion/url).
#[derive(Debug, Clone, Serialize)]
pub struct CiRun {
    pub id: u64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub html_url: String,
    pub head_branch: String,
    pub created_at: String,
}

/// The token, when one is stored. Never logged; only ever an Authorization header.
pub fn token() -> Option<String> {
    keyring::Entry::new(KEYRING_SERVICE, TOKEN_KEY)
        .ok()
        .and_then(|e| e.get_password().ok())
        .filter(|t| !t.is_empty())
}

/// "owner/repo" validation — both halves `[A-Za-z0-9._-]+` (GitHub's own
/// alphabet), no extra slashes, no `.`/`..` segments: the value is spliced
/// straight into an API path (`/repos/{repo}/…`), so `../` or `?`/`#` would
/// retarget the request.
pub fn valid_repo(repo: &str) -> Option<String> {
    let r = repo.trim().trim_matches('/');
    let mut parts = r.split('/');
    let (o, n) = match (parts.next(), parts.next(), parts.next()) {
        (Some(o), Some(n), None) => (o, n),
        _ => return None,
    };
    let n = n.strip_suffix(".git").unwrap_or(n);
    let ok_half = |h: &str| {
        !h.is_empty()
            && h.len() <= 100
            && h != "."
            && !h.contains("..")
            && h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    (ok_half(o) && ok_half(n)).then(|| format!("{o}/{n}"))
}

/// `owner/repo` out of a git remote URL: https/http (optionally with
/// credentials), `git@github.com:owner/repo(.git)` and `ssh://git@github.com/…`.
/// Only github.com — the API base this adapter speaks. None for anything else.
pub fn parse_github_remote(url: &str) -> Option<String> {
    let u = url.trim();
    let rest = if let Some(r) = u.strip_prefix("git@github.com:") {
        r.to_string()
    } else {
        let after = ["https://", "http://", "ssh://", "git://"]
            .iter()
            .find_map(|p| u.strip_prefix(p))?;
        // drop `user[:pass]@`, then require the host to be exactly github.com
        let after = after.rsplit_once('@').map_or(after, |(_, h)| h);
        let (host, path) = after.split_once('/')?;
        if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com") {
            return None;
        }
        path.to_string()
    };
    valid_repo(rest.trim_end_matches('/'))
}

/// The `owner/repo` a local FOLDER's git remote points at: `origin` first,
/// else the first remote that parses as a GitHub URL. The renderer hands the
/// Command Center's issues/CI panels a project folder, not `owner/repo`.
pub fn repo_from_folder(folder: &std::path::Path) -> Option<String> {
    let repo = git2::Repository::discover(folder).ok()?;
    let remotes = repo.remotes().ok()?;
    let mut names: Vec<String> = remotes.iter().flatten().flatten().map(String::from).collect();
    names.sort_by_key(|n| n != "origin"); // stable: origin first
    names.iter().find_map(|n| {
        let r = repo.find_remote(n).ok()?;
        r.url().ok().and_then(parse_github_remote)
    })
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .new_agent()
}

/// GET a JSON body from the GitHub REST API. User-Agent is required by the API
/// (requests without one are rejected); the token is attached when present.
fn gh_get(path_and_query: &str) -> Result<Value, String> {
    let url = format!("{API_BASE}{path_and_query}");
    let mut req = agent()
        .get(url.as_str())
        .header(
            "user-agent",
            concat!("agent-office/", env!("CARGO_PKG_VERSION")),
        )
        .header("accept", "application/vnd.github+json");
    if let Some(t) = token() {
        req = req.header("authorization", &format!("Bearer {t}"));
    }
    let mut resp = req
        .call()
        .map_err(|e| format!("github request failed: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().unwrap_or_default();
    if !(200..300).contains(&status) {
        let msg = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|j| {
                j.get("message")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| text.chars().take(200).collect());
        return Err(format!("GitHub {status}: {msg}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("bad response from GitHub: {e}"))
}

/// GET /repos/{owner}/{repo}/actions/runs?per_page=10 — the 10 most recent
/// workflow runs, normalized for the renderer.
pub fn list_ci_runs(owner_repo: &str) -> Result<Vec<CiRun>, String> {
    let repo = valid_repo(owner_repo)
        .ok_or_else(|| format!("bad repo {owner_repo:?} (want owner/repo)"))?;
    let v = gh_get(&format!("/repos/{repo}/actions/runs?per_page=10"))?;
    let runs = v
        .get("workflow_runs")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(runs
        .iter()
        .map(|r| CiRun {
            id: r.get("id").and_then(|x| x.as_u64()).unwrap_or(0),
            name: r
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            status: r
                .get("status")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            conclusion: r
                .get("conclusion")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string()),
            html_url: r
                .get("html_url")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            head_branch: r
                .get("head_branch")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            created_at: r
                .get("created_at")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        })
        .collect())
}

/// GET /repos/{owner}/{repo}/issues?state=open&per_page=30 — open issues.
/// NOTE: the REST issues endpoint also lists PRs; they are filtered out so the
/// result matches `gh issue list` semantics.
pub fn list_issues(owner_repo: &str) -> Result<Vec<GhIssue>, String> {
    let repo = valid_repo(owner_repo)
        .ok_or_else(|| format!("bad repo {owner_repo:?} (want owner/repo)"))?;
    let v = gh_get(&format!("/repos/{repo}/issues?state=open&per_page=30"))?;
    let raw = v.as_array().cloned().unwrap_or_default();
    Ok(raw
        .iter()
        .filter(|i| i.get("pull_request").is_none())
        .map(|i| GhIssue {
            number: i.get("number").and_then(|x| x.as_u64()).unwrap_or(0),
            title: i
                .get("title")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            body: i
                .get("body")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            url: i
                .get("html_url")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            labels: i
                .get("labels")
                .and_then(|l| l.as_array())
                .map(|l| {
                    l.iter()
                        .filter_map(|x| x.get("name").and_then(|n| n.as_str()))
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default(),
            assignees: i
                .get("assignees")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.get("login").and_then(|n| n.as_str()))
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_repo_charset_and_shape() {
        assert_eq!(valid_repo("octo/cat").as_deref(), Some("octo/cat"));
        assert_eq!(valid_repo(" /octo/cat.git/ ").as_deref(), Some("octo/cat"));
        assert_eq!(valid_repo("my-org/.github").as_deref(), Some("my-org/.github"));
        assert_eq!(valid_repo("a_b/c.d-e").as_deref(), Some("a_b/c.d-e"));
        for bad in [
            "", "octo", "octo/", "/cat", "a/b/c", "../cat", "octo/..", "./x", "octo/.", "a..b/c",
            "octo/cat?x=1", "octo/cat#f", "octo/ca t", "octo/c%2e", "octo\\cat",
            "oc to/cat", "octo/\u{e9}", &format!("o/{}", "a".repeat(101)),
        ] {
            assert!(valid_repo(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn github_remote_urls_parse_to_owner_repo() {
        for (url, want) in [
            ("https://github.com/octo/cat.git", Some("octo/cat")),
            ("https://github.com/octo/cat", Some("octo/cat")),
            ("https://github.com/octo/cat/", Some("octo/cat")),
            ("https://user:tok@github.com/octo/cat.git", Some("octo/cat")),
            ("git@github.com:octo/cat.git", Some("octo/cat")),
            ("ssh://git@github.com/octo/cat.git", Some("octo/cat")),
            ("https://gitlab.com/octo/cat.git", None),
            ("https://github.com.evil.io/octo/cat.git", None),
            ("https://evil.io/github.com/octo/cat.git", None),
            ("git@gitlab.com:octo/cat.git", None),
            ("https://github.com/octo", None),
            ("C:/work/repo", None),
            ("", None),
        ] {
            assert_eq!(parse_github_remote(url).as_deref(), want, "{url}");
        }
    }

    #[test]
    fn repo_from_folder_reads_origin_then_other_remotes() {
        let d = std::env::temp_dir().join(format!("oa-ghrepo-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(d.join("sub")).unwrap();
        let repo = git2::Repository::init(&d).unwrap();
        assert_eq!(repo_from_folder(&d), None, "no remotes");
        repo.remote("upstream", "https://github.com/up/stream.git").unwrap();
        repo.remote("mirror", "https://gitlab.com/x/y.git").unwrap();
        assert_eq!(repo_from_folder(&d).as_deref(), Some("up/stream"));
        repo.remote("origin", "git@github.com:me/mine.git").unwrap();
        // origin wins, and a subfolder resolves through discover()
        assert_eq!(repo_from_folder(&d.join("sub")).as_deref(), Some("me/mine"));
        assert_eq!(repo_from_folder(&d.join("missing")), None);
        drop(repo);
        let _ = std::fs::remove_dir_all(&d);
    }
}
