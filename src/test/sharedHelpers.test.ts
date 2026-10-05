/**
 * commandLine.tokenizeCommand — the shared argv splitter. Renderer and main
 * must split a spawn line identically or a worker boots with different args
 * than the UI showed (the exact drift the spawn-request fix killed).
 */
import { describe, it, expect } from 'vitest';
import { tokenizeCommand } from '@/shared/commandLine';
import { resolveGodName, DEFAULT_GOD_NAME } from '@/shared/godIdentity';
import { isComposingKey } from '@/shared/imeGuard';
import {
  enqueueHires, finishCurrentHire, clearHireQueue, hireQueueProgress, EMPTY_HIRE_QUEUE
} from '@/shared/hireQueue';
import type { HireManifest } from '@/shared/hire';

describe('tokenizeCommand', () => {
  it('splits on whitespace, keeps quoted runs whole', () => {
    expect(tokenizeCommand('claude --model opus')).toEqual(['claude', '--model', 'opus']);
    expect(tokenizeCommand('agy --model "Gemini 3.1 Pro (High)"'))
      .toEqual(['agy', '--model', 'Gemini 3.1 Pro (High)']);
    expect(tokenizeCommand("a 'b c' d")).toEqual(['a', 'b c', 'd']);
  });
  it('empty input → empty argv', () => {
    expect(tokenizeCommand('')).toEqual([]);
    expect(tokenizeCommand('   ')).toEqual([]);
  });
});

describe('resolveGodName', () => {
  it('honours a persisted rename — the registry wins over the default', () => {
    expect(resolveGodName('Ada')).toBe('Ada');
    expect(resolveGodName('  Ada  ')).toBe('Ada'); // trims
    expect(resolveGodName('')).toBe(DEFAULT_GOD_NAME);
    expect(resolveGodName(null)).toBe(DEFAULT_GOD_NAME);
    expect(resolveGodName(undefined)).toBe(DEFAULT_GOD_NAME);
  });
});

describe('isComposingKey', () => {
  it('an IME-composition Enter must not fire send', () => {
    expect(isComposingKey({ isComposing: true })).toBe(true);
    expect(isComposingKey({ keyCode: 229 })).toBe(true);            // legacy sentinel
    expect(isComposingKey({ nativeEvent: { isComposing: true } })).toBe(true); // React path
    expect(isComposingKey({ isComposing: false, keyCode: 13 })).toBe(false);   // real Enter
    expect(isComposingKey(null)).toBe(false);
  });
});

describe('hireQueue', () => {
  const m = (id: string) => ({ spec: 'munder-difflin/hire@1', id, name: id }) as unknown as HireManifest;

  it('enqueue appends; review progress tracks reviewed+pending', () => {
    let q = EMPTY_HIRE_QUEUE;
    q = enqueueHires(q, [m('a'), m('b')]);
    expect(q.pending).toHaveLength(2);
    expect(hireQueueProgress(q)).toEqual({ current: 1, total: 2 });
    q = finishCurrentHire(q);
    expect(q.reviewed).toBe(1);
    expect(hireQueueProgress(q)).toEqual({ current: 2, total: 2 });
    q = finishCurrentHire(q);
    expect(q).toBe(EMPTY_HIRE_QUEUE);
    expect(hireQueueProgress(q)).toBeNull();
  });

  it('mid-review arrivals do not reset progress; a drained queue does', () => {
    let q = enqueueHires(EMPTY_HIRE_QUEUE, [m('a'), m('b')]);
    q = finishCurrentHire(q);                    // reviewed 1
    q = enqueueHires(q, [m('c')]);               // +1 pending → total 3
    expect(hireQueueProgress(q)).toEqual({ current: 2, total: 3 });
    q = clearHireQueue(q);
    q = enqueueHires(q, [m('x')]);               // fresh run → reviewed resets
    expect(hireQueueProgress(q)).toEqual({ current: 1, total: 1 });
  });
});
