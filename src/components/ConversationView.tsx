import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore, type Agent } from '@/store/store';
import { useResolvedGodName } from '@/hooks/useResolvedGodName';
import {
  collapseTeamUpdates, echoesAfterQueueChange, humanQueued, isWorkingStatus, mergeMessages,
  nextPollDelay, normalizeRead, pendingBubbles, reconcileEchoes,
  type ConversationMessage, type ConversationReason, type ConversationSource, type Echo, type QueuedLike
} from '@shared/conversation';
import {
  Bubble, CONVO_CSS, EmptyConversation, MessageRow, WorkingIndicator
} from './ConversationBubbles';
import { ChatShowsQueueContext, fillComposer } from './composerBridge';

/**
 * Simple-mode "talk" tab: a clean chat with the orchestrator instead of the raw
 * terminal. Data comes from `conversation_read` (the engine's own transcript, see
 * src-tauri/src/conversation.rs). When an engine has no structured transcript
 * yet — or never will — the raw terminal stays the primary view with a one-line
 * friendly note; an empty chat is never shown for an unreadable source.
 *
 * The terminal element is ALWAYS mounted (hidden while collapsed): its xterm
 * stream feeds the avatar-status parser (usePtyParser) and the queue-delivery
 * automation, which must keep running whether or not anyone looks at it.
 */

interface ConvoState {
  messages: ConversationMessage[];
  source: ConversationSource;
  reason?: ConversationReason;
  loaded: boolean;
}

const INITIAL: ConvoState = { messages: [], source: 'none', loaded: false };
const EMPTY_QUEUE: QueuedLike[] = [];
const NEAR_BOTTOM_PX = 48;

const prefersReducedMotion = (): boolean =>
  typeof window !== 'undefined' && !!window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;

/**
 * Poll `conversation_read` for one agent. Runs only while mounted (the talk tab
 * is on screen) and while the window is visible; incremental via the cursor.
 */
export function useConversation(agentId: string, working: boolean, queue: readonly QueuedLike[]) {
  const [state, setState] = useState<ConvoState>(INITIAL);
  const [echoes, setEchoes] = useState<Echo[]>([]);
  const cursorRef = useRef('');
  const workingRef = useRef(working);
  workingRef.current = working;
  const messagesRef = useRef<ConversationMessage[]>([]);
  messagesRef.current = state.messages;

  useEffect(() => {
    let alive = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let inflight = false;
    cursorRef.current = '';
    setState(INITIAL);
    setEchoes([]);

    const schedule = (ms: number) => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => { void tick(); }, ms);
    };

    const tick = async () => {
      if (!alive || inflight) return;
      // Hidden window: stop. The visibilitychange handler below restarts us.
      if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;
      inflight = true;
      let delay = 5_000;
      try {
        const raw = await window.cth.conversationRead?.(agentId, cursorRef.current || null);
        if (!alive) return;
        const r = normalizeRead(raw) ?? { messages: [], cursor: '', source: 'none' as const, reason: 'unreadable' as const };
        cursorRef.current = r.source === 'none' ? '' : r.cursor;
        const incomingUsers = r.reset ? [] : r.messages;
        setState((prev) => {
          const sourceChanged = prev.source !== r.source;
          if (
            prev.loaded && !sourceChanged && r.messages.length === 0 && !r.reset && prev.reason === r.reason
          ) return prev;
          return {
            messages: r.source === 'none' ? [] : mergeMessages(prev.messages, r.messages, r.reset === true || sourceChanged),
            source: r.source,
            reason: r.reason,
            loaded: true
          };
        });
        setEchoes((e) => (e.length ? reconcileEchoes(e, incomingUsers, Date.now()) : e));
        delay = nextPollDelay({ more: r.more === true, source: r.source, reason: r.reason, working: workingRef.current });
      } catch {
        delay = 5_000; // keep what we have; try again
      } finally {
        inflight = false;
      }
      if (alive) schedule(delay);
    };

    const onVisible = () => {
      if (document.visibilityState === 'visible') schedule(0);
    };
    document.addEventListener('visibilitychange', onVisible);
    void tick();
    return () => {
      alive = false;
      if (timer) clearTimeout(timer);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [agentId]);

  // A message leaving the composer queue is "delivered": echo it until the
  // transcript catches up (or the echo expires).
  const humanQueue = useMemo(() => humanQueued(queue), [queue]);
  const prevQueueRef = useRef<QueuedLike[]>(humanQueue);
  useEffect(() => {
    const prev = prevQueueRef.current;
    prevQueueRef.current = humanQueue;
    if (prev.length === 0) return;
    setEchoes((e) => echoesAfterQueueChange(prev, humanQueue, e, messagesRef.current, Date.now()));
  }, [humanQueue]);

  // Echoes expire on their own even if the transcript never says anything.
  useEffect(() => {
    if (echoes.length === 0) return;
    const t = setTimeout(() => setEchoes((e) => reconcileEchoes(e, [], Date.now())), 5_000);
    return () => clearTimeout(t);
  }, [echoes]);

  return { ...state, echoes, humanQueue };
}

export interface ConversationViewProps {
  agent: Agent;
  /** The raw terminal element — kept mounted (see file header). */
  terminal: ReactNode;
  /** The message composer, kept at the bottom exactly as in advanced mode. */
  composer: ReactNode;
}

export function ConversationView({ agent, terminal, composer }: ConversationViewProps) {
  const { t } = useTranslation();
  const godName = useResolvedGodName();
  const name = agent.isGod ? godName : (agent.name || godName);
  const queue = useStore((s) => s.messageQueues[agent.id]) ?? EMPTY_QUEUE;
  const removeQueuedMessage = useStore((s) => s.removeQueuedMessage);
  const working = isWorkingStatus(agent.status);
  const convo = useConversation(agent.id, working, queue);
  const [terminalOpen, setTerminalOpen] = useState(false);
  const paneId = useId();

  // No structured transcript → the terminal IS the view (never an empty chat).
  const fallback = convo.loaded && convo.source === 'none';
  const showTerminal = fallback || terminalOpen;

  const transcript = useMemo(() => collapseTeamUpdates(convo.messages), [convo.messages]);
  const pending = useMemo(() => pendingBubbles(convo.humanQueue, convo.echoes), [convo.humanQueue, convo.echoes]);
  const isEmpty = transcript.length === 0 && pending.length === 0;

  // ── scrolling: follow the newest message unless the person scrolled up ──
  const logRef = useRef<HTMLDivElement | null>(null);
  const stickRef = useRef(true);
  const [away, setAway] = useState(false);

  const scrollToBottom = useCallback((smooth: boolean) => {
    const el = logRef.current;
    if (!el) return;
    const top = el.scrollHeight;
    if (typeof el.scrollTo === 'function') {
      el.scrollTo({ top, behavior: smooth && !prefersReducedMotion() ? 'smooth' : 'auto' });
    } else {
      el.scrollTop = top;
    }
  }, []);

  const onScroll = useCallback(() => {
    const el = logRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < NEAR_BOTTOM_PX;
    stickRef.current = atBottom;
    setAway(!atBottom);
  }, []);

  useLayoutEffect(() => {
    // An empty chat (the welcome text + starter ideas) reads from the TOP; it must
    // never open scrolled to its bottom with the greeting cut off above.
    if (isEmpty) {
      if (logRef.current) logRef.current.scrollTop = 0;
      return;
    }
    if (stickRef.current) scrollToBottom(false);
  }, [transcript.length, pending.length, working, terminalOpen, fallback, isEmpty, convo.loaded, scrollToBottom]);

  const note = fallback
    ? convo.reason === 'starting'
      ? t('conversation.noteStarting', { name })
      : convo.reason === 'unsupported_engine'
        ? t('conversation.noteUnsupported', { name })
        : t('conversation.noteUnreadable')
    : null;

  return (
    <div
      data-testid="conversation-view"
      style={{ flex: 1, minHeight: 0, overflow: 'hidden', display: 'flex', flexDirection: 'column', background: 'var(--cth-cream-50)' }}
    >
      <style>{CONVO_CSS}</style>

      {note && (
        <div
          role="status"
          data-testid="convo-note"
          style={{
            flexShrink: 0, padding: '6px 12px', fontSize: 13, lineHeight: '18px',
            color: 'var(--cth-ink-700)', background: 'var(--cth-cream-100)',
            borderBottom: '1px solid var(--cth-ink-100)', fontFamily: 'var(--cth-font-ui)'
          }}
        >
          {note}
        </div>
      )}

      {/* The conversation — not rendered at all in fallback mode (there is no
          transcript to show, and an empty chat must never appear). */}
      {!fallback && (
      <div
        // minHeight keeps the chat readable when the live terminal is open in the narrow sidebar
        style={{ flex: '1 1 0', minHeight: 96, position: 'relative', display: 'flex', flexDirection: 'column' }}
      >
        <div
          ref={logRef}
          role="log"
          aria-live="polite"
          aria-relevant="additions"
          aria-label={t('conversation.regionLabel', { name })}
          tabIndex={0}
          className="cth-convo-log"
          onScroll={onScroll}
          data-testid="convo-log"
          style={{
            // Generous bottom padding: the last bubble's caption (time, "Waiting
            // to send") always has air under it and never sits at the very edge.
            flex: 1, minHeight: 0, overflowY: 'auto', display: 'flex', flexDirection: 'column',
            gap: 10, padding: '12px 12px 20px'
          }}
        >
          {!convo.loaded ? null : isEmpty ? (
            <EmptyConversation name={name} onPick={(text) => fillComposer(agent.id, text)} />
          ) : (
            <>
              {transcript.map((m) => <MessageRow key={m.id} m={m} name={name} />)}
              {pending.map((p) => (
                <Bubble
                  key={p.id}
                  role="user"
                  name={name}
                  text={p.text}
                  pending={p.state}
                  onCancel={p.queueId ? () => removeQueuedMessage(agent.id, p.queueId!) : undefined}
                />
              ))}
            </>
          )}
        </div>
        {working && <WorkingIndicator name={name} />}
        {/* "Jump to latest" sits in the flow under the list, NOT floating over it:
            a floating pill used to cover the last bubble's caption. */}
        {away && (
          <div style={{ flexShrink: 0, display: 'flex', justifyContent: 'center', padding: '4px 12px 6px' }}>
            <button
              type="button"
              className="cth-convo-btn"
              data-testid="convo-jump"
              onClick={() => { stickRef.current = true; setAway(false); scrollToBottom(true); }}
              style={{
                padding: '4px 12px', borderRadius: 16, cursor: 'pointer',
                border: '1px solid var(--cth-ink-300)', background: 'var(--cth-paper-100)',
                color: 'var(--cth-ink-900)', fontFamily: 'var(--cth-font-ui)', fontSize: 13
              }}
            >
              {t('conversation.jumpToLatest')}
            </button>
          </div>
        )}
      </div>
      )}

      {/* Terminal disclosure — collapsed by default; in fallback mode the terminal
          is simply the view, so there is nothing to disclose. */}
      {!fallback && (
        <button
          type="button"
          className="cth-convo-btn"
          data-testid="convo-terminal-toggle"
          aria-expanded={terminalOpen}
          aria-controls={paneId}
          onClick={() => setTerminalOpen((o) => !o)}
          style={{
            flexShrink: 0, display: 'flex', alignItems: 'center', gap: 6, width: '100%',
            padding: '5px 12px', border: 'none', borderTop: '1px solid var(--cth-ink-100)',
            background: 'var(--cth-cream-100)', color: 'var(--cth-ink-700)', cursor: 'pointer',
            fontFamily: 'var(--cth-font-ui)', fontSize: 12, textAlign: 'start'
          }}
        >
          <span aria-hidden="true" style={{ display: 'inline-block', width: 10 }}>{terminalOpen ? '▾' : '▸'}</span>
          {terminalOpen ? t('conversation.hideTerminal') : t('conversation.showTerminal')}
        </button>
      )}

      <div
        id={paneId}
        data-testid="convo-terminal-pane"
        style={{
          display: showTerminal ? 'flex' : 'none',
          flex: fallback ? '1 1 0' : '0 1 40%',
          minHeight: 0
        }}
      >
        {terminal}
      </div>

      {/* While the chat is on screen it already shows what is waiting to be sent,
          so the composer must not list the same messages again. */}
      <ChatShowsQueueContext.Provider value={!fallback}>{composer}</ChatShowsQueueContext.Provider>
    </div>
  );
}
