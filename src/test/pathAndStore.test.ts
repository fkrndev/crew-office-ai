/**
 * Windows-safe path helpers + the store behaviours changed alongside them:
 * focus mode left with Esc must not rewrite the stored preference (and must not
 * bounce back in), a non-user spawn must not steal the selection, feeds are
 * capped, and "open in IDE" must find the owning agent whatever the separators.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import {
  relativeTo, isAbsolutePath, joinPath, lastSegment, normalizeSeparators
} from '@shared/pathUtil';
import { stashRosterStorage, ROSTER_STORAGE_KEYS } from '@/store/rosterSource';
import { formatDateTime } from '@shared/dateFormat';
import { useStore, type Agent } from '@/store/store';

describe('pathUtil', () => {
  it('relativeTo ignores separator spelling and (on Windows) case', () => {
    expect(relativeTo('C:\\Proj\\app', 'c:/proj/app/src/a.ts', true)).toBe('src/a.ts');
    expect(relativeTo('C:\\Proj\\app\\', 'C:\\Proj\\app\\src\\a.ts', true)).toBe('src/a.ts');
    expect(relativeTo('C:\\Proj\\app', 'C:\\Proj\\app', true)).toBe('');
    // A sibling that merely shares the prefix is NOT inside it.
    expect(relativeTo('C:\\Proj\\app', 'C:\\Proj\\app2\\x.ts', true)).toBeNull();
    // POSIX stays case-sensitive.
    expect(relativeTo('/Proj/app', '/proj/app/x.ts', false)).toBeNull();
    expect(relativeTo('/proj/app', '/proj/app/x.ts', false)).toBe('x.ts');
  });

  it('isAbsolutePath recognises every spelling a path token can arrive in', () => {
    for (const p of ['/usr/x', '~/x', 'C:\\x', 'c:/x', '\\\\server\\share\\x']) {
      expect(isAbsolutePath(p)).toBe(true);
    }
    for (const p of ['src/a.ts', './a.ts', 'a\\b.ts']) expect(isAbsolutePath(p)).toBe(false);
  });

  it('joinPath keeps the base separator; lastSegment splits on both', () => {
    expect(joinPath('C:\\proj', 'src/a.ts')).toBe('C:\\proj\\src\\a.ts');
    expect(joinPath('C:\\proj\\', './a.ts')).toBe('C:\\proj\\a.ts');
    expect(joinPath('/home/u/p', './src/a.ts')).toBe('/home/u/p/src/a.ts');
    expect(lastSegment('C:\\Users\\me\\proj\\')).toBe('proj');
    expect(lastSegment('/a/b/cth-voice-tools')).toBe('cth-voice-tools');
    expect(normalizeSeparators('C:\\a\\b\\')).toBe('C:/a/b');
  });

  it('stashRosterStorage clears the roster keys and can undo', () => {
    const mem = new Map<string, string>();
    const storage = {
      getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
      setItem: (k: string, v: string) => { mem.set(k, v); },
      removeItem: (k: string) => { mem.delete(k); }
    };
    for (const k of ROSTER_STORAGE_KEYS) mem.set(k, 'x');
    mem.set('cth.language', 'id'); // unrelated preference must survive
    const undo = stashRosterStorage(storage);
    expect([...mem.keys()]).toEqual(['cth.language']);
    undo();
    expect(mem.size).toBe(ROSTER_STORAGE_KEYS.length + 1);
  });

  it('formatDateTime: invalid input renders nothing, valid honours the UI locale', () => {
    expect(formatDateTime('not a date', 'en')).toBe('');
    expect(formatDateTime('2026-01-02T03:04:05Z', 'en')).toBe(new Date('2026-01-02T03:04:05Z').toLocaleString('en'));
  });
});

function agent(id: string, over: Partial<Agent> = {}): Agent {
  return {
    id, name: id, character: 'michael', accent: 'blue', description: '', project: 'p',
    tmuxTarget: '', cwd: `C:\\work\\${id}`, status: 'idle', action: '', progress: 0,
    currentStation: 'desk', ptyId: `pty-${id}`, ...over
  } as unknown as Agent;
}

describe('store behaviours', () => {
  beforeEach(() => {
    window.localStorage.clear();
    useStore.setState({
      agents: [], archivedAgents: [], restorableAgents: [], selectedId: null,
      fullscreenAgentId: null, feeds: {}, prefersFocusMode: false
    });
  });

  it('pushFeed keeps only the most recent 200 lines per agent', () => {
    const { pushFeed } = useStore.getState();
    for (let i = 0; i < 250; i++) pushFeed('a1', `line ${i}`);
    const feed = useStore.getState().feeds.a1;
    expect(feed).toHaveLength(200);
    expect(feed[0]).toBe('line 50');
    expect(feed[199]).toBe('line 249');
  });

  it('addAgent selects a user-initiated hire but not a {select:false} spawn', () => {
    const s = useStore.getState();
    s.addAgent(agent('mine'));
    expect(useStore.getState().selectedId).toBe('mine');
    s.addAgent(agent('voice-hire'), { select: false });
    expect(useStore.getState().selectedId).toBe('mine');
    // With nothing valid selected, it still lands somewhere sensible.
    useStore.setState({ selectedId: null });
    s.addAgent(agent('another'), { select: false });
    expect(useStore.getState().selectedId).toBe('another');
  });

  it('Esc-exit of focus mode leaves the stored preference alone and does not bounce back', () => {
    const s = useStore.getState();
    s.addAgent(agent('a1'));
    s.setFullscreen('a1');
    expect(window.localStorage.getItem('cth.prefersFocusMode')).toBe('1');

    useStore.getState().leaveFocusForNow();
    expect(useStore.getState().fullscreenAgentId).toBeNull();
    expect(window.localStorage.getItem('cth.prefersFocusMode')).toBe('1');
    expect(useStore.getState().prefersFocusMode).toBe(true);

    // The roster changes constantly (status flips); that must not re-enter focus.
    useStore.getState().restoreFocusMode();
    expect(useStore.getState().fullscreenAgentId).toBeNull();

    // The explicit toggle still re-arms it and still persists.
    useStore.getState().setFullscreen('a1');
    expect(useStore.getState().fullscreenAgentId).toBe('a1');
    useStore.getState().setFullscreen(null);
    expect(window.localStorage.getItem('cth.prefersFocusMode')).toBe('0');
  });

  it('openFileInIde finds the owning agent across separators and case on Windows', () => {
    const s = useStore.getState();
    s.addAgent(agent('a1', { cwd: 'C:\\Work\\Proj' }));
    s.addAgent(agent('a2', { cwd: 'C:\\Work\\Proj2' }));
    useStore.getState().openFileInIde('c:/work/proj/src/a.ts');
    expect(useStore.getState().ideAgentId).toBe('a1');
    useStore.getState().openFileInIde('C:\\Work\\Proj2\\x.ts');
    expect(useStore.getState().ideAgentId).toBe('a2');
  });
});
