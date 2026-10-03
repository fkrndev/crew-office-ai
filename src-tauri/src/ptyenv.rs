//! Port of `src/main/ptyEnv.ts` — environment construction for agent PTYs as a
//! pure function so the layering stays testable off-process.
//!
//! Layering rule, bottom to top (verbatim from the Electron version):
//!   1. the inherited environment, minus the parent Claude session's identity
//!   2. the app's own defaults (PATH, terminal identity, locale)
//!   3. per-agent values (`agent_env`) — always win, even over the strip below

use std::collections::HashMap;

/// Matches /^CLAUDE(CODE|_)/ — the app is often launched from INSIDE a Claude
/// Code session, so that session's identity markers arrive via the parent env
/// and would flow into every agent CLI. CLAUDE_CODE_CHILD_SESSION would make
/// the agent believe it is a child session and silently DISABLE transcript
/// saving, which breaks --resume for every agent of that run. Agents are
/// top-level sessions regardless of how the app was launched.
fn is_claude_marker(key: &str) -> bool {
    key.starts_with("CLAUDECODE") || key.starts_with("CLAUDE_")
}

/// Configuration, not identity: these share the prefix but are the OPERATOR's
/// own choices — where the CLI keeps its config, how it authenticates, which
/// backend serves it. Stripping them breaks agents in exactly the quiet way
/// the marker strip exists to prevent.
const CLAUDE_CONFIG_KEEP: &[&str] = &[
    "CLAUDE_CONFIG_DIR",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
];

pub fn build_pty_env(
    parent_env: &HashMap<String, String>,
    user_path: &str,
    agent_env: Option<&HashMap<String, String>>,
    is_windows: bool,
) -> HashMap<String, String> {
    // Layer 1 — inherit, minus the parent session's Claude identity. Only this
    // layer is stripped: a marker set deliberately via `agent_env` survives, so
    // per-agent environment overrides cannot be silently wiped by the strip.
    let mut env: HashMap<String, String> = parent_env
        .iter()
        .filter(|(k, _)| !(is_claude_marker(k) && !CLAUDE_CONFIG_KEEP.contains(&k.as_str())))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    // Layer 2 — app defaults.
    env.insert("PATH".into(), user_path.into());
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("COLORTERM".into(), "truecolor".into());
    // Help apps that look for a real interactive shell.
    env.insert("FORCE_COLOR".into(), "1".into());

    // A GUI-launched app may inherit NO locale, so children would run in the
    // C/POSIX locale and decode UTF-8 wrongly. This terminal IS UTF-8, so say
    // so. LC_CTYPE only, deliberately — never LC_ALL (would also override
    // collation and date formatting).
    if !is_windows {
        let lang = parent_env
            .get("LANG")
            .cloned()
            .unwrap_or_else(|| "en_US.UTF-8".into());
        let ctype = parent_env
            .get("LC_ALL")
            .or_else(|| parent_env.get("LC_CTYPE"))
            .or_else(|| parent_env.get("LANG"))
            .cloned()
            .unwrap_or_else(|| "en_US.UTF-8".into());
        env.insert("LANG".into(), lang);
        env.insert("LC_CTYPE".into(), ctype);
    }

    // Layer 3 — per-agent hive identity (AGENT_ID, HIVE_ROOT, …); always wins.
    if let Some(agent_env) = agent_env {
        env.extend(agent_env.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_parent_claude_session_markers() {
        let mut parent = HashMap::new();
        parent.insert("CLAUDE_CODE_CHILD_SESSION".into(), "1".into());
        parent.insert("CLAUDE_CONFIG_DIR".into(), "/home/u/.claude".into());
        parent.insert("HOME".into(), "/home/u".into());
        let env = build_pty_env(&parent, "/usr/bin", None, false);
        assert!(!env.contains_key("CLAUDE_CODE_CHILD_SESSION"));
        assert_eq!(env["CLAUDE_CONFIG_DIR"], "/home/u/.claude");
        assert_eq!(env["TERM"], "xterm-256color");
    }

    #[test]
    fn agent_env_wins_over_strip() {
        let mut parent = HashMap::new();
        parent.insert("PATH".into(), "/usr/bin".into());
        let mut agent = HashMap::new();
        agent.insert("CLAUDE_CODE_X".into(), "kept".into());
        agent.insert("AGENT_ID".into(), "a1".into());
        let env = build_pty_env(&parent, "/usr/bin", Some(&agent), true);
        assert_eq!(env["CLAUDE_CODE_X"], "kept");
        assert_eq!(env["AGENT_ID"], "a1");
    }
}
