import { useId, useState, type CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import { PixelButton } from './PixelButton';
import { useIsSimpleMode, openAiConnection } from '@/hooks/useUiMode';
import { friendlyError, rawErrorText, type FriendlyContext } from '@shared/friendlyError';

/**
 * One place that shows an error. In simple mode (or when `friendly` says so — the
 * onboarding wizard is not in simple mode yet) it maps the raw text to a title, a
 * hint, at most one button, and keeps the original behind a small "Details"
 * disclosure; the main text is never a stack trace, a status line or JSON. In
 * advanced mode it shows `legacy ?? the raw text`, exactly what each call site
 * showed before, so nothing changes for people who want the real message.
 *
 * a11y: `role="alert"` on the box, a real button for the disclosure with
 * `aria-expanded` / `aria-controls`, and the raw text in a labelled region.
 */
export interface ErrorNoticeProps {
  /** The raw error: a string, an Error, `{error}` / `{message}`. */
  error: unknown;
  /** Advanced-mode text (default: the raw text itself). */
  legacy?: string;
  context?: FriendlyContext;
  /** Force friendly (true) / raw (false); default follows the UI mode. */
  friendly?: boolean;
  /** Offered as the "Try again" button when the kind suggests a retry. */
  onRetry?: () => void;
  /** `box` = the coral banner most forms use; `inline` = small text under a field. */
  layout?: 'box' | 'inline';
  style?: CSSProperties;
  testId?: string;
}

const boxStyle: CSSProperties = {
  padding: '6px 10px',
  background: 'var(--cth-coral-light)',
  boxShadow: 'inset 0 0 0 1px var(--cth-coral)',
  fontSize: 13,
  color: 'var(--cth-ink-900)',
  overflowWrap: 'anywhere'
};

export function ErrorNotice({
  error, legacy, context, friendly, onRetry, layout = 'box', style, testId
}: ErrorNoticeProps) {
  const { t } = useTranslation();
  const simple = useIsSimpleMode();
  const [open, setOpen] = useState(false);
  const detailsId = useId();
  const showFriendly = friendly ?? simple;
  const raw = rawErrorText(error);
  const inline = layout === 'inline';
  const wrap: CSSProperties = inline
    ? { fontSize: 12, lineHeight: '17px', color: 'var(--cth-ink-900)', overflowWrap: 'anywhere' }
    : boxStyle;

  if (!showFriendly) {
    return <div role="alert" data-testid={testId} style={{ ...wrap, ...style }}>{legacy ?? raw}</div>;
  }

  const fe = friendlyError(error, t as unknown as (k: string, o?: Record<string, unknown>) => string, {
    context, canRetry: !!onRetry
  });
  const act = fe.action;
  const onAction = (): void => {
    if (!act) return;
    if (act.id === 'retry') onRetry?.();
    else if (act.id === 'addKey') openAiConnection();
    else window.dispatchEvent(new CustomEvent('cth:open-settings', { detail: {} }));
  };

  return (
    <div
      role="alert"
      data-testid={testId ?? 'friendly-error'}
      data-kind={fe.kind}
      style={{ ...wrap, display: 'flex', flexDirection: 'column', gap: 4, ...style }}
    >
      <div style={{ fontWeight: 700, lineHeight: inline ? '17px' : '19px' }}>{fe.title}</div>
      <div style={{ lineHeight: inline ? '17px' : '19px', color: 'var(--cth-ink-700)' }}>{fe.hint}</div>
      {(act || fe.raw) && (
        <div style={{ display: 'flex', alignItems: 'center', gap: 10, flexWrap: 'wrap', marginTop: 2 }}>
          {act && (
            <PixelButton variant="primary" size="sm" onClick={onAction}>{act.label}</PixelButton>
          )}
          {fe.raw && (
            <button
              type="button"
              onClick={() => setOpen((o) => !o)}
              aria-expanded={open}
              aria-controls={detailsId}
              style={{
                background: 'none', border: 'none', padding: 0, cursor: 'pointer',
                fontFamily: 'var(--cth-font-ui)', fontSize: 12, color: 'var(--cth-ink-700)',
                textDecoration: 'underline'
              }}
            >
              {open ? t('errors.hideDetails') : t('errors.details')}
            </button>
          )}
        </div>
      )}
      {open && fe.raw && (
        <pre
          id={detailsId}
          role="region"
          aria-label={t('errors.detailsLabel')}
          tabIndex={0}
          style={{
            margin: 0, maxHeight: 110, overflow: 'auto', padding: '6px 8px', whiteSpace: 'pre-wrap',
            wordBreak: 'break-word', background: 'var(--cth-paper-100)',
            boxShadow: 'inset 0 0 0 1px var(--cth-ink-300)', fontFamily: 'var(--cth-font-mono)',
            fontSize: 11, lineHeight: '15px', color: 'var(--cth-ink-700)'
          }}
        >{fe.raw}</pre>
      )}
    </div>
  );
}
