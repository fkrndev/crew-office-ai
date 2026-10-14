/**
 * Hermes-first UX audit regressions: model pickers, capability notes, engine
 * suggestion, probe normalisation, Hermes context commands, locale parity.
 */
import { describe, it, expect } from 'vitest';
import en from '@/i18n/locales/en.json';
import id from '@/i18n/locales/id.json';
import ar from '@/i18n/locales/ar.json';
import zh from '@/i18n/locales/zh-CN.json';
import { defaultAgentProvider, modelsForProvider, modelsForProviderAtVersion, onboardingEngineChoices } from '@/store/config';
import { capabilitiesForProvider, providerNoteKeys } from '@/shared/providerCapabilities';
import {
  clearCommandForProvider,
  compactionCommandForProvider,
  isCompactionCommand
} from '@/shared/providerAutomation';
import { suggestOrchestratorEngine, classifyEngineAvailability } from '@/shared/engineAvailability';
import { normalizeToolsProbe, toolCatalog } from '@/shared/toolCatalog';

describe('model pickers', () => {
  it('providers with no catalog entry get an EMPTY list (freeform field), never the Claude list', () => {
    expect(modelsForProvider('hermes')).toEqual([]);
    expect(modelsForProvider('openclaw')).toEqual([]);
    expect(modelsForProvider('custom')).toEqual([]);
    expect(modelsForProvider('claude').length).toBeGreaterThan(0);
    expect(modelsForProvider('codex').every((m) => !String(m.id).startsWith('claude'))).toBe(true);
  });

  it('an id that is not a preset at all still falls back to the Claude list', () => {
    const fallback = modelsForProviderAtVersion('zzz' as never, '99.0.0');
    expect(fallback.length).toBeGreaterThan(0);
  });
});

describe('add-agent default engine', () => {
  it('follows the orchestrator engine unless the default command was customised', () => {
    expect(defaultAgentProvider({ defaultCommand: 'claude', godProvider: 'hermes' })).toBe('hermes');
    expect(defaultAgentProvider({ defaultCommand: 'claude', godProvider: 'claude' })).toBe('claude');
    expect(defaultAgentProvider({ defaultCommand: 'claude', godProvider: undefined })).toBe('claude');
    expect(defaultAgentProvider({ defaultCommand: 'codex', godProvider: 'hermes' })).toBe('codex');
    expect(defaultAgentProvider({ defaultCommand: 'claude', godProvider: 'custom' })).toBe('claude');
  });
});

describe('provider capabilities', () => {
  it('claude has nothing to warn about', () => {
    expect(providerNoteKeys('claude', { autoMode: true })).toEqual([]);
  });

  it('hermes: first-run hint, honest auto-mode note, no-telemetry and own-skills notes', () => {
    const on = providerNoteKeys('hermes', { autoMode: true });
    expect(on).toContain('providerNotes.hermes.firstRunHint');
    expect(on).toContain('providerNotes.hermes.autoNote');
    expect(on).toContain('providerNotes.noUsage');
    expect(on).toContain('providerNotes.ownSkills');
    expect(providerNoteKeys('hermes', { autoMode: false })).not.toContain('providerNotes.hermes.autoNote');
    expect(capabilitiesForProvider('hermes').modelHintKey).toBe('providerNotes.hermes.modelHint');
    expect(capabilitiesForProvider('hermes').autoModeFlag).toBe('--yolo');
  });

  it('every note key resolves in every locale', () => {
    for (const keys of [providerNoteKeys('hermes', { autoMode: true }), ['providerNotes.hermes.modelHint']]) {
      for (const k of keys) {
        for (const loc of [en, id, ar, zh]) {
          const v = k.split('.').reduce<unknown>((o, p) => (o as Record<string, unknown>)?.[p], loc);
          expect(typeof v, k).toBe('string');
        }
      }
    }
  });
});

describe('hermes context commands (verified against hermes_cli/commands.py)', () => {
  it('compacts with /compress (with focus) and clears with /new', () => {
    expect(compactionCommandForProvider('hermes', 'keep the task')).toBe('/compress keep the task');
    expect(compactionCommandForProvider('hermes', '')).toBe('/compress');
    expect(clearCommandForProvider('hermes')).toBe('/new');
    expect(isCompactionCommand('/compress keep the task')).toBe(true);
  });
  it('providers with no typed command stay null', () => {
    expect(compactionCommandForProvider('crush')).toBeNull();
    expect(clearCommandForProvider('cursor')).toBeNull();
  });
});

describe('suggestOrchestratorEngine', () => {
  const eligible = onboardingEngineChoices().eligible.map((p) => p.id);
  const probe = (installed: string[]) =>
    normalizeToolsProbe(
      ['claude', 'codex', 'hermes', 'gemini', 'openclaw'].map((id) => ({
        id, found: installed.includes(id), path: installed.includes(id) ? `C:\\bin\\${id}.exe` : null
      }))
    );

  it('keeps Claude when it is installed', () => {
    expect(suggestOrchestratorEngine(probe(['claude', 'hermes']), eligible)).toBe('claude');
  });
  it('prefers Hermes over other installed engines when Claude is missing', () => {
    expect(suggestOrchestratorEngine(probe(['codex', 'hermes', 'gemini']), eligible)).toBe('hermes');
  });
  it('takes the first installed engine otherwise', () => {
    expect(suggestOrchestratorEngine(probe(['gemini', 'openclaw']), eligible)).toBe('gemini');
  });
  it('falls back to Claude when nothing is installed or the probe has not run', () => {
    expect(suggestOrchestratorEngine(probe([]), eligible)).toBe('claude');
    expect(suggestOrchestratorEngine(undefined, eligible)).toBe('claude');
  });
  it('never suggests an engine outside the eligible list', () => {
    expect(suggestOrchestratorEngine(probe(['hermes']), ['claude', 'codex'])).toBe('claude');
  });
});

describe('normalizeToolsProbe', () => {
  it('joins bare native rows with the catalog so engines classify', () => {
    const rows = normalizeToolsProbe([
      { id: 'claude', found: true, path: 'C:\\Users\\x\\.local\\bin\\claude.exe' },
      { id: 'hermes', found: true, path: 'C:\\hermes.exe' },
      { id: 'codex', found: false, path: null }
    ], 'win32');
    expect(rows.map((r) => r.id).sort()).toEqual(['engine:claude', 'engine:codex', 'engine:hermes']);
    expect(classifyEngineAvailability(rows, 'hermes').state).toBe('installed');
    expect(classifyEngineAvailability(rows, 'codex').state).toBe('installs-on-first-run');
    // not probed (kimi/copilot/cursor, git, uv…) => unknown, never "missing"
    expect(classifyEngineAvailability(rows, 'cursor').state).toBe('unknown');
    expect(rows.every((r) => r.kind === 'engine')).toBe(true);
  });
  it('passes already catalog-shaped rows through and tolerates junk', () => {
    const shaped = [{ id: 'engine:claude', kind: 'engine', found: true, path: 'x', installCommand: '' }];
    expect(normalizeToolsProbe(shaped)).toEqual(shaped);
    expect(normalizeToolsProbe(null)).toEqual([]);
    expect(normalizeToolsProbe([1, 'a', null])).toEqual([]);
  });
  it('no engine is "essential" any more (set-up-everything must not install Claude for a Hermes user)', () => {
    expect(toolCatalog().filter((t) => t.kind === 'engine').some((t) => t.essential)).toBe(false);
  });
});

describe('locale parity for the keys this audit touched', () => {
  const flat = (o: unknown, p = ''): Record<string, string> =>
    Object.entries(o as Record<string, unknown>).reduce<Record<string, string>>((acc, [k, v]) => {
      const q = p ? `${p}.${k}` : k;
      if (v && typeof v === 'object') Object.assign(acc, flat(v, q));
      else acc[q] = String(v);
      return acc;
    }, {});
  const ph = (s: string) => [...s.matchAll(/\{\{\s*(\w+)\s*\}\}/g)].map((m) => m[1]).sort().join(',');
  it('same key set and placeholders in all four locales', () => {
    const fe = flat(en);
    for (const [name, loc] of Object.entries({ id, ar, zh })) {
      const f = flat(loc);
      for (const k of Object.keys(fe).filter((k) => /^(providerNotes\.|aiEngines\.(keysScope|hermesHint)|skillsTab\.engineNote|settings\.autonomy\.usageNote|setupPanel\.missingSuffix|addAgent\.(resumePlaceholder|needsKey)|onboarding\.orchestrator\.(recommended|cliAgent))/.test(k))) {
        expect(f[k], `${name}:${k}`).toBeTruthy();
        expect(ph(f[k]), `${name}:${k} placeholders`).toBe(ph(fe[k]));
      }
    }
  });
});
