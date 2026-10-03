//! Provider bridges that are bundled *files* rather than hook-shim modes —
//! Pi (earendil-works) and OpenCode have no Claude-shaped hook stdin, so the
//! upstream shipped small JS plugins that post to HIVE_SOCK directly. The
//! JS is ported verbatim (it's self-contained and protocol-agnostic); only
//! the installer moved to Rust.
//!
//! Both are LIVE-UNVERIFIED upstream too — best-effort, fully wrapped, and
//! the renderer-side idle nudge is the guaranteed drain regardless.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

/// Pi extension — posts cth-hook-shaped payloads on tool_call / tool_result /
/// agent_end and auto-approves tool calls when the floor is in auto mode
/// (HIVE_AUTO_APPROVE — the Pam guardrail #5 from upstream). Verbatim port of
/// PI_EXTENSION (hive.ts:3232); runs on Pi's own JS runtime.
const PI_EXTENSION: &str = r#"'use strict';
var net = require('node:net');
var SOCK = process.env.HIVE_SOCK;
var AGENT = process.env.AGENT_ID || null;
var AUTO = process.env.HIVE_AUTO_APPROVE === '1';
function post(payload) {
  try {
    if (!SOCK) return;
    payload.agent_id = payload.agent_id || AGENT;
    var c = net.createConnection(SOCK, function () { try { c.end(JSON.stringify(payload) + '\n'); } catch (e) {} });
    c.on('error', function () {});
  } catch (e) {}
}
function firstDefined(primary, fallback) {
  return primary !== undefined && primary !== null ? primary : fallback;
}
function piField(ev, key) {
  try { return ev == null ? undefined : ev[key]; } catch (e) { return undefined; }
}
function piToolName(ev) {
  var tool = piField(ev, 'tool');
  return firstDefined(piField(ev, 'toolName'), firstDefined(piField(ev, 'name'), piField(tool, 'name')));
}
function piToolInput(ev) {
  return firstDefined(piField(ev, 'input'), piField(ev, 'args'));
}
function piToolPayload(hookEventName, ev) {
  return {
    hook_event_name: hookEventName,
    tool_name: piToolName(ev),
    tool_input: piToolInput(ev)
  };
}
function register(pi) {
  if (!pi || typeof pi.on !== 'function') return false;
  try {
    pi.on('tool_call', function (ev) {
      post(piToolPayload('PreToolUse', ev));
      if (AUTO) { try { if (ev && typeof ev.approve === 'function') ev.approve(); } catch (e) {} return { approve: true }; }
      return undefined;
    });
    pi.on('tool_result', function (ev) { post(piToolPayload('PostToolUse', ev)); });
    pi.on('agent_end', function () { post({ hook_event_name: 'Stop' }); });
    return true;
  } catch (e) { return false; }
}
try { if (typeof globalThis !== 'undefined' && globalThis.pi) register(globalThis.pi); } catch (e) {}
module.exports = function (pi) { return register(pi); };
module.exports.activate = function (pi) { return register(pi); };
module.exports.default = module.exports;
"#;

/// OpenCode plugin — posts on tool.execute.before/after + session.idle→Stop
/// (OpenCode has no Claude-shaped Stop hook; session.idle is the real
/// lifecycle event). Verbatim port of OPENCODE_PLUGIN (hive.ts:3291); ESM.
const OPENCODE_PLUGIN: &str = r#"import { createConnection } from 'node:net';
const SOCK = process.env.HIVE_SOCK;
const AGENT = process.env.AGENT_ID || null;
function post(payload) {
  try {
    if (!SOCK) return;
    payload.agent_id = payload.agent_id || AGENT;
    const c = createConnection(SOCK, () => { try { c.end(JSON.stringify(payload) + '\n'); } catch (e) {} });
    c.on('error', () => {});
  } catch (e) {}
}
export const HiveBridge = async () => {
  return {
    event: async (input) => {
      try { if (input && input.event && input.event.type === 'session.idle') post({ hook_event_name: 'Stop' }); } catch (e) {}
    },
    'tool.execute.before': async (input) => {
      try { post({ hook_event_name: 'PreToolUse', tool_name: input && (input.tool || input.name) }); } catch (e) {}
    },
    'tool.execute.after': async (input) => {
      try { post({ hook_event_name: 'PostToolUse', tool_name: input && (input.tool || input.name) }); } catch (e) {}
    }
  };
};
export default HiveBridge;
"#;

/// installPiHooks — per-agent `.pi-agent/` with the extension + manifest +
/// the user's models config copied across. Returns PI_CODING_AGENT_DIR.
pub fn install_pi_hooks(dir: &Path) -> Result<PathBuf> {
    let home = dir.join(".pi-agent");
    let ext_dir = home.join("extensions");
    fs::create_dir_all(&ext_dir)?;
    fs::write(ext_dir.join("hive-bridge.js"), PI_EXTENSION)?;
    fs::write(
        home.join("extensions.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "name": "agent-office-hive-bridge",
            "version": "0.3.2",
            "main": "extensions/hive-bridge.js",
            "auto": true
        }))?,
    )?;
    // Pi keeps model creds in ~/.pi/agent — carry them into the isolated dir.
    let user_pi = dirs::home_dir().unwrap_or_default().join(".pi").join("agent");
    for name in ["models.json", "models-store.json"] {
        let (src, dst) = (user_pi.join(name), home.join(name));
        if src.exists() && !dst.exists() {
            let _ = fs::copy(&src, &dst);
        }
    }
    Ok(home)
}

/// installCrushConfig — Crush has NO base-URL env override, so its global
/// config is pointed at the loopback proxy instead: per-agent CRUSH_GLOBAL_CONFIG
/// (crush.json) rewrites ONLY the matching wire provider's base_url (a
/// different-wire provider would hit the wrong endpoint — left to its real
/// upstream, mail still drains via the nudge path). CRUSH_GLOBAL_DATA isolates
/// session state from ~/.config/crush.
pub fn install_crush_config(dir: &Path, loopback_url: &str, api_openai: bool) -> Result<(PathBuf, PathBuf)> {
    let config = dir.join("crush.json");
    let data = dir.join(".crush-data");
    fs::create_dir_all(&data)?;
    let wire = if api_openai { "openai" } else { "anthropic" };
    fs::write(
        &config,
        serde_json::to_string_pretty(&serde_json::json!({
            "providers": { wire: { "base_url": loopback_url } },
            "options": { "tui": { "transparent": true } }
        }))?,
    )?;
    Ok((config, data))
}

/// installOpenCodePlugin — per-agent `.opencode/` config dir with the plugin
/// written to BOTH `plugin/` and `plugins/` (docs vs older builds — the
/// losing guess is silently-never-loads, so cover both). Returns
/// OPENCODE_CONFIG_DIR.
pub fn install_opencode_plugin(dir: &Path) -> Result<PathBuf> {
    let home = dir.join(".opencode");
    // `theme: system` keeps xterm's own fg/bg — reads right on light & dark.
    let choice = serde_json::json!({ "theme": "system" });
    fs::create_dir_all(&home)?;
    fs::write(
        home.join("tui.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "$schema": "https://opencode.ai/tui.json",
            "theme": choice["theme"]
        }))?,
    )?;
    fs::write(
        home.join("opencode.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "$schema": "https://opencode.ai/config.json",
            "theme": choice["theme"]
        }))?,
    )?;
    for name in ["plugin", "plugins"] {
        let pdir = home.join(name);
        fs::create_dir_all(&pdir)?;
        fs::write(pdir.join("hive-bridge.js"), OPENCODE_PLUGIN)?;
    }
    Ok(home)
}
