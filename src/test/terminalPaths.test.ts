/**
 * terminalPaths — the link/path detection that makes terminal output
 * clickable. Wrong matches = junk text opens the file picker; missed
 * matches = paths aren't clickable.
 */
import { describe, it, expect } from 'vitest';
import {
  stripPathToken, isPathToken, classifyPathToken,
  stripUrlToken, terminalLinkSpans
} from '@/shared/terminalPaths';

describe('stripUrlToken', () => {
  it('strips trailing punctuation the shell adds to URLs', () => {
    expect(stripUrlToken('https://x.com/a.')).toBe('https://x.com/a');
    expect(stripUrlToken('https://x.com/a,')).toBe('https://x.com/a');
    expect(stripUrlToken('https://x.com/a')).toBe('https://x.com/a');
  });
});

describe('path tokens', () => {
  it('strips wrapping punctuation and a trailing :line', () => {
    const t = stripPathToken('(C:\\repo\\file.ts:12)');
    expect(t).toBe('C:\\repo\\file.ts');
  });
  it('isPathToken recognises real path shapes only', () => {
    expect(isPathToken('src/foo.ts')).toBe(true);
    expect(isPathToken('./a/b.rs')).toBe(true);
    expect(isPathToken('word')).toBe(false);
    // A bare "name.43" prose fragment is dead text (no known ext, no sep).
    expect(isPathToken('electron.43')).toBe(false);
  });
  it('classifyPathToken: code → edit, markdown → preview, images → reveal', () => {
    expect(classifyPathToken('src/foo.ts')).toBe('edit');
    expect(classifyPathToken('notes.md')).toBe('preview');
    expect(classifyPathToken('shot.png')).toBe('reveal');
    expect(classifyPathToken('archive.bin')).toBe('reveal'); // unknown ext → reveal, never edit
  });
  it('terminalLinkSpans finds url spans in mixed text', () => {
    const spans = terminalLinkSpans('see https://x.com/a and C:\\f.txt for details');
    expect(spans.length).toBeGreaterThan(0);
    const urls = spans.filter((s) => s.kind === 'url');
    expect(urls[0]?.token).toContain('https://x.com/a');
  });
});
