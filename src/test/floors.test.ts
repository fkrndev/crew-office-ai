/**
 * Multi-window floors, renderer side (src/floors.ts): window-label detection,
 * per-floor localStorage key namespacing (main's keys must stay byte-for-byte
 * so existing rosters load), the New Floor gating helpers, and the shim's
 * window-scoped PTY listeners.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

const invoke = vi.hoisted(() => vi.fn(async (_cmd: string, _args?: unknown) => null as unknown));
const listen = vi.hoisted(() =>
  vi.fn(async (_ch: string, _cb: unknown, _opts?: unknown) => () => {})
);
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen }));
vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({ onDragDropEvent: async () => () => {} })
}));

import {
  windowLabel, floorNumber, isFloorWindow, floorKey, listenTarget,
  newFloorEnabled, isNewFloorShortcut
} from '@/floors';
import { rosterStorageKeys, stashRosterStorage, ROSTER_STORAGE_KEYS } from '@/store/rosterSource';
import { installCth } from '@/cth';

type Internals = { metadata?: { currentWebview?: { label: string }; currentWindow?: { label: string } } };
function asWindow(label: string | null): void {
  const w = window as unknown as { __TAURI_INTERNALS__?: Internals };
  if (label === null) delete w.__TAURI_INTERNALS__;
  else w.__TAURI_INTERNALS__ = { metadata: { currentWebview: { label }, currentWindow: { label } } };
}

afterEach(() => asWindow(null));

describe('window label', () => {
  it('is main without Tauri, and reads the webview label under it', () => {
    asWindow(null);
    expect(windowLabel()).toBe('main');
    expect(isFloorWindow()).toBe(false);
    asWindow('floor-3');
    expect(windowLabel()).toBe('floor-3');
    expect(floorNumber()).toBe(3);
    expect(isFloorWindow()).toBe(true);
    expect(listenTarget()).toBe('floor-3');
  });

  it('only floor-<n>, n >= 2, is a floor (main is floor 1)', () => {
    expect(floorNumber('main')).toBeNull();
    expect(floorNumber('floor-1')).toBeNull();
    expect(floorNumber('floor-x')).toBeNull();
    expect(floorNumber('floor-')).toBeNull();
    expect(floorNumber('floor-12')).toBe(12);
  });
});

describe('per-floor storage keys', () => {
  it('main keeps every key unchanged (backward compatible)', () => {
    expect(floorKey('cth.agents', 'main')).toBe('cth.agents');
    expect(rosterStorageKeys('main')).toEqual([...ROSTER_STORAGE_KEYS]);
  });

  it('a floor gets its own copy of each key, distinct from main and from other floors', () => {
    const main = rosterStorageKeys('main');
    const f2 = rosterStorageKeys('floor-2');
    const f3 = rosterStorageKeys('floor-3');
    expect(f2).toEqual([
      'cth.agents@floor-2', 'cth.archivedAgents@floor-2', 'cth.restorableAgents@floor-2',
      'cth.selectedId@floor-2', 'cth.messageQueues@floor-2'
    ]);
    expect(new Set([...main, ...f2, ...f3]).size).toBe(15);
    // Still under the `cth.` prefix Settings → reset clears.
    expect(f2.every((k) => k.startsWith('cth.'))).toBe(true);
  });

  it('keys follow the current window when no label is passed', () => {
    asWindow('floor-4');
    expect(floorKey('cth.rosterHome')).toBe('cth.rosterHome@floor-4');
    expect(rosterStorageKeys()[0]).toBe('cth.agents@floor-4');
  });

  it('stashRosterStorage touches only the calling window\'s keys, and undo restores them', () => {
    const data: Record<string, string> = {
      'cth.agents': 'MAIN', 'cth.agents@floor-2': 'F2', 'cth.messageQueues@floor-2': 'Q2'
    };
    const storage = {
      getItem: (k: string) => (k in data ? data[k] : null),
      setItem: (k: string, v: string) => { data[k] = v; },
      removeItem: (k: string) => { delete data[k]; }
    };
    const undo = stashRosterStorage(storage, 'floor-2');
    expect(data).toEqual({ 'cth.agents': 'MAIN' });
    undo();
    expect(data['cth.agents@floor-2']).toBe('F2');
    expect(data['cth.messageQueues@floor-2']).toBe('Q2');
    expect(data['cth.agents']).toBe('MAIN');
  });
});

describe('roster isolation through the real store', () => {
  const agent = (id: string) => ({
    id, name: id, character: 'michael', accent: 'lemon', description: 'd', project: 'p',
    tmuxTarget: '', cwd: 'C:/w', status: 'idle', action: 'a', progress: 0,
    currentStation: 'desk', ptyId: `pty-${id}`, command: 'cmd'
  });

  beforeEach(() => {
    window.localStorage.clear();
    window.localStorage.setItem('cth.agents', JSON.stringify([agent('main-a')]));
    window.localStorage.setItem('cth.agents@floor-2', JSON.stringify([agent('floor-a')]));
    vi.resetModules();
  });

  it('main loads and writes the plain keys', async () => {
    asWindow('main');
    const { useStore } = await import('@/store/store');
    expect(useStore.getState().agents.map((a) => a.id)).toEqual(['main-a']);
    useStore.getState().select('main-a');
    expect(JSON.parse(window.localStorage.getItem('cth.agents')!).map((a: { id: string }) => a.id))
      .toEqual(['main-a']);
    expect(window.localStorage.getItem('cth.agents@floor-2')).toContain('floor-a');
  });

  it('a floor loads ITS roster and never reads or overwrites main\'s', async () => {
    asWindow('floor-2');
    const { useStore } = await import('@/store/store');
    expect(useStore.getState().agents.map((a) => a.id)).toEqual(['floor-a']);
    useStore.getState().updateAgent('floor-a', { name: 'renamed' });
    useStore.getState().select('floor-a');
    expect(JSON.parse(window.localStorage.getItem('cth.agents@floor-2')!)[0].name).toBe('renamed');
    expect(JSON.parse(window.localStorage.getItem('cth.agents')!)[0].id).toBe('main-a');
    expect(window.localStorage.getItem('cth.selectedId')).toBeNull();
    expect(window.localStorage.getItem('cth.selectedId@floor-2')).toBe('floor-a');
  });

  it('a fresh floor starts empty rather than adopting main\'s roster', async () => {
    asWindow('floor-3');
    const { useStore } = await import('@/store/store');
    expect(useStore.getState().agents).toEqual([]);
  });
});

describe('New Floor gating', () => {
  it('is available only when multiWindow === true (default OFF)', () => {
    expect(newFloorEnabled(null)).toBe(false);
    expect(newFloorEnabled(undefined)).toBe(false);
    expect(newFloorEnabled({})).toBe(false);
    expect(newFloorEnabled({ multiWindow: false })).toBe(false);
    expect(newFloorEnabled({ multiWindow: true })).toBe(true);
  });

  it('Ctrl/Cmd+Shift+N only', () => {
    const e = (o: Partial<Record<'key' | 'ctrlKey' | 'metaKey' | 'shiftKey' | 'altKey', unknown>>) =>
      ({ key: 'N', ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, ...o }) as KeyboardEvent;
    expect(isNewFloorShortcut(e({ ctrlKey: true, shiftKey: true }))).toBe(true);
    expect(isNewFloorShortcut(e({ metaKey: true, shiftKey: true, key: 'n' }))).toBe(true);
    expect(isNewFloorShortcut(e({ ctrlKey: true }))).toBe(false);
    expect(isNewFloorShortcut(e({ shiftKey: true }))).toBe(false);
    expect(isNewFloorShortcut(e({ ctrlKey: true, shiftKey: true, altKey: true }))).toBe(false);
    expect(isNewFloorShortcut(e({ ctrlKey: true, shiftKey: true, key: 'm' }))).toBe(false);
  });
});

describe('cth shim: window-scoped listeners', () => {
  beforeEach(() => { listen.mockClear(); invoke.mockClear(); });
  const cth = () => (window as unknown as { cth: Record<string, (...a: unknown[]) => unknown> }).cth;

  it('PTY streams and the floor-close request listen with THIS window as target', () => {
    asWindow('floor-2');
    installCth();
    cth().onPtyData('pty-x', () => {});
    cth().onPtyExit('pty-x', () => {});
    cth().onPtyRelaunch('pty-x', () => {});
    cth().onFloorCloseRequested(() => {});
    cth().onCloseRequested(() => {});
    const byChannel = new Map(listen.mock.calls.map(([ch, , opts]) => [ch, opts]));
    for (const ch of ['pty:data:pty-x', 'pty:exit:pty-x', 'pty:relaunch:pty-x', 'floor:closeRequested', 'app:closeRequested']) {
      expect(byChannel.get(ch)).toEqual({ target: 'floor-2' });
    }
  });

  it('main listens as main, and broadcast channels stay untargeted', () => {
    asWindow(null);
    installCth();
    cth().onPtyData('pty-y', () => {});
    cth().onConfigChanged(() => {});
    const byChannel = new Map(listen.mock.calls.map(([ch, , opts]) => [ch, opts]));
    expect(byChannel.get('pty:data:pty-y')).toEqual({ target: 'main' });
    expect(byChannel.get('config:changed')).toBeUndefined();
  });

  it('newFloor / closeFloor hit the window commands', async () => {
    installCth();
    await cth().newFloor();
    await cth().closeFloor();
    expect(invoke.mock.calls.map((c) => c[0])).toEqual(['window_new_floor', 'window_close_floor']);
  });
});
