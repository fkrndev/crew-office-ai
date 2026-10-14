/**
 * cth.ts adapters — the layer that turns what the Rust commands ACTUALLY return
 * (bare bools, arrays, rejections, bool-only Slack, positional vs object args)
 * into the shapes the UI was written against. Each case here is a call site that
 * misfired before: a Cancel that threw a TypeError, a "task changed" error on a
 * successful save, a Slack start with no port, a skills list that never loaded.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

const invoke = vi.hoisted(() =>
  vi.fn(async (_cmd: string, _args?: unknown) => null as unknown)
);
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({ onDragDropEvent: async () => () => {} })
}));

import { installCth, warmCth } from '@/cth';

type Cth = Record<string, (...a: unknown[]) => Promise<any> | any>;
const cth = (): Cth => (window as unknown as { cth: Cth }).cth;

/** Route invoke() by command name; anything unlisted resolves null. */
function backend(routes: Record<string, unknown | ((args: any) => unknown)>) {
  invoke.mockImplementation(async (cmd: string, args?: unknown) => {
    if (!(cmd in routes)) return null;
    const r = routes[cmd];
    if (typeof r === 'function') return (r as (a: unknown) => unknown)(args);
    return r;
  });
}
const lastOf = (cmd: string) => {
  const hits = invoke.mock.calls.filter(([c]) => c === cmd);
  return hits[hits.length - 1]?.[1];
};

describe('cth adapters', () => {
  beforeEach(() => {
    invoke.mockReset();
    backend({});
    installCth();
  });

  it('openTerminalAt: a bare bool becomes {ok}, a rejection becomes {ok:false,error}', async () => {
    backend({ terminal_open_at_folder: true });
    expect(await cth().openTerminalAt('C:\\proj')).toEqual({ ok: true });
    expect(lastOf('terminal_open_at_folder')).toEqual({ path: 'C:\\proj' });

    invoke.mockImplementationOnce(async () => { throw 'no supported terminal found'; });
    expect(await cth().openTerminalAt('C:\\proj')).toEqual({ ok: false, error: 'no supported terminal found' });
  });

  it('hivePatchTask / hiveDeleteTask: resolved card / remaining list read as success', async () => {
    const card = { id: 't1', status: 'done' };
    backend({ hive_patch_task: card, hive_delete_task: [card] });
    expect(await cth().hivePatchTask('t1', { status: 'done' })).toEqual({ ok: true, task: card });
    expect(lastOf('hive_patch_task')).toEqual({ id: 't1', patch: { status: 'done' } });
    expect(await cth().hiveDeleteTask('t1')).toEqual({ ok: true, tasks: [card] });

    invoke.mockImplementationOnce(async () => { throw 'no such task'; });
    expect(await cth().hivePatchTask('zz', {})).toEqual({ ok: false, error: 'no such task' });
  });

  it('textSearch wraps the bare hit list as {ok,results}', async () => {
    const hits = [{ source: 'board.md', excerpt: 'x' }];
    backend({ hive_text_search: hits });
    expect(await cth().textSearch('x')).toEqual({ ok: true, results: hits });
  });

  it('fs adapters speak the {ok,…} envelope the file tree / editor expect', async () => {
    backend({
      fs_list: [{ name: 'src', kind: 'dir', size: 0, mtime: 1 }, { name: 'a.ts', kind: 'file', size: 3, mtime: 2 }],
      fs_read: { kind: 'text', data: 'hello' },
      fs_write_file: { ok: true, path: 'C:\\w\\a.ts' }
    });
    const ls = await cth().listDir('C:\\w', '');
    expect(ls.ok).toBe(true);
    expect(ls.entries.map((e: any) => [e.name, e.isDir])).toEqual([['src', true], ['a.ts', false]]);

    const rd = await cth().readFile('C:\\w', 'src/a.ts');
    expect(rd).toEqual({ ok: true, content: 'hello', path: 'C:\\w\\src\\a.ts' });

    backend({ fs_read: { kind: 'image', mime: 'image/png', data: 'AA==' } });
    const img = await cth().readFile('C:\\w', 'p.png');
    expect(img.ok).toBe(false);

    invoke.mockImplementationOnce(async () => { throw 'path escapes the workspace'; });
    expect(await cth().writeFile('C:\\w', '..\\x', 'c')).toEqual({ ok: false, error: 'path escapes the workspace' });
  });

  it('readBinary(root, rel) joins confined and decodes base64 into bytes', async () => {
    backend({ fs_read_binary: { ok: true, base64: 'AQID', mime: 'image/png', size: 3, path: '/w/i.png' } });
    const r = await cth().readBinary('/w', 'sub/i.png');
    expect(lastOf('fs_read_binary')).toEqual({ path: '/w/sub/i.png' });
    expect(r.ok).toBe(true);
    expect(Array.from(r.bytes as Uint8Array)).toEqual([1, 2, 3]);
    // `..` can never leave the root.
    expect((await cth().readBinary('/w', '../etc/passwd')).ok).toBe(false);
  });

  it('git: checkout sends the real `ref` key (not `r#ref`) and wraps failures', async () => {
    backend({ git_checkout: { ok: true, detached: false } });
    expect(await cth().gitCheckout('/r', 'main', false)).toEqual({ ok: true, detached: false });
    expect(lastOf('git_checkout')).toEqual({ cwd: '/r', ref: 'main', detach: false });

    invoke.mockImplementationOnce(async () => { throw 'working tree has 2 uncommitted changes'; });
    expect(await cth().gitCheckout('/r', 'main', false)).toEqual({
      ok: false, error: 'working tree has 2 uncommitted changes'
    });
  });

  it('git reads hand panes {error} (their `"error" in r` check) instead of rejecting', async () => {
    invoke.mockImplementationOnce(async () => { throw 'not a git repository'; });
    expect(await cth().gitBranch('/x')).toEqual({ error: 'not a git repository' });
    invoke.mockImplementationOnce(async () => { throw 'not a git repository'; });
    expect(await cth().gitMainRepo('/x')).toBeNull();
  });

  it('gitDiff rebuilds both sides: HEAD blob + working-tree file', async () => {
    backend({
      git_show_file: { ok: true, exists: true, isBinary: false, content: 'old' },
      fs_read: { kind: 'text', data: 'new' }
    });
    expect(await cth().gitDiff('/r', 'a.ts')).toEqual({ ok: true, isBinary: false, head: 'old', working: 'new' });
    expect(lastOf('git_show_file')).toEqual({ cwd: '/r', rev: 'HEAD', path: 'a.ts' });
  });

  it('skills: a single cwd (or none) is sent as the Vec<String> the backend wants', async () => {
    await cth().skillsLocal('C:\\p');
    expect(lastOf('skills_local')).toEqual({ cwds: ['C:\\p'] });
    await cth().skillsLocal();
    expect(lastOf('skills_local')).toEqual({ cwds: [] });
    await cth().skillsUninstall('C:\\p\\.claude\\skills\\x', 'C:\\p');
    expect(lastOf('skills_uninstall')).toEqual({ path: 'C:\\p\\.claude\\skills\\x', cwds: ['C:\\p'] });
  });

  it('historyAdd accepts the object form (as hiveMessages does) and positional', async () => {
    await cth().historyAdd({ agentId: 'a1', cwd: 'C:\\p', text: 'ls' });
    expect(lastOf('command_history_add')).toEqual({ agentId: 'a1', cwd: 'C:\\p', text: 'ls' });
    await cth().historyAdd('a2', 'C:\\q', 'dir');
    expect(lastOf('command_history_add')).toEqual({ agentId: 'a2', cwd: 'C:\\q', text: 'dir' });
  });

  it('slackReply maps {channel,thread_ts,text} onto the threadTs argument', async () => {
    await cth().slackReply({ channel: 'C1', thread_ts: '1.2', text: 'hi' });
    expect(lastOf('slack_reply')).toEqual({ channel: 'C1', threadTs: '1.2', text: 'hi' });
  });

  it('slackStart passes the port, defaults it, and returns {ok,url} from slack_status', async () => {
    backend({ slack_start: true, slack_status: { running: true, url: 'https://t.example' } });
    expect(await cth().slackStart(4000, 'C9')).toEqual({ ok: true, url: 'https://t.example' });
    expect(lastOf('slack_start')).toEqual({ port: 4000, channelId: 'C9' });

    await cth().slackStart();
    expect(lastOf('slack_start')).toEqual({ port: 3847, channelId: null });

    backend({ slack_start: () => { throw 'SLACK_SIGNING_SECRET not in credential store'; } });
    expect(await cth().slackStart()).toEqual({
      ok: false, error: 'SLACK_SIGNING_SECRET not in credential store'
    });
  });

  it('freeflowTranscribe base64-encodes the audio and maps mimeType', async () => {
    backend({ freeflow_transcribe: { ok: true, text: 'hello' } });
    const audio = new Uint8Array([1, 2, 3]).buffer;
    const r = await cth().freeflowTranscribe({ audio, mimeType: 'audio/webm', filename: 'd.webm' });
    expect(r).toEqual({ ok: true, text: 'hello' });
    expect(lastOf('freeflow_transcribe')).toEqual({
      audioB64: 'AQID', mime: 'audio/webm', filename: 'd.webm', language: null
    });
  });

  it('saveClipboardImage forwards the data URL as base64 and envelopes the bool', async () => {
    backend({ clipboard_save_image: true });
    expect(await cth().saveClipboardImage('data:image/png;base64,AAAA')).toEqual({ ok: true });
    expect(lastOf('clipboard_save_image')).toEqual({ base64: 'data:image/png;base64,AAAA' });
    invoke.mockImplementationOnce(async () => { throw 'decode image: bad'; });
    expect(await cth().saveClipboardImage('x')).toEqual({ ok: false, error: 'decode image: bad' });
  });

  it('clipboardImageToFile forwards base64 and envelopes {ok,path}', async () => {
    backend({ clipboard_image_to_file: { ok: true, path: 'C:\\h\\p.png' } });
    expect(await cth().clipboardImageToFile('data:image/png;base64,AAAA')).toEqual({ ok: true, path: 'C:\\h\\p.png' });
    expect(lastOf('clipboard_image_to_file')).toEqual({ base64: 'data:image/png;base64,AAAA' });
    invoke.mockImplementationOnce(async () => { throw 'decode image: bad'; });
    expect(await cth().clipboardImageToFile('x')).toEqual({ ok: false, error: 'decode image: bad' });
  });

  it('importHireFiles folds the single-file answer into {ok,manifests,errors}', async () => {
    const manifest = { name: 'Dana' };
    backend({ hire_open_file: { ok: true, manifest, warnings: ['w'] } });
    expect(await cth().importHireFiles()).toEqual({ ok: true, manifests: [manifest], errors: [], warnings: ['w'] });

    // Cancel is not an error and must not throw on `.manifests.length`.
    backend({ hire_open_file: { ok: false, error: 'cancelled' } });
    expect(await cth().importHireFiles()).toEqual({ ok: false, manifests: [], errors: [], error: 'cancelled' });

    // Validation failures: one file, so one entry.
    backend({ hire_open_file: { ok: false, errors: ['"name" is required', 'bad spec'] } });
    const bad = await cth().importHireFiles();
    expect(bad.manifests).toEqual([]);
    expect(bad.errors).toEqual(['"name" is required; bad spec']);
  });

  it('hireFetch / drainPendingHires use the real command names; onHirePending exists', async () => {
    backend({ hire_fetch: { ok: true, manifest: { name: 'X' } }, hire_drain_pending: 'https://h.example/m.json' });
    expect((await cth().hireFetch('https://h.example/m.json')).ok).toBe(true);
    expect(lastOf('hire_fetch')).toEqual({ url: 'https://h.example/m.json' });
    expect(await cth().drainPendingHires()).toBe('https://h.example/m.json');
    expect(typeof (cth().onHirePending as unknown as (cb: () => void) => unknown)(() => {})).toBe('function');
  });

  it('forceQuit invokes app_force_quit; startClosingTime stays a bare number', async () => {
    backend({ app_force_quit: true, closing_time_start: 3 });
    expect(await cth().forceQuit()).toBe(true);
    expect(await cth().startClosingTime()).toBe(3);
  });

  it('updateOpenRelease() with no url opens the releases page instead of throwing', async () => {
    backend({ update_open_release: true });
    expect(await cth().updateOpenRelease()).toBe(true);
    expect((lastOf('update_open_release') as { url: string }).url).toMatch(/^https:\/\/github\.com\/.+\/releases/);
    await cth().updateOpenRelease('https://github.com/x/y/releases/tag/v1');
    expect(lastOf('update_open_release')).toEqual({ url: 'https://github.com/x/y/releases/tag/v1' });
    invoke.mockImplementationOnce(async () => { throw 'url is not a release page'; });
    expect(await cth().updateOpenRelease('https://evil.example')).toBe(false);
  });

  it("changeHome 'fresh' clears the old hive's roster keys; 'move' keeps them; failure restores", async () => {
    const keys = ['cth.agents', 'cth.archivedAgents', 'cth.restorableAgents', 'cth.selectedId', 'cth.messageQueues'];
    const seed = () => keys.forEach((k) => window.localStorage.setItem(k, '[1]'));
    const present = () => keys.filter((k) => window.localStorage.getItem(k) !== null);

    seed();
    backend({ config_change_home: null });
    await cth().changeHome('C:\\new', 'fresh');
    expect(present()).toEqual([]);

    seed();
    await cth().changeHome('C:\\new', 'move');
    expect(present()).toHaveLength(keys.length);

    // A rejected switch (relaunch never happened) must put the roster back.
    backend({ config_change_home: () => { throw 'invalid newHome'; } });
    await expect(cth().changeHome('C:\\bad', 'fresh')).rejects.toBe('invalid newHome');
    expect(present()).toHaveLength(keys.length);
    window.localStorage.clear();
  });

  it('sync reads come from the warmed caches (no sync XHR), harnessHomeSync included', async () => {
    const xhr = vi.spyOn(XMLHttpRequest.prototype, 'open');
    backend({
      app_clipboard_read: 'clip text',
      roster_read: { agents: [{ id: 'a' }] },
      config_home_sync: 'C:\\Users\\t\\HarnessAgents'
    });
    await warmCth();
    expect(cth().readClipboardSync()).toBe('clip text');
    expect(cth().rosterReadSync()).toEqual({ agents: [{ id: 'a' }] });
    expect(cth().harnessHomeSync()).toBe('C:\\Users\\t\\HarnessAgents');
    expect(xhr).not.toHaveBeenCalled();
    xhr.mockRestore();
  });
});
