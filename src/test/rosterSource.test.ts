/**
 * chooseRosterSource — the boot decision that decides whose floor you see.
 * Pins the malformed-file guard (a corrupt roster.json used to throw at
 * module load and blank the WHOLE app before onboarding rendered).
 */
import { describe, it, expect } from 'vitest';
import { chooseRosterSource } from '@/store/rosterSource';

const POPULATED = { agents: [{ id: 'a1' }], archived: [], restorable: [] };
const EMPTY = { agents: [], archived: [], restorable: [] };

describe('chooseRosterSource', () => {
  it('a populated file always wins', () => {
    expect(chooseRosterSource({
      fileRoster: POPULATED, currentHome: 'C:\\h1', storedHome: 'C:\\h2'
    })).toEqual({ useFileRoster: true, useLocalFallback: false });
  });

  it('a populated file wins even over matching localStorage', () => {
    const r = chooseRosterSource({
      fileRoster: POPULATED, currentHome: 'C:\\h1', storedHome: 'C:\\h1'
    });
    expect(r.useFileRoster).toBe(true);
  });

  it('localStorage is only read when it was written for THIS hive', () => {
    // same home → adopt
    expect(chooseRosterSource({
      fileRoster: EMPTY, currentHome: 'C:\\h1', storedHome: 'C:\\h1'
    }).useLocalFallback).toBe(true);
    // different home → don't import the other hive's team
    expect(chooseRosterSource({
      fileRoster: EMPTY, currentHome: 'C:\\h1', storedHome: 'C:\\h2'
    }).useLocalFallback).toBe(false);
    // pre-stamp localStorage (null) is adopted once
    expect(chooseRosterSource({
      fileRoster: EMPTY, currentHome: 'C:\\h1', storedHome: null
    }).useLocalFallback).toBe(true);
  });

  it('a MALFORMED roster file does not throw — falls through like an empty one', () => {
    // {} / missing arrays / wrong-typed fields — every shape that used to
    // crash fileHasRoster at module load.
    for (const bad of [
      {},
      { agents: null },
      { agents: 'not-an-array' },
    ]) {
      const r = chooseRosterSource({
        fileRoster: bad as never, currentHome: 'C:\\h1', storedHome: 'C:\\h1'
      });
      expect(r.useFileRoster).toBe(false);
      expect(r.useLocalFallback).toBe(true);
    }
    // …but a file carrying ANY populated slice still counts as a roster.
    expect(chooseRosterSource({
      fileRoster: { agents: [{ id: 'a' }] } as never, currentHome: 'h', storedHome: 'h'
    }).useFileRoster).toBe(true);
    expect(chooseRosterSource({
      fileRoster: { agents: [], archived: [{}] } as never, currentHome: 'h', storedHome: 'h'
    }).useFileRoster).toBe(true);
  });

  it('no file at all → same as an empty one', () => {
    expect(chooseRosterSource({
      fileRoster: null, currentHome: 'C:\\h1', storedHome: 'C:\\h1'
    })).toEqual({ useFileRoster: false, useLocalFallback: true });
  });
});
