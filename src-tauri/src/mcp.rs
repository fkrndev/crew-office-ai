//! MCP catalog — Rust mirror of src/shared/mcpCatalog.ts. The TS file is the
//! renderer's consent UI source-of-truth; this table is the SPAWN-time mirror
//! that builds each agent's `settings.json → mcpServers` map. Keep the two in
//! sync: same ids, same tiers, same transport. Any env value left EMPTY here
//! is resolved at merge time (keychain → placeholder), never hard-coded.
//!
//! Per-agent opt-in rides on top of the global `config.mcpDefaults` consent:
//! SpawnArgs.mcp_enable adds ids for one spawn, mcp_disable removes globally
//! consented ones — the write/secret tier can never ride in on a default,
//! matching upstream buildDefaultMcpServers (hive.ts:1318).

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum McpTier {
    SafeReadonly,
    Write,
    Secret,
}

pub struct McpEntry {
    pub id: &'static str,
    pub tier: McpTier,
    pub command: &'static str,
    /// `<cwd>` is replaced with the agent cwd; `<harness>` with the hive root —
    /// both resolved at merge so a stored template never points whole-disk.
    pub args: &'static [&'static str],
    /// Keys are literal env names; values are merge-time hints: "" = resolve
    /// from the keychain, anything else = a literal default.
    pub env: &'static [(&'static str, &'static str)],
    /// Safe-readonly entries are ON unless disabled; the rest need consent.
    pub default_enabled: bool,
}

const fn safe(id: &'static str, command: &'static str, args: &'static [&'static str]) -> McpEntry {
    McpEntry {
        id,
        tier: McpTier::SafeReadonly,
        command,
        args,
        env: &[],
        default_enabled: true,
    }
}

const fn keyed(
    id: &'static str,
    tier: McpTier,
    command: &'static str,
    args: &'static [&'static str],
    env: &'static [(&'static str, &'static str)],
) -> McpEntry {
    McpEntry { id, tier, command, args, env, default_enabled: false }
}

/// The spawn-time catalog. Order matches mcpCatalog.ts; comments carry the
/// TODO-verify flags the TS side already had.
pub const CATALOG: &[McpEntry] = &[
    safe("sequential-thinking", "npx", &["-y", "@modelcontextprotocol/server-sequential-thinking"]),
    safe("time", "uvx", &["mcp-server-time"]),
    safe("fetch", "uvx", &["mcp-server-fetch"]),
    safe("context7", "npx", &["-y", "@upstash/context7-mcp"]),
    safe("filesystem", "npx", &["-y", "@modelcontextprotocol/server-filesystem", "<cwd>"]),
    safe("git", "uvx", &["mcp-server-git", "--repository", "<cwd>"]),
    keyed(
        "github-token",
        McpTier::Secret,
        "npx",
        &["-y", "@modelcontextprotocol/server-github"],
        &[("GITHUB_PERSONAL_ACCESS_TOKEN", "")],
    ),
    keyed("db", McpTier::Secret, "npx", &["-y", "@modelcontextprotocol/server-postgres"], &[("DATABASE_URL", "")]),
    keyed("email-calendar", McpTier::Secret, "npx", &["-y", "@modelcontextprotocol/server-gsuite"], &[("GOOGLE_OAUTH_TOKEN", "")]),
    keyed("search-with-key", McpTier::Secret, "npx", &["-y", "@modelcontextprotocol/server-brave-search"], &[("BRAVE_API_KEY", "")]),
    // Trello boards via the free REST API — key+token from trello.com/app-key.
    keyed(
        "trello",
        McpTier::Secret,
        "npx",
        &["-y", "@delorenj/mcp-server-trello"],
        &[("TRELLO_API_KEY", ""), ("TRELLO_TOKEN", "")],
    ),
    // Google Sheets via service account — the env value is the key-JSON file
    // PATH (keychain stores the path string, not the JSON itself).
    keyed(
        "google-sheets",
        McpTier::Secret,
        "npx",
        &["-y", "@whizzy/mcp-sheets"],
        &[("SERVICE_ACCOUNT_PATH", ""), ("DRIVE_FOLDER_ID", "")],
    ),
    // ShardX anti-detect browser — the multi-account marketplace story. The
    // launcher lives on the user's machine (HTTP API on 127.0.0.1:40325); the
    // MCP bridge is a Node source tree we vendor under <harness>/mcp-servers/
    // shardx/ via `shardx_setup`. SHARDX_TOKEN resolves from the keychain —
    // never persisted in config.
    keyed(
        "shardx",
        McpTier::Secret,
        "node",
        &["<harness>/mcp-servers/shardx/index.js"],
        &[("SHARDX_API", "http://127.0.0.1:40325"), ("SHARDX_TOKEN", "")],
    ),
];

pub fn catalog_entry(id: &str) -> Option<&'static McpEntry> {
    CATALOG.iter().find(|e| e.id == id)
}

/// One server entry in the written `mcpServers` map.
#[derive(Clone, Serialize)]
pub struct McpServerSpec {
    pub command: String,
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

/// Build the per-agent `mcpServers` map (upstream hive.ts buildDefaultMcpServers):
///
/// * catalog ∩ consent — global `mcp_defaults` map plus per-spawn
///   `enable`/`disable` lists;
/// * write/secret entries need an EXPLICIT enable (global consent or per-agent),
///   never a default — a hand-edited config can't silently arm a keyed server;
/// * `<cwd>`/`<harness>` placeholders resolve at merge;
/// * env hints "" resolve through `secret_lookup` (keychain); unresolved
///   secrets leave the empty string — the server fails loudly, never guesses;
/// * every id is namespaced `munder-<id>` so the user's own ~/.claude servers
///   of the same name are never clobbered.
pub fn build_default_mcp_servers(
    cwd: &str,
    harness_root: &std::path::Path,
    defaults: &HashMap<String, bool>,
    enable: &[String],
    disable: &[String],
    secret_lookup: &dyn Fn(&str) -> Option<String>,
) -> BTreeMap<String, McpServerSpec> {
    let mut out = BTreeMap::new();
    for e in CATALOG {
        let mut on = defaults.get(e.id).copied().unwrap_or(e.default_enabled);
        if enable.iter().any(|i| i == e.id) {
            on = true;
        }
        if disable.iter().any(|i| i == e.id) {
            on = false;
        }
        if !on {
            continue;
        }
        // Defense-in-depth: a write/secret server requires an explicit opt-in —
        // either the global consent map or this spawn's mcp_enable.
        if e.tier != McpTier::SafeReadonly
            && !(defaults.get(e.id).copied().unwrap_or(false) || enable.iter().any(|i| i == e.id))
        {
            continue;
        }
        let args: Vec<String> = e
            .args
            .iter()
            .map(|a| {
                a.replace("<cwd>", cwd).replace(
                    "<harness>",
                    &harness_root.display().to_string(),
                )
            })
            .collect();
        let mut env = BTreeMap::new();
        for (k, hint) in e.env {
            let v = if hint.is_empty() {
                secret_lookup(k).unwrap_or_default()
            } else {
                hint.to_string()
            };
            env.insert(k.to_string(), v);
        }
        out.insert(
            format!("munder-{}", e.id),
            McpServerSpec { command: e.command.into(), args, env },
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn no_secret(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn defaults_ship_safe_readonly_on_and_keyed_off() {
        let m = build_default_mcp_servers("C:/w", Path::new("C:/h"), &HashMap::new(), &[], &[], &no_secret);
        assert!(m.contains_key("munder-filesystem"));
        assert!(m.contains_key("munder-sequential-thinking"));
        // keyed tiers can never ride a default
        assert!(!m.contains_key("munder-shardx"));
        assert!(!m.contains_key("munder-github-token"));
    }

    #[test]
    fn cwd_placeholder_scopes_filesystem() {
        let m = build_default_mcp_servers("C:/work/repo", Path::new("C:/h"), &HashMap::new(), &[], &[], &no_secret);
        let fs = &m["munder-filesystem"];
        assert!(fs.args.iter().any(|a| a == "C:/work/repo"));
        assert!(!fs.args.iter().any(|a| a.contains("<cwd>")));
    }

    #[test]
    fn per_spawn_enable_arms_shardx_and_resolves_paths() {
        let m = build_default_mcp_servers(
            "C:/w",
            Path::new("C:/harness"),
            &HashMap::new(),
            &["shardx".to_string()],
            &[],
            &|k| (k == "SHARDX_TOKEN").then(|| "tok-1".to_string()),
        );
        let s = &m["munder-shardx"];
        assert_eq!(s.command, "node");
        assert_eq!(s.args[0], "C:/harness/mcp-servers/shardx/index.js");
        assert_eq!(s.env["SHARDX_API"], "http://127.0.0.1:40325");
        assert_eq!(s.env["SHARDX_TOKEN"], "tok-1");
    }

    #[test]
    fn disable_beats_enable_and_global_consent() {
        let mut defaults = HashMap::new();
        defaults.insert("fetch".to_string(), true);
        let m = build_default_mcp_servers("C:/w", Path::new("C:/h"), &defaults, &[], &["fetch".to_string()], &no_secret);
        assert!(!m.contains_key("munder-fetch"));
        // per-spawn enable wins over global default-off for safe tiers too
        let m2 = build_default_mcp_servers("C:/w", Path::new("C:/h"), &HashMap::new(), &["shardx".to_string()], &["shardx".to_string()], &no_secret);
        assert!(!m2.contains_key("munder-shardx"));
    }

    #[test]
    fn unknown_ids_are_ignored() {
        let m = build_default_mcp_servers("C:/w", Path::new("C:/h"), &HashMap::new(), &["nope".to_string()], &[], &no_secret);
        assert!(!m.keys().any(|k| k.contains("nope")));
    }
}
