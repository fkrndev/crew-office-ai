import { useTranslation } from 'react-i18next';
import type { CSSProperties } from 'react';
import { MarkdownPreview } from '@/markdown/MarkdownPreview';
import type { ConversationMessage, PendingBubble } from '@shared/conversation';

/**
 * Presentational pieces of the simple-mode conversation view. No data fetching
 * here — see ConversationView.tsx.
 *
 * Every bubble starts with a visually-hidden "<speaker> said:" so a screen
 * reader announces WHO is talking (including when a new bubble is read out by
 * the log's live region); the visible caption above it is hidden from assistive
 * tech so the speaker is not read twice.
 */

/** Visually hidden but available to assistive tech. */
export const CONVO_CSS = `
.cth-convo-sr { position: absolute; width: 1px; height: 1px; margin: -1px; padding: 0; overflow: hidden; clip: rect(0 0 0 0); clip-path: inset(50%); white-space: nowrap; border: 0; }
.cth-convo-bubble .cth-md-preview { font-size: inherit; line-height: inherit; }
.cth-convo-bubble .cth-md-preview pre { overflow-x: auto; max-width: 100%; }
.cth-convo-bubble .cth-md-preview table { display: block; overflow-x: auto; max-width: 100%; }
.cth-convo-bubble .cth-md-preview img { max-width: 100%; height: auto; }
.cth-convo-log:focus-visible, .cth-convo-btn:focus-visible { outline: 2px solid var(--cth-brand); outline-offset: 2px; }
.cth-convo-dots span { display: inline-block; width: 6px; height: 6px; border-radius: 50%; background: var(--cth-ink-500); margin-inline-end: 4px; animation: cth-convo-bounce 1.2s ease-in-out infinite; }
.cth-convo-dots span:nth-child(2) { animation-delay: 0.15s; }
.cth-convo-dots span:nth-child(3) { animation-delay: 0.3s; }
@keyframes cth-convo-bounce { 0%, 60%, 100% { transform: translateY(0); opacity: 0.5; } 30% { transform: translateY(-4px); opacity: 1; } }
@media (prefers-reduced-motion: reduce) {
  .cth-convo-dots span { animation: none !important; opacity: 0.8; }
}
`;

const rowBase: CSSProperties = { display: 'flex', flexDirection: 'column', maxWidth: '88%', minWidth: 0 };

const captionStyle: CSSProperties = {
  fontSize: 11, lineHeight: '14px', color: 'var(--cth-ink-500)', margin: '0 4px 2px'
};

function formatTime(ts: number, lang: string): string {
  if (!ts) return '';
  try {
    return new Date(ts).toLocaleTimeString(lang, { hour: 'numeric', minute: '2-digit' });
  } catch {
    return '';
  }
}

export interface BubbleProps {
  role: 'user' | 'assistant';
  /** assistant's display name (the orchestrator's resolved name) */
  name: string;
  text: string;
  ts?: number;
  /** a queued / sending message from the composer, shown greyed */
  pending?: PendingBubble['state'];
}

export function Bubble({ role, name, text, ts = 0, pending }: BubbleProps) {
  const { t, i18n } = useTranslation();
  const mine = role === 'user';
  const speaker = mine ? t('conversation.you') : name;
  const said = mine ? t('conversation.saidYou') : t('conversation.saidAssistant', { name });
  const time = formatTime(ts, i18n.language);
  const bubble: CSSProperties = mine
    ? pending
      ? {
          background: 'var(--cth-cream-200)', color: 'var(--cth-ink-700)',
          border: '1px dashed var(--cth-ink-300)'
        }
      : { background: 'var(--cth-brand)', color: 'var(--cth-on-accent)', border: '1px solid transparent' }
    : {
        background: 'var(--cth-paper-100)', color: 'var(--cth-ink-900)',
        border: '1px solid var(--cth-ink-100)'
      };
  return (
    <div
      data-testid={pending ? 'convo-pending' : `convo-${role}`}
      data-role={role}
      data-pending={pending ?? undefined}
      style={{ ...rowBase, alignSelf: mine ? 'flex-end' : 'flex-start' }}
    >
      <div aria-hidden="true" style={{ ...captionStyle, textAlign: mine ? 'end' : 'start' }}>
        {speaker}{time ? ` · ${time}` : ''}
      </div>
      <div
        className="cth-convo-bubble"
        style={{
          ...bubble, padding: '8px 12px', borderRadius: 12,
          borderStartEndRadius: mine ? 4 : 12, borderStartStartRadius: mine ? 12 : 4,
          fontFamily: 'var(--cth-font-ui)', fontSize: 14, lineHeight: '20px',
          overflowWrap: 'anywhere', minWidth: 0
        }}
      >
        <span className="cth-convo-sr">{said} </span>
        {mine ? (
          <span style={{ whiteSpace: 'pre-wrap' }}>{text}</span>
        ) : (
          <MarkdownPreview source={text} variant="card" />
        )}
      </div>
      {pending && (
        <div style={{ ...captionStyle, textAlign: 'end', fontStyle: 'italic' }}>
          {pending === 'sending' ? t('conversation.pendingSending') : t('conversation.pendingWaiting')}
        </div>
      )}
    </div>
  );
}

export function TeamUpdateLine({ name }: { name: string }) {
  const { t } = useTranslation();
  return (
    <div
      data-testid="convo-team"
      style={{
        alignSelf: 'center', fontSize: 12, lineHeight: '16px', color: 'var(--cth-ink-500)',
        padding: '2px 10px', borderRadius: 10, background: 'var(--cth-cream-100)'
      }}
    >
      {t('conversation.teamUpdate', { name })}
    </div>
  );
}

export function WorkingIndicator({ name }: { name: string }) {
  const { t } = useTranslation();
  return (
    <div
      role="status"
      data-testid="convo-working"
      style={{
        display: 'flex', alignItems: 'center', gap: 8, padding: '4px 14px 6px',
        fontSize: 13, color: 'var(--cth-ink-700)', fontFamily: 'var(--cth-font-ui)'
      }}
    >
      <span className="cth-convo-dots" aria-hidden="true"><span /><span /><span /></span>
      <span>{t('conversation.working', { name })}</span>
    </div>
  );
}

export function EmptyConversation({ name }: { name: string }) {
  const { t } = useTranslation();
  return (
    <div
      data-testid="convo-empty"
      style={{
        margin: 'auto', maxWidth: 360, textAlign: 'center', padding: 16,
        fontFamily: 'var(--cth-font-ui)', color: 'var(--cth-ink-700)'
      }}
    >
      <div style={{ fontSize: 16, lineHeight: '22px', fontWeight: 600, color: 'var(--cth-ink-900)', marginBottom: 6 }}>
        {t('conversation.emptyTitle', { name })}
      </div>
      <div style={{ fontSize: 14, lineHeight: '20px' }}>{t('conversation.emptyBody', { name })}</div>
    </div>
  );
}

/** Render a transcript message (user / assistant / team update). */
export function MessageRow({ m, name }: { m: ConversationMessage; name: string }) {
  if (m.role === 'system') return m.kind === 'team' ? <TeamUpdateLine name={name} /> : null;
  return <Bubble role={m.role} name={name} text={m.text} ts={m.ts} />;
}
