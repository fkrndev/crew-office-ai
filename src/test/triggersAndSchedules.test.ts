/**
 * Triggers (isAutoAllowed/classifyInboundKind/validateAgainstSchema) +
 * weeklySchedule + agentTemplates — the inbound-work gate and the
 * scheduler/template pure logic.
 */
import { describe, it, expect } from 'vitest';
import {
  isAutoAllowed, classifyInboundKind, validateAgainstSchema,
  DEFAULT_ORG_TRIGGER, DEFAULT_CONTEXT_TRIGGER
} from '@/shared/triggers';
import {
  normalizeWeekly, formatWeekly, nextWeeklyFireMs, weeklyDelayMs,
  formatMinute, WEEKLY_CATCHUP_MS
} from '@/shared/weeklySchedule';
import { sanitizeTemplates } from '@/shared/agentTemplates';

describe('trigger gate', () => {
  it('strict blocks everything; communication-only blocks directives; allow-all blocks nothing', () => {
    expect(isAutoAllowed('strict', 'directive')).toBe(false);
    expect(isAutoAllowed('strict', 'communication')).toBe(false);
    expect(isAutoAllowed('communication-only', 'directive')).toBe(false);
    expect(isAutoAllowed('communication-only', 'communication')).toBe(true);
    expect(isAutoAllowed('allow-all', 'directive')).toBe(true);
  });

  it('classifyInboundKind errs toward directive — mis-labelled work is the dangerous case', () => {
    // Pure question, no imperative verbs → communication.
    expect(classifyInboundKind('how is the release going?')).toBe('communication');
    // "build" is an imperative token — even inside a question it stays
    // directive (deliberately conservative; senders can declare `kind`).
    expect(classifyInboundKind('what is the build status?')).toBe('directive');
    expect(classifyInboundKind('can you fix the flaky test?')).toBe('directive');
    expect(classifyInboundKind('deploy the release')).toBe('directive');
    expect(classifyInboundKind('')).toBe('communication');
  });

  it('defaults exist and are sane', () => {
    expect(DEFAULT_ORG_TRIGGER.mode).toBe('strict');
    expect(DEFAULT_CONTEXT_TRIGGER.compact.enabled).toBeDefined();
  });

  it('validateAgainstSchema returns violations for non-matching bodies', () => {
    const schema = { type: 'object', required: ['message'], properties: { message: { type: 'string' } } };
    expect(validateAgainstSchema({ message: 'hi' }, schema)).toEqual({ ok: true });
    expect(validateAgainstSchema({ nope: 1 }, schema).ok).toBe(false);
    expect(validateAgainstSchema({ message: 42 }, schema).ok).toBe(false); // wrong prop type
    expect(validateAgainstSchema({}, null).ok).toBe(true);                 // no schema = accept
    // An exotic/broken schema degrades to ACCEPT — never locks the caller out
    // of their own endpoint (documented upstream semantics).
    expect(validateAgainstSchema({}, { type: 'unicorn' }).ok).toBe(true);
  });
});

describe('weeklySchedule', () => {
  it('normalizeWeekly validates, sorts, dedupes', () => {
    expect(normalizeWeekly({ days: [3, 1, 3, 9, -1], minute: 540 }))
      .toEqual({ days: [1, 3], minute: 540 });
    expect(normalizeWeekly({ days: [], minute: 60 })).toBeNull();
    expect(normalizeWeekly({ days: [1], minute: 1440 })).toBeNull();
    expect(normalizeWeekly('junk')).toBeNull();
    expect(normalizeWeekly(null)).toBeNull();
  });

  it('formats labels: weekdays/weekends/every day get shortcuts', () => {
    expect(formatMinute(540)).toBe('09:00');
    expect(formatWeekly({ days: [1,2,3,4,5], minute: 540 })).toBe('weekdays at 09:00');
    expect(formatWeekly({ days: [0,6], minute: 0 })).toBe('weekends at 00:00');
    expect(formatWeekly({ days: [0,1,2,3,4,5,6], minute: 540 })).toBe('every day at 09:00');
    expect(formatWeekly({ days: [1,4], minute: 870 })).toBe('Mon, Thu at 14:30');
    expect(formatWeekly(null)).toBe('no days picked');
  });

  it('nextWeeklyFireMs finds the next matching local slot', () => {
    // Monday 2026-01-05 08:00 local → Monday 09:00 schedule fires same day.
    const mon8am = new Date(2026, 0, 5, 8, 0).getTime();
    const next = nextWeeklyFireMs({ days: [1], minute: 540 }, mon8am)!;
    expect(new Date(next).getHours()).toBe(9);
    expect(new Date(next).getDay()).toBe(1);
    // Past the slot today → next Monday.
    const mon10am = new Date(2026, 0, 5, 10, 0).getTime();
    const later = nextWeeklyFireMs({ days: [1], minute: 540 }, mon10am)!;
    expect(new Date(later).getDay()).toBe(1);
    expect(later).toBeGreaterThan(mon10am);
    expect(later - mon10am).toBeGreaterThan(6 * 86400000); // ~7 days out
  });

  it('weeklyDelayMs fires 0 for a recently-missed slot within catchup', () => {
    const mon10am = new Date(2026, 0, 5, 10, 0).getTime(); // missed 09:00 by an hour
    expect(weeklyDelayMs({ days: [1], minute: 540 }, mon10am, 0)).toBe(0);
    // Already ran it → don't re-fire on re-arm (the lastFiredAt clause).
    const slot = new Date(2026, 0, 5, 9, 0).getTime();
    expect(weeklyDelayMs({ days: [1], minute: 540 }, mon10am, slot)).toBeGreaterThan(0);
    // Missed by MORE than catchup → wait for next week's slot.
    const stale = mon10am + WEEKLY_CATCHUP_MS + 60_000;
    expect(weeklyDelayMs({ days: [1], minute: 540 }, stale, 0)).toBeGreaterThan(0);
  });
});

describe('agentTemplates.sanitizeTemplates', () => {
  it('keeps valid entries, drops the unspawnable', () => {
    const out = sanitizeTemplates([
      { id: 't1', label: 'Backend reviewer', provider: 'claude', command: 'claude --model opus' },
      { label: 'no command', provider: 'claude' },                 // command missing → drop
      { label: 'bad provider', provider: 'zzz', command: 'x' },    // unknown provider → drop
      'not-an-object',
      { id: 't2', label: '', provider: 'claude', command: 'x' },   // empty label → drop
    ]);
    expect(out).toHaveLength(1);
    expect(out[0].label).toBe('Backend reviewer');
    expect(out[0].command).toBe('claude --model opus'); // verbatim, not rebuilt
  });

  it('non-array input → empty list (config.json is user-editable)', () => {
    expect(sanitizeTemplates('junk')).toEqual([]);
    expect(sanitizeTemplates(null)).toEqual([]);
    expect(sanitizeTemplates(undefined)).toEqual([]);
  });
});
