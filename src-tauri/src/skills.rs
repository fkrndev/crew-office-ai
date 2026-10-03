//! SKILLS — port of `skills.ts`. Two halves:
//!
//!  1. LOCAL — skills already installed, discovered by walking the dirs each
//!     CLI reads. Claude Code's format is well-specified (a folder holding
//!     SKILL.md with YAML frontmatter name/description); OpenCode and Codex
//!     use plugin/config dirs, so they are reported as plugins rather than
//!     pretending they share a format.
//!
//!  2. CATALOG — abubakarsiddik31/claude-skills-collection, markdown tables
//!     parsed + cached on disk (24h TTL; stale-beats-empty on network fail).
//!
//!  3. INSTALL/UNINSTALL — written as if the source is hostile (it is: a
//!     skill is arbitrary instructions that run inside an agent holding the
//!     user's tools). Name sanitised, post-resolution containment re-checked,
//!     file count/bytes/depth capped, only regular files written, write only
//!     after the whole tree resolved.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub provider: String, // 'claude' | 'opencode' | 'codex'
    pub scope: String,    // 'user' | 'project' | 'bundled'
    pub path: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CatalogSkill {
    pub name: String,
    pub description: String,
    pub url: String,
    pub category: String,
    pub owner: String,
}

/// Strip a leading YAML frontmatter block and pull name/description —
/// deliberately not a YAML parser (description is routinely a multi-line `|`
/// block, which a naive key:value split would truncate at the first line).
pub fn parse_skill_frontmatter(md: &str) -> (Option<String>, Option<String>) {
    let body = md
        .strip_prefix("---\n")
        .or_else(|| md.strip_prefix("---\r\n"))
        .and_then(|rest| {
            rest.find("\n---\n")
                .or_else(|| rest.find("\r\n---"))
                .map(|i| &rest[..i])
        });
    let Some(body) = body else { return (None, None) };
    let mut name = None;
    let mut description = None;
    let mut lines = body.lines().peekable();
    while let Some(l) = lines.next() {
        if let Some(v) = l.strip_prefix("name:") {
            name = Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
        } else if l.starts_with("description:") {
            let v = l["description:".len()..].trim();
            if v.starts_with('|') || v.starts_with('>') {
                // Block scalar → the indented lines that follow, joined.
                let mut parts = Vec::new();
                while let Some(nl) = lines.peek() {
                    if nl.starts_with(' ') || nl.starts_with('\t') {
                        parts.push(lines.next().unwrap().trim().to_string());
                    } else {
                        break;
                    }
                }
                description = Some(parts.join(" "));
            } else {
                description = Some(v.trim_matches('"').trim_matches('\'').to_string());
            }
        }
    }
    (name, description)
}

/// Every folder under `dir` holding a SKILL.md.
fn scan_skill_dir(dir: &Path, provider: &str, scope: &str, out: &mut Vec<LocalSkill>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let skill_dir = e.path();
        let md = skill_dir.join("SKILL.md");
        if !skill_dir.is_dir() || !md.exists() {
            continue;
        }
        let entry = e.file_name().to_string_lossy().to_string();
        let (name, desc) = fs::read_to_string(&md)
            .map(|t| parse_skill_frontmatter(&t))
            .unwrap_or((None, None));
        out.push(LocalSkill {
            id: format!("{scope}:{entry}"),
            name: name.unwrap_or(entry),
            description: desc.unwrap_or_default(),
            provider: provider.into(),
            scope: scope.into(),
            path: skill_dir.display().to_string(),
        });
    }
}

/// Plugin dirs for CLIs that don't use the SKILL.md format — reported so the
/// tab tells the truth about what a provider has.
fn scan_plugin_dir(dir: &Path, provider: &str, scope: &str, out: &mut Vec<LocalSkill>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let entry = e.file_name().to_string_lossy().to_string();
        if entry.starts_with('.') {
            continue;
        }
        let parent = dir
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let dir_name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        out.push(LocalSkill {
            id: format!("{scope}:{provider}:{entry}"),
            name: entry.trim_end_matches(".js").trim_end_matches(".mjs").trim_end_matches(".cjs").into(),
            description: format!("Plugin in {parent}/{dir_name}"),
            provider: provider.into(),
            scope: scope.into(),
            path: e.path().display().to_string(),
        });
    }
}

/// Everything installed, deduped by (provider,name) with the most specific
/// scope winning — the same precedence the CLIs apply.
pub fn list_local_skills(cwds: &[String], bundled_dir: Option<&Path>) -> Vec<LocalSkill> {
    let home = dirs::home_dir().unwrap_or_default();
    let mut found = Vec::new();
    if let Some(b) = bundled_dir {
        scan_skill_dir(b, "claude", "bundled", &mut found);
    }
    scan_skill_dir(&home.join(".claude").join("skills"), "claude", "user", &mut found);
    scan_plugin_dir(&home.join(".config").join("opencode").join("plugin"), "opencode", "user", &mut found);
    scan_plugin_dir(&home.join(".codex").join("plugins"), "codex", "user", &mut found);
    for cwd in cwds.iter().filter(|c| !c.is_empty()) {
        let c = Path::new(cwd);
        scan_skill_dir(&c.join(".claude").join("skills"), "claude", "project", &mut found);
        scan_plugin_dir(&c.join(".opencode").join("plugin"), "opencode", "project", &mut found);
    }
    let rank = |s: &str| match s {
        "project" => 3,
        "user" => 2,
        _ => 1,
    };
    let mut best: std::collections::HashMap<String, LocalSkill> = Default::default();
    for s in found {
        let key = format!("{}:{}", s.provider, s.name.to_lowercase());
        if best.get(&key).map(|p| rank(&p.scope)).unwrap_or(0) < rank(&s.scope) {
            best.insert(key, s);
        }
    }
    let mut out: Vec<_> = best.into_values().collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

const CATALOG_URL: &str =
    "https://raw.githubusercontent.com/abubakarsiddik31/claude-skills-collection/main/README.md";
const CATALOG_TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// Parse the catalog README: markdown TABLES under `## <emoji> Category`
/// headings. Rows need 3 cells AND a resolvable https link — header/rule/
/// overview rows drop rather than guess.
pub fn parse_catalog_markdown(md: &str) -> Vec<CatalogSkill> {
    let mut out = Vec::new();
    let mut category = "Skills".to_string();
    let mut seen = std::collections::HashSet::new();
    for raw in md.lines() {
        let line = raw.trim();
        if let Some(h) = line.strip_prefix("## ").or_else(|| line.strip_prefix("### ")) {
            let clean = h
                .split(']')
                .map(|seg| seg.split('[').next_back().unwrap_or(seg))
                .collect::<String>();
            let clean: String = clean
                .chars()
                .filter(|c| !matches!(c, '*' | '_' | '`'))
                .collect();
            let clean = clean.trim_start_matches(|c: char| !c.is_alphanumeric()).trim();
            if !clean.is_empty() {
                category = clean.to_string();
            }
            continue;
        }
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.split('|').skip(1).collect();
        let cells: Vec<&str> = cells[..cells.len().saturating_sub(1)].iter().map(|c| c.trim()).collect();
        if cells.len() < 3 {
            continue;
        }
        if cells[0].replace(':', "").trim().chars().all(|c| c == '-') && !cells[0].is_empty() {
            continue; // the |---| rule
        }
        let name = cells[0].replace(['*', '`'], "");
        let name = name.trim();
        let description = cells[1].replace(['*', '`'], "");
        let cell2 = cells[2];
        let url = cell2
            .find("https://")
            .map(|i| cell2[i..].trim_end_matches([')', ' ', '"', '\'', '.', ',']))
            .unwrap_or_default();
        if name.is_empty() || url.is_empty() {
            continue;
        }
        let key = format!("{name}|{url}");
        if !seen.insert(key) {
            continue;
        }
        let owner = url
            .strip_prefix("https://github.com/")
            .and_then(|r| r.split('/').next())
            .unwrap_or("other")
            .to_lowercase();
        out.push(CatalogSkill {
            name: name.to_string(),
            description: description.trim().to_string(),
            url: url.to_string(),
            category: category.clone(),
            owner,
        });
    }
    out
}

#[derive(Serialize)]
pub struct CatalogResult {
    pub skills: Vec<CatalogSkill>,
    pub fetched_at: u64,
    pub stale: bool,
    pub error: Option<String>,
}

/// The catalog — cache-fresh, else network, else stale-beats-empty.
pub fn load_catalog(cache_path: &Path, force: bool) -> CatalogResult {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let cached: Option<(Vec<CatalogSkill>, u64)> = fs::read_to_string(cache_path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| {
            Some((
                serde_json::from_value(v["skills"].clone()).ok()?,
                v["fetchedAt"].as_u64()?,
            ))
        });
    if let Some((skills, at)) = &cached {
        if now.saturating_sub(*at) < CATALOG_TTL_MS && !force {
            return CatalogResult { skills: skills.clone(), fetched_at: *at, stale: false, error: None };
        }
    }
    match ureq::get(CATALOG_URL).call() {
        Ok(mut resp) => {
            let md = resp.body_mut().read_to_string().unwrap_or_default();
            let skills = parse_catalog_markdown(&md);
            if skills.is_empty() {
                if let Some((skills, at)) = cached {
                    return CatalogResult {
                        skills, fetched_at: at, stale: true,
                        error: Some("catalog format changed".into()),
                    };
                }
                return CatalogResult { skills: vec![], fetched_at: 0, stale: true, error: Some("empty catalog".into()) };
            }
            let payload = serde_json::json!({ "skills": skills, "fetchedAt": now });
            if let Some(p) = cache_path.parent() {
                let _ = fs::create_dir_all(p);
            }
            if let Ok(s) = serde_json::to_string(&payload) {
                let _ = fs::write(cache_path, s);
            }
            CatalogResult { skills, fetched_at: now, stale: false, error: None }
        }
        Err(e) => {
            let err = e.to_string();
            if let Some((skills, at)) = cached {
                CatalogResult { skills, fetched_at: at, stale: true, error: Some(err) }
            } else {
                CatalogResult { skills: vec![], fetched_at: 0, stale: true, error: Some(err) }
            }
        }
    }
}

/* ── Install / uninstall ─────────────────────────────────────────────────── */

const MAX_FILES: usize = 60;
const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024;
const MAX_DEPTH: usize = 5;

#[derive(Deserialize)]
#[allow(dead_code)]
struct GhEntry {
    name: String,
    path: String,
    #[serde(rename = "type")]
    kind: String,
    size: Option<u64>,
    download_url: Option<String>,
}

/// A GitHub source URL → contents-API pieces. Two shapes: a folder inside a
/// repo (`/tree/<ref>/<path>`) and a whole repo whose root IS the skill
/// (ref empty = default branch — no guessing main vs master).
pub fn parse_github_source_url(url: &str) -> Option<(String, String, String, String)> {
    let clean = url.trim();
    let clean = clean.split(['#', '?']).next().unwrap_or("").trim_end_matches('/');
    let gh = clean.strip_prefix("https://github.com/")?;
    let parts: Vec<&str> = gh.split('/').collect();
    if parts.len() >= 4 && parts[2] == "tree" {
        return Some((
            parts[0].to_string(),
            parts[1].to_string(),
            parts[3].to_string(),
            parts[4..].join("/"),
        ));
    }
    if parts.len() == 2 {
        let owner = parts[0];
        if ["orgs", "topics", "collections", "sponsors", "features"]
            .contains(&owner.to_lowercase().as_str())
        {
            return None;
        }
        return Some((
            owner.to_string(),
            parts[1].trim_end_matches(".git").to_string(),
            String::new(),
            String::new(),
        ));
    }
    None
}

/// A folder name we are willing to create — separators, dot-dots and leading
/// dots are rejected outright rather than massaged into safety.
pub fn safe_skill_dir_name(raw: &str) -> Option<String> {
    let base = raw.trim().rsplit('/').next().unwrap_or("");
    if base.is_empty() || base == "." || base == ".." || base.contains("..") {
        return None;
    }
    let mut chars = base.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphanumeric());
    let rest_ok = base
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        && base.len() <= 64;
    if first_ok && rest_ok {
        Some(base.to_string())
    } else {
        None
    }
}

fn gh_get_json(url: &str) -> Result<serde_json::Value> {
    let mut resp = ureq::get(url).call()?;
    let v: serde_json::Value = resp.body_mut().read_json()?;
    Ok(v)
}

/// For a repo-ROOT source, locate the directory holding the skill BEFORE any
/// download (upstream issue #385 — the caps must bound the skill folder, not
/// the whole repo).
fn resolve_repo_root_skill_dir(
    owner: &str,
    repo: &str,
    entry_name: &str,
) -> std::result::Result<String, String> {
    let api = |p: &str| {
        format!(
            "https://api.github.com/repos/{owner}/{repo}/contents/{}",
            p
        )
    };
    let list = |p: &str| -> Vec<GhEntry> {
        gh_get_json(&api(p))
            .and_then(|v| {
                if v.is_array() {
                    serde_json::from_value(v).map_err(Into::into)
                } else {
                    serde_json::from_value(v).map(|e| vec![e]).map_err(Into::into)
                }
            })
            .unwrap_or_default()
    };
    let has_skill = |l: &[GhEntry]| l.iter().any(|it| it.kind == "file" && it.name == "SKILL.md");
    let root = list("");
    if has_skill(&root) {
        return Ok(String::new());
    }
    let has_dir = |n: &str| root.iter().any(|it| it.kind == "dir" && it.name == n);
    let mut candidates = Vec::new();
    if let Some(name) = safe_skill_dir_name(entry_name) {
        if has_dir("skills") {
            candidates.push(format!("skills/{name}"));
        }
        if has_dir(".claude") {
            candidates.push(format!(".claude/skills/{name}"));
        }
        if has_dir(&name) {
            candidates.push(name);
        }
    }
    for c in candidates {
        if has_skill(&list(&c)) {
            return Ok(c);
        }
    }
    Err("couldn't find the skill's SKILL.md in that repository — install it by hand".into())
}

/// officialskills.sh pages render a GitHub folder and link back — one request
/// turns those rows installable.
fn resolve_source_url(url: &str) -> Option<String> {
    if parse_github_source_url(url).is_some() {
        return Some(url.to_string());
    }
    if !url.to_lowercase().starts_with("https://officialskills.sh/")
        && !url.to_lowercase().starts_with("https://www.officialskills.sh/")
    {
        return None;
    }
    let mut resp = ureq::get(url).call().ok()?;
    let html = resp.body_mut().read_to_string().ok()?;
    html.find("https://github.com/").map(|i| {
        html[i..]
            .split(|c: char| matches!(c, '"' | '\'' | '<' | '>' | ')' | ' '))
            .next()
            .unwrap_or("")
            .trim_end_matches(['.', ',', ')'])
            .to_string()
    })
    .filter(|s| s.contains("/tree/") || parse_github_source_url(s).is_some())
}

/// Download one skill folder into the user's Claude skills directory.
/// Structured refusals, never throws — "not installable" and "install
/// failed" are different answers.
pub fn install_skill(entry_url: &str, entry_name: &str) -> std::result::Result<String, String> {
    let Some(source) = resolve_source_url(entry_url) else {
        return Err("No downloadable source — open Learn more to install it by hand.".into());
    };
    let Some((owner, repo, ref_, mut skill_path)) = parse_github_source_url(&source) else {
        return Err("Source is not a GitHub folder.".into());
    };
    let api = |p: &str| {
        let mut u = format!(
            "https://api.github.com/repos/{owner}/{repo}/contents/{}",
            p
        );
        if !ref_.is_empty() {
            u.push_str(&format!("?ref={ref_}"));
        }
        u
    };
    if skill_path.is_empty() {
        skill_path = resolve_repo_root_skill_dir(&owner, &repo, entry_name)?;
    }
    let dir_name = safe_skill_dir_name(if skill_path.is_empty() { entry_name } else { &skill_path })
        .ok_or_else(|| "That skill has a name this app will not create a folder for.".to_string())?;
    let root = dirs::home_dir().unwrap_or_default().join(".claude").join("skills");
    let dest = root.join(&dir_name);
    if dest.exists() {
        return Err(format!("Already installed at {}", dest.display()));
    }

    // Walk first, write later — a mid-download failure cannot leave a
    // half-installed skill an agent would then load.
    let mut files: Vec<(String, String)> = Vec::new();
    let mut total = 0u64;
    let base_len = skill_path.trim_start_matches('/').len();
    fn walk(
        api: &dyn Fn(&str) -> String,
        path: &str,
        depth: usize,
        files: &mut Vec<(String, String)>,
        total: &mut u64,
        base_len: usize,
    ) -> std::result::Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("the folder nests deeper than this installer will follow".into());
        }
        let v = gh_get_json(&api(path)).map_err(|e| e.to_string())?;
        let listing: Vec<GhEntry> = if v.is_array() {
            serde_json::from_value(v).map_err(|e| e.to_string())?
        } else {
            vec![serde_json::from_value(v).map_err(|e| e.to_string())?]
        };
        for it in listing {
            if files.len() >= MAX_FILES {
                return Err("that skill has more files than this installer will fetch".into());
            }
            if it.kind == "dir" {
                walk(api, &it.path, depth + 1, files, total, base_len)?;
                continue;
            }
            // Only regular files — symlink/submodule entries are skipped.
            if it.kind != "file" {
                continue;
            }
            let Some(durl) = it.download_url.clone() else { continue };
            *total += it.size.unwrap_or(0);
            if *total > MAX_TOTAL_BYTES {
                return Err("that skill is larger than this installer will fetch".into());
            }
            let rel = if base_len > 0 && it.path.len() > base_len {
                it.path[base_len..].trim_start_matches('/').to_string()
            } else {
                it.path.clone()
            };
            files.push((rel, durl));
        }
        Ok(())
    }
    walk(&api, &skill_path, 0, &mut files, &mut total, base_len)?;
    if files.is_empty() {
        return Err("No files found at that source.".into());
    }

    let dest_canon = dest.canonicalize().unwrap_or(dest.clone());
    for (rel, url) in &files {
        let target = dest.join(rel);
        // Post-resolution containment — the only check that survives a
        // crafted path.
        let parent_canon = target
            .parent()
            .map(|p| {
                let _ = fs::create_dir_all(p);
                p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
            })
            .unwrap_or_default();
        if !parent_canon.starts_with(&dest_canon) && parent_canon != dest_canon {
            let _ = fs::remove_dir_all(&dest);
            return Err(format!("refusing to write outside the skill folder: {rel}"));
        }
        let mut resp = ureq::get(url).call().map_err(|e| e.to_string())?;
        let body = resp.body_mut().read_to_vec().map_err(|e| e.to_string())?;
        if let Err(e) = fs::write(&target, body) {
            let _ = fs::remove_dir_all(&dest);
            return Err(e.to_string());
        }
    }
    Ok(dest.display().to_string())
}

/// Remove an installed skill. Refuses anything it cannot prove is a skill
/// folder inside a managed skills root — the failure mode of getting this
/// wrong is deleting a directory of the user's work.
pub fn uninstall_skill(skill_path: &str, cwds: &[String]) -> std::result::Result<(), String> {
    let target = PathBuf::from(skill_path.trim());
    if skill_path.trim().is_empty() {
        return Err("no path given".into());
    }
    let target = target.canonicalize().map_err(|_| "unreadable path".to_string())?;
    let home = dirs::home_dir().unwrap_or_default();
    let mut roots = vec![
        home.join(".claude").join("skills"),
        home.join(".config").join("opencode").join("plugin"),
        home.join(".codex").join("plugins"),
    ];
    for c in cwds.iter().filter(|c| !c.is_empty()) {
        roots.push(Path::new(c).join(".claude").join("skills"));
        roots.push(Path::new(c).join(".opencode").join("plugin"));
    }
    let roots: Vec<PathBuf> = roots
        .iter()
        .map(|r| r.canonicalize().unwrap_or_else(|_| r.clone()))
        .collect();
    let Some(root) = roots.iter().find(|r| target.starts_with(r.as_path())) else {
        return Err("That folder is not inside a skills directory this app manages.".into());
    };
    if target == *root {
        return Err("refusing to delete the skills directory itself".into());
    }
    if !target.exists() {
        return Err("Already gone.".into());
    }
    if target.is_dir() && !target.join("SKILL.md").exists() {
        return Err("That folder has no SKILL.md — refusing to delete it.".into());
    }
    fs::remove_dir_all(&target)
        .or_else(|_| fs::remove_file(&target))
        .map_err(|e| e.to_string())
}
