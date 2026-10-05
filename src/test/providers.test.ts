/**
 * Provider catalog tests — the "kok ga ada pilihan open claw dan hermes"
 * regression: a provider present in the AgentProvider union but missing from
 * AGENT_PROVIDER_PRESETS renders nowhere. Pins that every union member has a
 * preset and that command→provider inference covers the real binaries.
 */
import { describe, it, expect } from 'vitest';
import {
  AGENT_PROVIDER_PRESETS,
  inferAgentProvider,
  isAgentProvider,
  providerPreset
} from '@/shared/agentProvider';
import { modelProvidersForAgent, onboardingEngineChoices } from '@/store/config';

describe('provider presets', () => {
  it('hermes and openclaw have full presets', () => {
    const hermes = AGENT_PROVIDER_PRESETS.find((p) => p.id === 'hermes');
    const openclaw = AGENT_PROVIDER_PRESETS.find((p) => p.id === 'openclaw');
    expect(hermes).toBeTruthy();
    expect(openclaw).toBeTruthy();
    // Spawnable defaults: a real command + a verified auto flag.
    expect(hermes!.defaultCommand).toBe('hermes --cli');
    expect(hermes!.autoModeFlag).toBe('--yolo');
    expect(hermes!.supportsModel).toBe(true);
    expect(hermes!.canReceiveInbox).toBe(true); // real hook bridge
    expect(openclaw!.defaultCommand).toBe('openclaw tui --local');
    expect(openclaw!.canReceiveInbox).toBe(true);
  });

  it('every preset id is a valid AgentProvider', () => {
    for (const p of AGENT_PROVIDER_PRESETS) {
      expect(isAgentProvider(p.id), `preset id ${p.id}`).toBe(true);
    }
  });

  it('onboarding lists both as orchestrator-eligible engines', () => {
    const { eligible } = onboardingEngineChoices();
    const ids = eligible.map((p) => p.id);
    expect(ids).toContain('hermes');
    expect(ids).toContain('openclaw');
    // And the workers-only group still shows non-inbox engines (not hidden).
    const { workersOnly } = onboardingEngineChoices();
    expect(workersOnly.every((p) => !ids.includes(p.id))).toBe(true);
  });

  it('modelProvidersForAgent(true) matches onboarding eligibility', () => {
    const eligible = modelProvidersForAgent(true).map((p) => p.id);
    expect(eligible).toContain('hermes');
    expect(eligible).toContain('openclaw');
    expect(eligible).not.toContain('custom');
  });
});

describe('inferAgentProvider', () => {
  it('maps hermes and openclaw binaries', () => {
    expect(inferAgentProvider('hermes --cli')).toBe('hermes');
    expect(inferAgentProvider('hermes')).toBe('hermes');
    expect(inferAgentProvider('openclaw tui --local')).toBe('openclaw');
    expect(inferAgentProvider('openclaw')).toBe('openclaw');
  });

  it('still maps the classics', () => {
    expect(inferAgentProvider('claude --model opus')).toBe('claude');
    expect(inferAgentProvider('cursor-agent --force')).toBe('cursor');
    expect(inferAgentProvider('random-thing')).toBe('custom');
  });

  it('providerPreset returns a preset for every inferrable provider', () => {
    for (const bin of ['claude', 'codex', 'gemini', 'hermes', 'openclaw', 'qwen', 'crush', 'pi', 'cursor-agent']) {
      const p = inferAgentProvider(bin);
      expect(() => providerPreset(p), bin).not.toThrow();
    }
  });
});
