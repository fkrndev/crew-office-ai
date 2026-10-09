/**
 * spawnWithShardxSetup — the needsShardx rescue on every spawn path.
 *
 * Backend flags a spawn that armed shardx (consent granted) while its setup
 * gates (vendored bridge + stored token) aren't met. The wrapper's contract:
 * ONE confirm → bootstrap → respawn armed; decline/failure keeps the original
 * spawn untouched.
 */
import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { installFakeBridge, type FakeBridge } from './fakeBridge';
import { spawnWithShardxSetup } from '@/lib/spawnWithShardxSetup';

let cth: FakeBridge;
const SPAWN = { id: 'pty-x', cwd: 'C:/repo' };

beforeEach(() => {
  cth = installFakeBridge();
});

afterEach(() => {
  vi.restoreAllMocks();
});

const callsOf = (method: string) => cth.calls.filter((c) => c.method === method);

describe('spawnWithShardxSetup', () => {
  it('no flag → plain pass-through, never asks', async () => {
    cth.spawnPty = vi.fn(async (a: unknown) => {
      cth.calls.push({ method: 'spawnPty', args: [a] });
      return { ok: true, cwd: 'C:/repo' };
    });
    const confirm = vi.spyOn(window, 'confirm');
    const res = await spawnWithShardxSetup(SPAWN);
    expect(res.ok).toBe(true);
    expect(confirm).not.toHaveBeenCalled();
    expect(callsOf('shardxBootstrap')).toHaveLength(0);
    expect(callsOf('spawnPty')).toHaveLength(1);
  });

  it('flag + accept → bootstrap, kill, respawn; returns the RESPAWN result', async () => {
    let n = 0;
    cth.spawnPty = vi.fn(async (a: unknown) => {
      n++;
      cth.calls.push({ method: 'spawnPty', args: [a] });
      // First spawn is armed-but-uninstalled; the re-spawn after bootstrap is clean.
      return n === 1 ? { ok: true, needsShardx: true } : { ok: true };
    });
    cth.shardxBootstrap = vi.fn(async () => {
      cth.calls.push({ method: 'shardxBootstrap', args: [] });
      return { ok: true };
    });
    cth.killPty = vi.fn(async (id: unknown) => {
      cth.calls.push({ method: 'killPty', args: [id] });
      return { ok: true };
    });
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    const res = await spawnWithShardxSetup(SPAWN);
    expect(res).toEqual({ ok: true });
    expect(callsOf('shardxBootstrap')).toHaveLength(1);
    // The unarmed first spawn is killed under the SAME pty id, then respawned.
    expect(callsOf('killPty')).toEqual([{ method: 'killPty', args: ['pty-x'] }]);
    expect(callsOf('spawnPty')).toHaveLength(2);
  });

  it('flag + decline → original spawn stands, no bootstrap/kill/respawn', async () => {
    cth.spawnPty = vi.fn(async (a: unknown) => {
      cth.calls.push({ method: 'spawnPty', args: [a] });
      return { ok: true, needsShardx: true };
    });
    vi.spyOn(window, 'confirm').mockReturnValue(false);

    const res = await spawnWithShardxSetup(SPAWN);
    expect(res.needsShardx).toBe(true);
    expect(callsOf('shardxBootstrap')).toHaveLength(0);
    expect(callsOf('killPty')).toHaveLength(0);
    expect(callsOf('spawnPty')).toHaveLength(1);
  });

  it('bootstrap fails → original spawn returned, no kill/respawn', async () => {
    cth.spawnPty = vi.fn(async (a: unknown) => {
      cth.calls.push({ method: 'spawnPty', args: [a] });
      return { ok: true, needsShardx: true };
    });
    cth.shardxBootstrap = vi.fn(async () => ({ ok: false }));
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    const res = await spawnWithShardxSetup(SPAWN);
    expect(res.needsShardx).toBe(true);
    expect(callsOf('killPty')).toHaveLength(0);
    expect(callsOf('spawnPty')).toHaveLength(1);
  });

  it('failed spawn never reaches the rescue', async () => {
    cth.spawnPty = vi.fn(async (a: unknown) => {
      cth.calls.push({ method: 'spawnPty', args: [a] });
      return { ok: false, error: 'cli missing' };
    });
    const confirm = vi.spyOn(window, 'confirm');
    const res = await spawnWithShardxSetup(SPAWN);
    expect(res.ok).toBe(false);
    expect(confirm).not.toHaveBeenCalled();
  });
});
