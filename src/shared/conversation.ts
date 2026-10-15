/**
 * The conversation view's data model — pure logic only (no React, no IPC).
 *
 * `conversation_read` (src-tauri/src/conversation.rs) returns clean user /
 * assistant turns from an engine's own transcript. This module normalizes that
 * payload defensively, merges incremental batches, decides which queued
 * composer messages count as "the person's own words" (and so get a pending
 * bubble), and keeps a short-lived local echo for a message that has left the
 * queue but is not in the transcript yet.
 */

export type ConversationRole = 'user' | 'assistant' | 'system';

export interface ConversationMessage {
  id: string;
  role: ConversationRole;
  text: string;
  /** epoch ms, 0 when unknown */
  ts: number;
  /** only on `system` rows: 'team' = a background update from the team */
  kind?: 'team';
}

export type ConversationSource = 'hermes' | 'claude' | 'none';
export type ConversationReason = 'unsupported_engine' | 'starting' | 'unreadable';

export interface ConversationRead {
  messages: ConversationMessage[];
  cursor: string;
  source: ConversationSource;
  reason?: ConversationReason;
  /** the cursor no longer matched: REPLACE the list with `messages` */
  reset?: boolean;
  /** more already-written data is waiting: poll again at once */
  more?: boolean;
}

/** Most messages the view keeps in memory (oldest dropped). */
export const MAX_UI_MESSAGES = 500;
/** How long a delivered-but-not-yet-in-the-transcript message is echoed. */
export const ECHO_TTL_MS = 30_000;

const ROLES: readonly string[] = ['user', 'assistant', 'system'];

/** Validate/coerce whatever the bridge handed back. `null` = not a usable payload. */
export function normalizeRead(raw: unknown): ConversationRead | null {
  if (!raw || typeof raw !== 'object') return null;
  const r = raw as Record<string, unknown>;
  const source = r.source === 'hermes' || r.source === 'claude' ? r.source : 'none';
  const messages: ConversationMessage[] = [];
  if (Array.isArray(r.messages)) {
    for (const m of r.messages) {
      if (!m || typeof m !== 'object') continue;
      const o = m as Record<string, unknown>;
      if (typeof o.id !== 'string' || typeof o.role !== 'string' || !ROLES.includes(o.role)) continue;
      const text = typeof o.text === 'string' ? o.text : '';
      if (o.role !== 'system' && !text.trim()) continue;
      messages.push({
        id: o.id,
        role: o.role as ConversationRole,
        text,
        ts: typeof o.ts === 'number' && Number.isFinite(o.ts) ? o.ts : 0,
        ...(o.kind === 'team' ? { kind: 'team' as const } : {})
      });
    }
  }
  const reason = r.reason === 'unsupported_engine' || r.reason === 'starting' || r.reason === 'unreadable'
    ? r.reason
    : undefined;
  return {
    messages,
    cursor: typeof r.cursor === 'string' ? r.cursor : '',
    source,
    ...(source === 'none' ? { reason: reason ?? 'unreadable' } : {}),
    reset: r.reset === true,
    more: r.more === true
  };
}

const contentKey = (m: ConversationMessage) => `${m.role}\u0000${m.ts}\u0000${m.text}`;

/** Append an incremental batch (or replace on `reset`), dropping repeats by id and
 *  by (role, ts, text) — a compacted engine session re-writes the same turns. */
export function mergeMessages(
  prev: readonly ConversationMessage[],
  incoming: readonly ConversationMessage[],
  reset: boolean
): ConversationMessage[] {
  const base = reset ? [] : [...prev];
  const ids = new Set(base.map((m) => m.id));
  const keys = new Set(base.map(contentKey));
  for (const m of incoming) {
    if (ids.has(m.id)) continue;
    const k = contentKey(m);
    if (keys.has(k)) continue;
    ids.add(m.id);
    keys.add(k);
    base.push(m);
  }
  return base.length > MAX_UI_MESSAGES ? base.slice(base.length - MAX_UI_MESSAGES) : base;
}

/** Consecutive "update from the team" rows read as one muted line. */
export function collapseTeamUpdates(messages: readonly ConversationMessage[]): ConversationMessage[] {
  const out: ConversationMessage[] = [];
  for (const m of messages) {
    const last = out[out.length - 1];
    if (m.kind === 'team' && last?.kind === 'team') continue;
    out.push(m);
  }
  return out;
}

// ─── what counts as the person's own words ──────────────────────────────────

/** Drop a leading `<goal>…</goal>` block (useHive `withStandingGoal`). */
export function stripGoalBlock(text: string): string {
  const t = text.trimStart();
  if (t.startsWith('<goal>')) {
    const end = t.indexOf('</goal>');
    if (end >= 0) return t.slice(end + '</goal>'.length).trimStart();
  }
  return text;
}

/**
 * True for text the APP queues on its own (inbox nudges, work orders, the
 * orientation prompt, /compact) — never a pending bubble for the person.
 * Mirrors `classify_user_text` in src-tauri/src/conversation.rs; keep the two
 * identical (a Rust test pins the head strings against the TS senders).
 */
export function isInternalPrompt(text: string): boolean {
  const t = stripGoalBlock(text).trimStart();
  if (
    t.startsWith('You have new hive inbox message(s)') ||
    t.startsWith('WORK ORDER FROM HIVE') ||
    t.startsWith('Terminal handoff failed for ') ||
    t.startsWith('[System:')
  ) return true;
  if (t.startsWith("You're online as ") && t.includes('orchestrator of the hive')) return true;
  // the hive protocol prompt, typed into engines that cannot take it any other way
  if (t.startsWith('You are "') && t.includes('an autonomous agent in a collaborating hive')) return true;
  const first = t.split(/\s+/)[0] ?? '';
  return first === '/compact' || first === '/clear';
}

// ─── pending / echo bubbles ─────────────────────────────────────────────────

export interface QueuedLike {
  id: string;
  text: string;
  /** epoch ms the message was queued */
  ts: number;
}

export interface PendingBubble {
  id: string;
  text: string;
  /** 'waiting' = still in the composer queue; 'sending' = delivered, not in the transcript yet */
  state: 'waiting' | 'sending';
}

export interface Echo {
  id: string;
  text: string;
  /** when the queue item was queued (epoch ms) */
  queuedAt: number;
  /** when it left the queue (epoch ms) */
  sentAt: number;
}

const norm = (s: string) => stripGoalBlock(s).replace(/\s+/g, ' ').trim().toLowerCase();

/** Does a transcript user message correspond to this composer text? */
export function textsMatch(composerText: string, transcriptText: string): boolean {
  const a = norm(composerText);
  const b = norm(transcriptText);
  if (!a || !b) return false;
  if (a === b) return true;
  const [short, long] = a.length <= b.length ? [a, b] : [b, a];
  return short.length >= 12 && long.includes(short);
}

/** Queued messages worth a pending bubble: the person's own, in queue order. */
export function humanQueued(queue: readonly QueuedLike[]): QueuedLike[] {
  return queue.filter((q) => q.text.trim() && !isInternalPrompt(q.text));
}

/**
 * Turn "left the queue" into echoes. `prevIds` is what the previous render's
 * human queue held; anything now gone was delivered (or removed by the person —
 * the TTL ends that echo soon). A message the transcript already shows (it
 * arrived while the queue entry was still being cleared) gets no echo.
 */
export function echoesAfterQueueChange(
  prev: readonly QueuedLike[],
  nowQueue: readonly QueuedLike[],
  echoes: readonly Echo[],
  transcript: readonly ConversationMessage[],
  now: number
): Echo[] {
  const still = new Set(nowQueue.map((q) => q.id));
  const known = new Set(echoes.map((e) => e.id));
  const next = [...echoes];
  for (const q of prev) {
    if (still.has(q.id) || known.has(q.id)) continue;
    const alreadyShown = transcript.some(
      (m) => m.role === 'user' && m.ts >= q.ts - 2000 && textsMatch(q.text, m.text)
    );
    if (alreadyShown) continue;
    next.push({ id: q.id, text: q.text, queuedAt: q.ts, sentAt: now });
  }
  return next;
}

/** Drop echoes the transcript has caught up with (only messages that arrived in
 *  THIS batch count, so sending "hi" twice cannot be satisfied by the old "hi"),
 *  and echoes past their TTL. */
export function reconcileEchoes(
  echoes: readonly Echo[],
  newMessages: readonly ConversationMessage[],
  now: number
): Echo[] {
  const users = newMessages.filter((m) => m.role === 'user');
  const consumed = new Set<number>();
  const out: Echo[] = [];
  for (const e of echoes) {
    if (now - e.sentAt > ECHO_TTL_MS) continue;
    const i = users.findIndex((m, idx) => !consumed.has(idx) && textsMatch(e.text, m.text));
    if (i >= 0) {
      consumed.add(i);
      continue;
    }
    out.push(e);
  }
  return out;
}

export function pendingBubbles(queue: readonly QueuedLike[], echoes: readonly Echo[]): PendingBubble[] {
  return [
    ...echoes.map((e): PendingBubble => ({ id: `echo-${e.id}`, text: stripGoalBlock(e.text), state: 'sending' })),
    ...humanQueued(queue).map((q): PendingBubble => ({ id: `queued-${q.id}`, text: stripGoalBlock(q.text), state: 'waiting' }))
  ];
}

// ─── polling cadence ────────────────────────────────────────────────────────

export interface PollInputs {
  more: boolean;
  source: ConversationSource;
  reason?: ConversationReason;
  working: boolean;
}

/** Milliseconds until the next poll. Fast while there is more to read or the
 *  agent is working; slow when nothing can change (engine without a transcript). */
export function nextPollDelay(p: PollInputs): number {
  if (p.more) return 40;
  if (p.source === 'none') return p.reason === 'unsupported_engine' ? 10_000 : p.reason === 'starting' ? 1_500 : 5_000;
  return p.working ? 1_200 : 2_500;
}

/** Statuses that show the "working…" indicator. */
export function isWorkingStatus(status: string | undefined): boolean {
  return status === 'working' || status === 'thinking';
}
