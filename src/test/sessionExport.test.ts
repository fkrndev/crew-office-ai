/**
 * Session export composer — pins the Markdown shape the export button writes.
 * Pure function; runs in jsdom without a bridge.
 */
import { describe, it, expect } from 'vitest';
import { buildSessionExport, sessionExportFileName } from '@/shared/sessionExport';

const AGENT = {
  id: 'w-1',
  name: 'Ada',
  provider: 'claude',
  model: 'opus',
  cwd: 'D:\\repo',
  goal: 'fix tests'
};

describe('buildSessionExport', () => {
  it('includes identity, usage, mail, and terminal tail', () => {
    const md = buildSessionExport(
      AGENT,
      { input: 100, output: 50, cacheRead: 10, usd: 0.42 },
      [
        {
          from: 'god',
          to: 'w-1',
          act: 'task',
          subject: 'do the thing',
          body: 'details here',
          direction: 'inbox',
          created_at: '2026-01-01T00:00:00Z'
        }
      ],
      'last terminal output\nsecond line'
    );
    expect(md).toContain('# Session export — Ada');
    expect(md).toContain('`w-1`');
    expect(md).toContain('claude · opus');
    expect(md).toContain('fix tests');
    expect(md).toContain('Spend: $0.42');
    expect(md).toContain('do the thing');
    expect(md).toContain('details here');
    expect(md).toContain('last terminal output');
  });

  it('survives a dead agent: no usage, no mail, no tail', () => {
    const md = buildSessionExport(undefined, null, [], '');
    expect(md).toContain('# Session export — unknown-agent');
    expect(md).not.toContain('## Usage');
    expect(md).not.toContain('## Hive mail');
    expect(md).not.toContain('## Terminal tail');
  });

  it('mail renders oldest→newest (backend returns newest-first)', () => {
    const md = buildSessionExport(AGENT, null, [
      { subject: 'newest', created_at: '2026-01-02T00:00:00Z' },
      { subject: 'oldest', created_at: '2026-01-01T00:00:00Z' }
    ], '');
    expect(md.indexOf('oldest')).toBeLessThan(md.indexOf('newest'));
  });

  it('caps the terminal tail at 400 lines', () => {
    const tail = Array.from({ length: 600 }, (_, i) => `line-${i}`).join('\n');
    const md = buildSessionExport(AGENT, null, [], tail);
    expect(md).toContain('line-599');
    expect(md).not.toContain('line-100'); // lines beyond the cap are cut
  });
});

describe('sessionExportFileName', () => {
  it('is filesystem-safe and unique by timestamp', () => {
    const name = sessionExportFileName('My Agent! /weird', new Date('2026-01-02T03:04:05Z'));
    expect(name).toMatch(/^[\w.-]+-session-\d{4}-\d{2}-\d{2}-\d{2}-\d{2}\.md$/);
    expect(name).not.toMatch(/[\/\\?*:]/);
  });

  it('falls back to agent for an empty slug', () => {
    expect(sessionExportFileName('  ///  ', new Date(0))).toMatch(/^agent-session-/);
  });
});
