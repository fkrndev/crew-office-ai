/**
 * First-run engine setup: the pure half (event reducer, step counter, failure
 * actions), the per-agent store fed by a FAKE event source, and the bridge
 * helpers (subscribe + replay, retry). No backend, no real keyring.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { fakeBridge } from './fakeBridge';
import {
  QUICK_START_GRACE_MS,
  failureAction,
  failureStem,
  listenEngineSetup,
  reduceSetupEvent,
  retryEngineSetup,
  setupStep,
  setupStepTextKey,
  shouldShowSetup,
  type EngineSetupEvent,
  type EngineSetupState
} from '@/shared/engineSetup';
import { setupBadge, startEngineSetupFeed, useEngineSetup } from '@/store/engineSetup';
import en from '@/i18n/locales/en.json';
import id from '@/i18n/locales/id.json';
import ar from '@/i18n/locales/ar.json';
import zh from '@/i18n/locales/zh-CN.json';

const ev = (over: Partial<EngineSetupEvent>): EngineSetupEvent => ({
  agentId: 'god', engine: 'hermes', phase: 'checking', detail: 'binary', ts: 1000, startedAt: 1000, ...over
});

function run(events: Array<Partial<EngineSetupEvent>>): EngineSetupState | undefined {
  let s: EngineSetupState | undefined;
  for (const e of events) s = reduceSetupEvent(s, ev(e));
  return s;
}

beforeEach(() => useEngineSetup.getState().reset());

describe('reduceSetupEvent', () => {
  it('walks the real first-run sequence and clears on ready', () => {
    let s = run([{ phase: 'checking' }]);
    expect(s).toMatchObject({ phase: 'checking', engine: 'hermes', startedAt: 1000 });
    s = reduceSetupEvent(s, ev({ phase: 'installing', detail: 'native', ts: 2000 }));
    expect(s).toMatchObject({ phase: 'installing', sawInstall: true, phaseSince: 2000, startedAt: 1000 });
    s = reduceSetupEvent(s, ev({ phase: 'installing', detail: 'python', ts: 2500, line: 'Downloading Python runtime' }));
    expect(s?.detail).toBe('python');
    expect(s?.phaseSince).toBe(2000); // same phase: the clock does not restart
    expect(s?.lines).toEqual(['Downloading Python runtime']);
    s = reduceSetupEvent(s, ev({ phase: 'starting', detail: 'relaunch', ts: 9000 }));
    s = reduceSetupEvent(s, ev({ phase: 'preparing', detail: 'preparing-deps', ts: 9500 }));
    expect(s).toMatchObject({ phase: 'preparing', sawInstall: true });
    expect(reduceSetupEvent(s, ev({ phase: 'ready', detail: 'prompt' }))).toBeUndefined();
    expect(reduceSetupEvent(s, ev({ phase: 'stopped' }))).toBeUndefined();
  });

  it('keeps a bounded, de-duplicated line history and ignores junk events', () => {
    let s = run([{ phase: 'installing' }]);
    for (let i = 0; i < 30; i++) s = reduceSetupEvent(s, ev({ phase: 'installing', line: `line ${i}` }));
    s = reduceSetupEvent(s, ev({ phase: 'installing', line: 'line 29' }));
    expect(s?.lines.length).toBe(12);
    expect(s?.lines[s.lines.length - 1]).toBe('line 29');
    expect(s?.lines.filter((l) => l === 'line 29').length).toBe(1);
    expect(reduceSetupEvent(s, { nope: true })).toBe(s);
    expect(reduceSetupEvent(s, null)).toBe(s);
    expect(reduceSetupEvent(s, ev({ phase: 'bogus' as never }))).toBe(s);
  });

  it('a failure keeps its reason and the phase it failed in, and a late line cannot un-fail it', () => {
    let s = run([{ phase: 'checking' }, { phase: 'installing' }]);
    s = reduceSetupEvent(s, ev({ phase: 'failed', reason: 'network', detail: 'curl: (6) Could not resolve host' }));
    expect(s).toMatchObject({ phase: 'failed', reason: 'network', failedIn: 'installing' });
    expect(s?.lines).toContain('curl: (6) Could not resolve host');
    expect(reduceSetupEvent(s, ev({ phase: 'installing', line: 'late' }))).toBe(s);
    // An unknown reason degrades to a generic one the UI can still act on.
    const odd = run([{ phase: 'failed', reason: 'weird' }]);
    expect(odd?.reason).toBe('engine_exit');
  });

  it('a new `checking` event starts a fresh attempt (Try again)', () => {
    const failed = run([{ phase: 'installing', line: 'old line' }, { phase: 'failed', reason: 'network' }]);
    const again = reduceSetupEvent(failed, ev({ phase: 'checking', ts: 50_000, startedAt: 50_000 }));
    expect(again).toMatchObject({ phase: 'checking', startedAt: 50_000, sawInstall: false, lines: [] });
    expect(again?.reason).toBeUndefined();
  });

  it('a snapshot replay seeds the line history', () => {
    const s = reduceSetupEvent(undefined, ev({ phase: 'preparing', lines: ['a b c', 'd e f'] }));
    expect(s?.lines).toEqual(['a b c', 'd e f']);
  });
});

describe('step counter', () => {
  const st = (phase: EngineSetupState['phase'], sawInstall: boolean, failedIn?: EngineSetupState['failedIn']) =>
    ({ phase, sawInstall, failedIn, detail: '', lines: [], agentId: 'a', engine: 'hermes', startedAt: 0, phaseSince: 0 }) as EngineSetupState;

  it('has three steps when an installer ran and two when the engine was already there', () => {
    expect(setupStep(st('checking', true))).toEqual({ current: 1, total: 3, key: 'checking' });
    expect(setupStep(st('installing', true))).toEqual({ current: 2, total: 3, key: 'installing' });
    expect(setupStep(st('starting', true))).toEqual({ current: 3, total: 3, key: 'starting' });
    expect(setupStep(st('preparing', true))).toEqual({ current: 3, total: 3, key: 'starting' });
    expect(setupStep(st('checking', false))).toEqual({ current: 1, total: 2, key: 'checking' });
    expect(setupStep(st('starting', false))).toEqual({ current: 2, total: 2, key: 'starting' });
    expect(setupStep(st('failed', true, 'installing'))).toEqual({ current: 2, total: 3, key: 'installing' });
    expect(setupStep(st('failed', false))).toEqual({ current: 2, total: 2, key: 'starting' });
  });

  it('names the step in plain words, refining installer sub-steps', () => {
    expect(setupStepTextKey({ ...st('installing', true), detail: 'native' })).toBe('firstRun.step.installing');
    expect(setupStepTextKey({ ...st('installing', true), detail: 'ffmpeg' })).toBe('firstRun.detail.ffmpeg');
    expect(setupStepTextKey(st('preparing', true))).toBe('firstRun.step.preparing');
    expect(setupStepTextKey(st('starting', true))).toBe('firstRun.step.starting');
    expect(setupStepTextKey(st('checking', true))).toBe('firstRun.step.checking');
  });

  it('every text key it can produce exists in English', () => {
    const get = (k: string) => k.split('.').reduce<unknown>((o, p) => (o as Record<string, unknown>)?.[p], en);
    for (const d of ['python', 'node', 'ripgrep', 'ffmpeg']) {
      expect(get(setupStepTextKey({ ...st('installing', true), detail: d }))).toBeTruthy();
    }
    for (const p of ['checking', 'starting', 'preparing', 'installing'] as const) {
      expect(get(setupStepTextKey(st(p, true)))).toBeTruthy();
    }
  });
});

describe('failures offer ONE clear action', () => {
  it('maps each reason to retry / add key / open settings', () => {
    expect(failureAction('no_key')).toBe('addKey');
    expect(failureAction('installer_unavailable')).toBe('openSettings');
    for (const r of ['network', 'installer_error', 'engine_missing', 'engine_exit', 'timeout', 'spawn_error'] as const) {
      expect(failureAction(r), r).toBe('retry');
    }
    expect(failureAction(undefined)).toBe('retry');
    expect(failureStem(undefined)).toBe('engine_exit');
  });

  it('every reason has a title and a body in English', () => {
    for (const r of ['network', 'no_key', 'installer_error', 'installer_unavailable', 'engine_missing', 'engine_exit', 'timeout', 'spawn_error'] as const) {
      const f = (en.firstRun.failure as Record<string, { title: string; body: string }>)[r];
      expect(f?.title, r).toBeTruthy();
      expect(f?.body, r).toBeTruthy();
    }
    expect(en.firstRun.failure.network.body).toMatch(/internet connection/i);
  });
});

describe('when the card shows', () => {
  it('a quick start stays invisible through its grace period; install / prepare / failure never wait', () => {
    const base = run([{ phase: 'starting', ts: 1000, startedAt: 1000 }])!;
    expect(shouldShowSetup(undefined, 5000)).toBe(false);
    expect(shouldShowSetup(base, 1000 + QUICK_START_GRACE_MS - 1)).toBe(false);
    expect(shouldShowSetup(base, 1000 + QUICK_START_GRACE_MS)).toBe(true);
    expect(shouldShowSetup(run([{ phase: 'installing' }]), 1001)).toBe(true);
    expect(shouldShowSetup(run([{ phase: 'preparing' }]), 1001)).toBe(true);
    expect(shouldShowSetup(run([{ phase: 'failed', reason: 'network' }]), 1001)).toBe(true);
  });
});

describe('the store, fed by a fake event source', () => {
  it('follows events per agent and clears on ready', () => {
    const bridge = fakeBridge();
    const off = startEngineSetupFeed(bridge as never);
    bridge.emit('onEngineSetup', ev({ agentId: 'god', phase: 'installing' }));
    bridge.emit('onEngineSetup', ev({ agentId: 'w1', phase: 'checking' }));
    expect(Object.keys(useEngineSetup.getState().byAgent).sort()).toEqual(['god', 'w1']);
    expect(setupBadge(useEngineSetup.getState().byAgent.god)).toEqual({ failed: false, current: 2, total: 3 });
    bridge.emit('onEngineSetup', ev({ agentId: 'god', phase: 'ready', detail: 'prompt' }));
    expect(useEngineSetup.getState().byAgent.god).toBeUndefined();
    expect(useEngineSetup.getState().byAgent.w1).toBeDefined();
    bridge.emit('onEngineSetup', ev({ agentId: 'w1', phase: 'failed', reason: 'no_key' }));
    expect(setupBadge(useEngineSetup.getState().byAgent.w1)).toMatchObject({ failed: true });
    expect(setupBadge(undefined)).toBeUndefined();
    off();
    bridge.emit('onEngineSetup', ev({ agentId: 'late', phase: 'installing' }));
    expect(useEngineSetup.getState().byAgent.late).toBeUndefined();
  });

  it('replays the backend snapshot (a window that loaded mid-install) and survives a missing/throwing one', async () => {
    const snap = [
      ev({ agentId: 'god', phase: 'preparing', detail: 'preparing-deps', lines: ['preparing dependencies'] }),
      ev({ agentId: 'w2', phase: 'failed', reason: 'network' })
    ];
    const bridge = fakeBridge({ engineSetupSnapshot: async () => snap });
    startEngineSetupFeed(bridge as never);
    await vi.waitFor(() => expect(useEngineSetup.getState().byAgent.god?.phase).toBe('preparing'));
    expect(useEngineSetup.getState().byAgent.god.lines).toEqual(['preparing dependencies']);
    expect(useEngineSetup.getState().byAgent.w2.reason).toBe('network');

    useEngineSetup.getState().reset();
    const off = listenEngineSetup({ engineSetupSnapshot: async () => { throw new Error('nope'); } }, () => {});
    await Promise.resolve();
    off();
    listenEngineSetup({}, () => {})(); // a bridge with neither method is fine
  });
});

describe('retryEngineSetup', () => {
  it('asks the backend to re-run the ladder for that agent', async () => {
    const engineSetupRetry = vi.fn(async () => true);
    expect(await retryEngineSetup({ engineSetupRetry }, 'god')).toEqual({ ok: true });
    expect(engineSetupRetry).toHaveBeenCalledExactlyOnceWith('god');
  });

  it('reports a refusal, and tells "no such agent" apart (the app then re-boots its orchestrator)', async () => {
    expect(await retryEngineSetup({ engineSetupRetry: async () => false }, 'g')).toMatchObject({ ok: false, noAgent: false });
    const r = await retryEngineSetup({ engineSetupRetry: async () => { throw 'no agent god'; } }, 'god');
    expect(r).toEqual({ ok: false, error: 'no agent god', noAgent: true });
    const r2 = await retryEngineSetup({ engineSetupRetry: async () => { throw new Error('boom'); } }, 'god');
    expect(r2).toMatchObject({ ok: false, error: 'boom', noAgent: false });
  });
});

describe('firstRun.* locale parity', () => {
  const flat = (o: unknown, p = ''): Record<string, string> =>
    Object.entries(o as Record<string, unknown>).reduce<Record<string, string>>((acc, [k, v]) => {
      const q = p ? `${p}.${k}` : k;
      if (v && typeof v === 'object') Object.assign(acc, flat(v, q));
      else acc[q] = String(v);
      return acc;
    }, {});
  const ph = (s: string) => [...s.matchAll(/\{\{\s*(\w+)\s*\}\}/g)].map((m) => m[1]).sort().join(',');
  it('identical key sets and placeholders in all four locales, every value filled', () => {
    const fe = flat(en.firstRun);
    expect(Object.keys(fe).length).toBeGreaterThan(50);
    for (const [name, loc] of Object.entries({ id, ar, zh })) {
      const f = flat((loc as typeof en).firstRun);
      expect(Object.keys(f).sort(), name).toEqual(Object.keys(fe).sort());
      for (const k of Object.keys(fe)) {
        expect(f[k], `${name}:${k}`).toBeTruthy();
        expect(ph(f[k]), `${name}:${k} placeholders`).toBe(ph(fe[k]));
      }
    }
  });
});
