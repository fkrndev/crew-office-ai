/**
 * engineAvailability — how the onboarding picker (and worker cards) classify
 * each engine from the toolsStatus probe. Informational only: onboarding never
 * blocks on it.
 */
import { describe, it, expect } from 'vitest';
import {
  classifyEngineAvailability,
  engineAvailabilityBadge,
  engineAvailabilityBadgeKey
} from '@/shared/engineAvailability';

type Row = { id: string; found: boolean; path: string | null; installCommand?: string; docsUrl?: string };

const probe = (rows: Row[]) => rows as never;

describe('classifyEngineAvailability', () => {
  it('installed when the probe found a path', () => {
    const a = classifyEngineAvailability(
      probe([{ id: 'engine:claude', found: true, path: 'C:\\bin\\claude.exe' }]),
      'claude'
    );
    expect(a.state).toBe('installed');
    expect(a.path).toBe('C:\\bin\\claude.exe');
  });

  it('installs-on-first-run when missing but the app has an installer', () => {
    const a = classifyEngineAvailability(
      probe([{ id: 'engine:hermes', found: false, path: null, installCommand: 'iex …' }]),
      'hermes'
    );
    expect(a.state).toBe('installs-on-first-run');
    expect(a.installCommand).toBe('iex …');
  });

  it('not-installable when missing with no installer', () => {
    const a = classifyEngineAvailability(
      probe([{ id: 'engine:cursor', found: false, path: null }]),
      'cursor'
    );
    expect(a.state).toBe('not-installable');
  });

  it('unknown when the probe has not run or lacks the row', () => {
    expect(classifyEngineAvailability(undefined, 'claude').state).toBe('unknown');
    expect(classifyEngineAvailability(probe([]), 'claude').state).toBe('unknown');
  });
});

describe('badges', () => {
  it('badges map to the three visible states', () => {
    expect(engineAvailabilityBadge({ state: 'installed', path: null, installCommand: '' })).toBe('INSTALLED');
    expect(engineAvailabilityBadge({ state: 'installs-on-first-run', path: null, installCommand: '' })).toBe('INSTALLS ON FIRST RUN');
    expect(engineAvailabilityBadge({ state: 'not-installable', path: null, installCommand: '' })).toBe('NOT INSTALLED');
    expect(engineAvailabilityBadge({ state: 'unknown', path: null, installCommand: '' })).toBeNull();
  });

  it('every visible state has a localized badge key; unknown has none', () => {
    for (const s of ['installed', 'installs-on-first-run', 'not-installable'] as const) {
      expect(engineAvailabilityBadgeKey({ state: s, path: null, installCommand: '' })).toMatch(/^onboarding\.orchestrator\.badge/);
    }
    expect(engineAvailabilityBadgeKey({ state: 'unknown', path: null, installCommand: '' })).toBeNull();
  });
});
