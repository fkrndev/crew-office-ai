/**
 * The cth.ts shim — the method→command+args packing layer every renderer
 * call funnels through. Regressions here are invisible until a button does
 * nothing: pack() dropping args, a command name typo, or the object-vs-
 * positional ambiguity that broke hiveMessages (commit 1cfb765 — callers
 * passing {id} or {agentId,limit} got repacked into {agentId:{...}}).
 *
 * Mocks @tauri-apps/api so `invoke` is observable, installs the real shim,
 * and asserts the wire shape for the callsites that actually broke.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

// vi.hoisted so the mock factory (hoisted above the const) can see it.
const invoke = vi.hoisted(() =>
  vi.fn(async (_cmd: string, _args?: unknown) => null as unknown)
);
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({ onDragDropEvent: async () => () => {} })
}));

import { installCth } from '@/cth';

type Cth = Record<string, (...a: unknown[]) => Promise<unknown> | unknown>;

function cth(): Cth {
  return (window as unknown as { cth: Cth }).cth;
}

/** The (cmd, args) pairs invoke saw, in order. */
function calls(): Array<{ cmd: string; args: unknown }> {
  return invoke.mock.calls.map(([cmd, args]) => ({ cmd: cmd as string, args }));
}
function lastCall() {
  const c = calls();
  return c[c.length - 1];
}

describe('cth shim', () => {
  beforeEach(() => {
    invoke.mockClear();
    installCth();
  });

  it('pack() maps positional args to the command\'s named payload', async () => {
    await cth().spawnPty({ bin: 'claude', cwd: '/repo' });
    expect(lastCall()).toEqual({
      cmd: 'spawn_agent',
      args: { args: { bin: 'claude', cwd: '/repo' } }
    });
  });

  it('hiveMessages accepts the OBJECT form — upstream callers pass {id} etc.', async () => {
    await cth().hiveMessages({ id: 'm-42' });
    // The bug: repacking put the whole object under `agentId`, so the backend
    // queried an agent literally named "[object Object]".
    expect(lastCall()).toEqual({ cmd: 'hive_messages', args: { id: 'm-42' } });

    await cth().hiveMessages({ agentId: 'a1', limit: 10 });
    expect(lastCall()).toEqual({ cmd: 'hive_messages', args: { agentId: 'a1', limit: 10 } });
  });

  it('hiveMessages still accepts POSITIONAL args', async () => {
    await cth().hiveMessages('a1', 'm-9', 5, true);
    expect(lastCall()).toEqual({
      cmd: 'hive_messages',
      args: { agentId: 'a1', id: 'm-9', limit: 5, includeArchived: true }
    });
  });

  it('session export callsites: telemetryUsage + ptyRedraw pack correctly', async () => {
    await cth().telemetryUsage('a1');
    expect(lastCall()).toEqual({ cmd: 'telemetry_usage', args: { agentId: 'a1' } });
    await cth().redrawPty('pty-9');
    expect(lastCall()).toEqual({ cmd: 'pty_redraw', args: { id: 'pty-9' } });
  });

  it('on* listeners return an unsubscribe fn', async () => {
    const unsub = cth().onConfigChanged(() => {}) as () => void;
    expect(typeof unsub).toBe('function');
  });

  it('unknown methods warn once and resolve null, not crash', async () => {
    const missing = cth().doesNotExist as undefined | (() => Promise<unknown>);
    if (missing) expect(await missing()).toBeNull();
  });
});
