/**
 * buildSpawnCommand — the string every agent terminal actually runs.
 * Pins each provider's compose rules: preset binary, model flag quoting,
 * auto-mode flag, and the claude/custom fallbacks. A regression here spawns
 * a CLI with wrong flags — an agent that either refuses to start or runs
 * with no skip-permissions and waits forever for approvals.
 */
import { describe, it, expect } from 'vitest';
import { buildSpawnCommand } from '@/store/config';

const CFG = (autoMode: boolean, defaultCommand = 'claude') =>
  ({ autoMode, defaultCommand });

describe('buildSpawnCommand', () => {
  it('hermes: preset binary + --model + --yolo under auto mode', () => {
    expect(buildSpawnCommand(CFG(true), 'anthropic/claude-sonnet-4.6', 'hermes'))
      .toBe('hermes chat --model anthropic/claude-sonnet-4.6 --yolo');
  });

  it('hermes: no --yolo when autoMode is off', () => {
    expect(buildSpawnCommand(CFG(false), undefined, 'hermes'))
      .toBe('hermes chat');
  });

  it('openclaw: tui --local; NO --model (tui rejects it → argv-dead spawn) and NO auto flag', () => {
    // `openclaw tui` accepts only --local/--url/--token/--session/--deliver/
    // --thinking/--message/--timeout-ms/--history-limit — a --model here is an
    // argparse reject = spawn that never comes up (same class as hermes' --cli).
    // The picked model rides SpawnArgs.model → seeded openclaw.json instead.
    expect(buildSpawnCommand(CFG(true), 'openrouter/auto', 'openclaw'))
      .toBe('openclaw tui --local');
    expect(buildSpawnCommand(CFG(false), undefined, 'openclaw'))
      .toBe('openclaw tui --local');
  });

  it('claude: uses the configured defaultCommand, not the preset', () => {
    expect(buildSpawnCommand(CFG(true, 'claude --verbose'), 'opus', 'claude'))
      .toBe('claude --verbose --model opus --permission-mode bypassPermissions');
  });

  it('custom: falls back to the user\'s defaultCommand verbatim', () => {
    expect(buildSpawnCommand(CFG(false, 'my-agent --flag'), undefined, 'custom'))
      .toBe('my-agent --flag');
  });

  it('model values containing whitespace get quoted', () => {
    expect(buildSpawnCommand(CFG(false), 'Gemini 3.1 Pro (High)', 'antigravity'))
      .toContain('"Gemini 3.1 Pro (High)"');
  });

  it('providers without a model flag ignore the model', () => {
    // cursor/copilot supportModel per their presets — pick one that does NOT:
    const cmd = buildSpawnCommand(CFG(false), 'ignored-model', 'custom');
    expect(cmd).not.toContain('ignored-model');
  });
});
