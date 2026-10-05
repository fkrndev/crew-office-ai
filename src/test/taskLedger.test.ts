/**
 * taskLedger — the merge/patch rules that keep UI edits from deleting fields
 * the display model doesn't know about (upstream regression this exists for:
 * writeTasks used to re-serialize a normalized model and silently drop
 * `result`, `repo`, `commit`, `blockedOn`… on EVERY card).
 */
import { describe, it, expect } from 'vitest';
import { mergeTaskLedger, patchTaskInLedger } from '@/shared/taskLedger';

describe('mergeTaskLedger', () => {
  const disk = [
    { id: 't1', title: 'card', result: 'https://link', repo: 'r', notes: 'god wrote this' },
    { id: 't2', title: 'other' },
  ];

  it('keeps on-disk fields the incoming card does not mention', () => {
    const incoming = [{ id: 't1', title: 'card', col: 'done' }]; // UI's partial model
    const out = mergeTaskLedger(disk, incoming) as Record<string, unknown>[];
    expect(out[0].col).toBe('done');           // caller's field wins
    expect(out[0].result).toBe('https://link'); // untouched fields survive
    expect(out[0].repo).toBe('r');
    expect(out[0].notes).toBe('god wrote this');
  });

  it('deletion still works — absent from incoming means gone', () => {
    const out = mergeTaskLedger(disk, [{ id: 't1', title: 'card' }]);
    expect(out).toHaveLength(1);
  });

  it('an explicit null clears a field; a MISSING field keeps it', () => {
    const out = mergeTaskLedger(disk, [{ id: 't1', title: 'card', result: null }]) as Record<string, unknown>[];
    expect(out[0].result).toBeNull();
    expect(out[0].repo).toBe('r');
  });

  it('cards without an id pass through untouched (no key to merge on)', () => {
    const out = mergeTaskLedger(disk, [{ title: 'no id' }, { id: 't3' }]);
    expect(out).toEqual([{ title: 'no id' }, { id: 't3' }]);
  });
});

describe('patchTaskInLedger', () => {
  it('patches one card byte-for-byte, leaves every other entry alone', () => {
    const disk = [
      { id: 't1', col: 'todo', custom: 'keep' },
      { id: 't2', col: 'doing' },
    ];
    const out = patchTaskInLedger(disk, 't1', { col: 'done' }) as Record<string, unknown>[];
    expect(out[0]).toEqual({ id: 't1', col: 'done', custom: 'keep' });
    expect(out[1]).toEqual({ id: 't2', col: 'doing' });
  });
  it('non-array input yields an empty list, never a throw', () => {
    expect(patchTaskInLedger('junk', 't1', {})).toEqual([]);
  });
});
