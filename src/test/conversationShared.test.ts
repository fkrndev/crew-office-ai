/**
 * The conversation view's pure logic (src/shared/conversation.ts): payload
 * normalization, incremental merge, the "is this the person's own text" rule
 * (kept in step with the Rust classifier), pending/echo bubbles, poll cadence.
 */
import { describe, it, expect } from 'vitest';
import {
  ECHO_TTL_MS, MAX_UI_MESSAGES, collapseTeamUpdates, echoesAfterQueueChange, humanQueued,
  isInternalPrompt, isWorkingStatus, mergeMessages, nextPollDelay, normalizeRead, pendingBubbles,
  reconcileEchoes, stripGoalBlock, textsMatch, type ConversationMessage
} from '@shared/conversation';
import { inboxNudgeText } from '@shared/hiveNudge';
import { initialGodPrompt } from '@/hooks/useHive';

const m = (id: string, role: ConversationMessage['role'], text: string, ts = 0): ConversationMessage => ({
  id, role, text, ts
});

describe('normalizeRead', () => {
  it('coerces a well-formed payload and drops junk rows', () => {
    const r = normalizeRead({
      messages: [
        { id: 'h1', role: 'user', text: 'hi', ts: 5 },
        { id: 'h2', role: 'assistant', text: '   ', ts: 6 },     // blank → dropped
        { id: 'h3', role: 'tool', text: 'x' },                   // unknown role → dropped
        { id: 7, role: 'user', text: 'x' },                      // bad id → dropped
        null,
        { id: 'h4', role: 'system', text: '', kind: 'team', ts: 'soon' }
      ],
      cursor: 'h:4', source: 'hermes', more: true
    })!;
    expect(r.messages.map((x) => x.id)).toEqual(['h1', 'h4']);
    expect(r.messages[1]).toMatchObject({ role: 'system', kind: 'team', ts: 0 });
    expect(r).toMatchObject({ cursor: 'h:4', source: 'hermes', more: true, reset: false });
  });

  it('a source of none always carries a reason; nonsense is null', () => {
    expect(normalizeRead({ source: 'none' })).toMatchObject({ source: 'none', reason: 'unreadable' });
    expect(normalizeRead({ source: 'none', reason: 'starting' })).toMatchObject({ reason: 'starting' });
    expect(normalizeRead({ source: 'weird', messages: 'nope' })).toMatchObject({ source: 'none', messages: [] });
    expect(normalizeRead(undefined)).toBeNull();
    expect(normalizeRead('x')).toBeNull();
  });
});

describe('mergeMessages / collapseTeamUpdates', () => {
  it('appends new rows, ignores repeats by id and by role+ts+text, replaces on reset', () => {
    const a = [m('1', 'user', 'hi', 1), m('2', 'assistant', 'yo', 2)];
    const merged = mergeMessages(a, [m('2', 'assistant', 'yo', 2), m('9', 'user', 'hi', 1), m('3', 'user', 'again', 3)], false);
    expect(merged.map((x) => x.id)).toEqual(['1', '2', '3']);
    expect(mergeMessages(a, [m('5', 'user', 'fresh', 5)], true).map((x) => x.id)).toEqual(['5']);
  });

  it('keeps a bounded window of the newest messages', () => {
    const many = Array.from({ length: MAX_UI_MESSAGES + 25 }, (_, i) => m(`i${i}`, 'user', `t${i}`, i));
    const out = mergeMessages([], many, false);
    expect(out).toHaveLength(MAX_UI_MESSAGES);
    expect(out[out.length - 1].id).toBe(`i${MAX_UI_MESSAGES + 24}`);
  });

  it('collapses consecutive team updates into one line', () => {
    const team = (id: string): ConversationMessage => ({ ...m(id, 'system', ''), kind: 'team' });
    const out = collapseTeamUpdates([team('a'), team('b'), m('c', 'assistant', 'ok'), team('d')]);
    expect(out.map((x) => x.id)).toEqual(['a', 'c', 'd']);
  });
});

describe('isInternalPrompt — the app-injected prompts are never the person\'s bubbles', () => {
  it('matches what the senders actually produce', () => {
    expect(isInternalPrompt(inboxNudgeText(['m1', 'm2']))).toBe(true);
    expect(isInternalPrompt(initialGodPrompt('Michael', 'non-technical'))).toBe(true);
    expect(isInternalPrompt(initialGodPrompt('Ada'))).toBe(true);
    expect(isInternalPrompt('WORK ORDER FROM HIVE\nMessage: x')).toBe(true);
    expect(isInternalPrompt('<goal>\nship\n</goal>\n\nWORK ORDER FROM HIVE')).toBe(true);
    expect(isInternalPrompt('You are "Michael" (god), an autonomous agent in a collaborating hive of Claude agents.\nYour private workspace is X')).toBe(true);
    expect(isInternalPrompt('You are right about that, thanks')).toBe(false);
    expect(isInternalPrompt('/compact')).toBe(true);
    expect(isInternalPrompt('  /clear')).toBe(true);
  });
  it('leaves human text alone', () => {
    for (const t of ['hello Michael', 'what does /compact do?', 'please read the inbox', 'تحدث معي', '你好']) {
      expect(isInternalPrompt(t)).toBe(false);
    }
    expect(stripGoalBlock('<goal>g</goal>\n\nreal')).toBe('real');
    expect(stripGoalBlock('plain')).toBe('plain');
  });
});

describe('pending + echo bubbles', () => {
  const q = (id: string, text: string, ts = 1000) => ({ id, text, ts });

  it('only the person\'s own queued text gets a pending bubble, in queue order', () => {
    const queue = [q('a', 'first'), q('n', inboxNudgeText(['m'])), q('b', 'second'), q('e', '   ')];
    expect(humanQueued(queue).map((x) => x.id)).toEqual(['a', 'b']);
    expect(pendingBubbles(queue, [])).toEqual([
      { id: 'queued-a', text: 'first', state: 'waiting' },
      { id: 'queued-b', text: 'second', state: 'waiting' }
    ]);
  });

  it('a message that leaves the queue is echoed as "sending" until the transcript shows it', () => {
    const prev = [q('a', 'please build the report'), q('b', 'and email it')];
    const echoes = echoesAfterQueueChange(prev, [q('b', 'and email it')], [], [], 5000);
    expect(echoes).toEqual([{ id: 'a', text: 'please build the report', queuedAt: 1000, sentAt: 5000 }]);
    expect(pendingBubbles([q('b', 'and email it')], echoes).map((p) => [p.id, p.state])).toEqual([
      ['echo-a', 'sending'], ['queued-b', 'waiting']
    ]);
    // idempotent
    expect(echoesAfterQueueChange(prev, [q('b', 'and email it')], echoes, [], 6000)).toHaveLength(1);
    // the transcript catches up with a NEW user row → echo gone
    expect(reconcileEchoes(echoes, [m('h9', 'user', 'Please build the report')], 6000)).toEqual([]);
    // an assistant row or an unrelated user row does not satisfy it
    expect(reconcileEchoes(echoes, [m('h9', 'assistant', 'please build the report'), m('h10', 'user', 'other')], 6000)).toHaveLength(1);
  });

  it('no echo when the transcript already showed it before the queue entry cleared', () => {
    const prev = [q('a', 'already there', 10_000)];
    const shown = [m('h1', 'user', 'already there', 10_500)];
    expect(echoesAfterQueueChange(prev, [], [], shown, 12_000)).toEqual([]);
    // but an OLD identical message does not hide a fresh send of the same words
    const old = [m('h0', 'user', 'already there', 1_000)];
    expect(echoesAfterQueueChange(prev, [], [], old, 12_000)).toHaveLength(1);
  });

  it('echoes expire', () => {
    const e = [{ id: 'a', text: 'x', queuedAt: 0, sentAt: 1000 }];
    expect(reconcileEchoes(e, [], 1000 + ECHO_TTL_MS)).toHaveLength(1);
    expect(reconcileEchoes(e, [], 1001 + ECHO_TTL_MS)).toHaveLength(0);
  });

  it('textsMatch tolerates whitespace/case/goal wrapper and attachment suffixes, not short substrings', () => {
    expect(textsMatch('Hello   there', 'hello there')).toBe(true);
    expect(textsMatch('<goal>g</goal>\n\nbuild it now please', 'build it now please')).toBe(true);
    expect(textsMatch('build the report', 'build the report\n\nAttached files:\n- C:/a.png (a.png)')).toBe(true);
    expect(textsMatch('ok', 'ok, thanks for everything you did')).toBe(false);
    expect(textsMatch('', 'x')).toBe(false);
  });
});

describe('poll cadence + working status', () => {
  it('is fast while working or when more is waiting, slow when nothing can change', () => {
    expect(nextPollDelay({ more: true, source: 'hermes', working: false })).toBeLessThan(100);
    expect(nextPollDelay({ more: false, source: 'hermes', working: true })).toBe(1200);
    expect(nextPollDelay({ more: false, source: 'claude', working: false })).toBe(2500);
    expect(nextPollDelay({ more: false, source: 'none', reason: 'unsupported_engine', working: true })).toBe(10_000);
    expect(nextPollDelay({ more: false, source: 'none', reason: 'starting', working: false })).toBe(1500);
  });
  it('working means working or thinking', () => {
    expect(isWorkingStatus('working')).toBe(true);
    expect(isWorkingStatus('thinking')).toBe(true);
    for (const s of ['idle', 'blocked', 'waiting', 'ghost', undefined]) expect(isWorkingStatus(s)).toBe(false);
  });
});
