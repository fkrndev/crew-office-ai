/**
 * Hive nudge strings — the wake-up text poked into a sleeping agent's
 * terminal when mail lands for it. isInboxNudge must recognise exactly what
 * inboxNudgeText produces (the terminal hook filters on it) — any drift
 * between them silently stops wake-ups.
 */
import { describe, it, expect } from 'vitest';
import { inboxNudgeText, isInboxNudge } from '@/shared/hiveNudge';

describe('inboxNudgeText / isInboxNudge', () => {
  it('round-trips: a generated nudge is recognised', () => {
    const t = inboxNudgeText(['msg1', 'msg2']);
    expect(isInboxNudge(t)).toBe(true);
  });

  it('does not recognise ordinary chat text', () => {
    expect(isInboxNudge('please review the PR')).toBe(false);
    expect(isInboxNudge('')).toBe(false);
  });
});
