import { useCallback, useEffect, useId, useRef, useState, type CSSProperties, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { PixelPanel } from './PixelPanel';
import { ErrorNotice } from './FriendlyError';
import { useEscape } from '@/hooks/useEscape';
import { stopEverything, type StopAllBridge } from '@shared/stopAll';

/**
 * Simple mode's "Stop everything": one clearly visible, not alarming button in
 * the title bar. It asks first, in plain words, then halts every agent (the
 * orchestrator and the workers) through the backend's control registry — nothing
 * is quit, killed or deleted — and says what happened. Pressing it again is
 * harmless (idempotent).
 *
 * a11y: a real <button> with an accessible name; the confirmation is an
 * `alertdialog` (aria-modal, labelled + described) that moves focus to the SAFE
 * choice ("No, keep going"), keeps Tab inside, closes on Esc through useEscape
 * (like every other modal here) and hands focus back to the button. The result is
 * announced through a polite status region.
 */

const NOTE_MS = 12_000;

const dialogBtn = (primary: boolean): CSSProperties => ({
  minHeight: 32, padding: '0 14px', cursor: 'pointer', border: 'none',
  fontFamily: 'var(--cth-font-ui)', fontSize: 14,
  background: primary ? 'var(--cth-coral)' : 'var(--cth-cream-100)',
  color: primary ? 'var(--cth-on-accent)' : 'var(--cth-ink-900)',
  boxShadow: `inset 0 0 0 1px ${primary ? 'var(--cth-ink-500)' : 'var(--cth-ink-300)'}, inset 0 -2px 0 ${primary ? 'var(--cth-ink-300)' : 'var(--cth-ink-100)'}`
});

type Note = { kind: 'done'; n: number } | { kind: 'error'; error: unknown };

export function StopEverything({ bridge, style }: { bridge?: StopAllBridge; style?: CSSProperties }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<Note | null>(null);
  const titleId = useId();
  const bodyId = useId();
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const keepRef = useRef<HTMLButtonElement | null>(null);
  const yesRef = useRef<HTMLButtonElement | null>(null);
  const noteTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => () => { if (noteTimer.current) clearTimeout(noteTimer.current); }, []);

  const close = useCallback((): void => {
    setOpen(false);
    // Hand focus back to what opened the dialog.
    queueMicrotask(() => triggerRef.current?.focus());
  }, []);
  // Esc = "keep going" (never while the request is in flight).
  useEscape(open && !busy ? close : null);

  // Focus the safe choice when the dialog opens.
  useEffect(() => { if (open) keepRef.current?.focus(); }, [open]);

  const confirm = async (): Promise<void> => {
    if (busy) return;
    setBusy(true);
    const r = await stopEverything(bridge ?? (window.cth as unknown as StopAllBridge));
    setBusy(false);
    setOpen(false);
    setNote(r.ok ? { kind: 'done', n: r.stopped } : { kind: 'error', error: r.error });
    if (noteTimer.current) clearTimeout(noteTimer.current);
    // An error stays until dismissed; the good news fades.
    if (r.ok) noteTimer.current = setTimeout(() => setNote(null), NOTE_MS);
    queueMicrotask(() => triggerRef.current?.focus());
  };

  // Keep Tab inside the two-button dialog.
  const trap = (e: ReactKeyboardEvent<HTMLDivElement>): void => {
    if (e.key !== 'Tab') return;
    const first = keepRef.current;
    const last = yesRef.current;
    if (!first || !last) return;
    const active = document.activeElement;
    if (e.shiftKey && active === first) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && active === last) { e.preventDefault(); first.focus(); }
  };

  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        className="cth-titlebar-nodrag"
        data-testid="stop-everything"
        onClick={() => setOpen(true)}
        title={t('stopAll.buttonTitle')}
        style={{
          display: 'inline-flex', alignItems: 'center', gap: 6, height: 28, padding: '0 10px',
          border: 'none', cursor: 'pointer', borderRadius: 2,
          background: 'var(--cth-paper-100)', color: 'var(--cth-ink-900)',
          boxShadow: 'inset 0 0 0 1px var(--cth-ink-300)',
          fontFamily: 'var(--cth-font-ui)', fontSize: 13, whiteSpace: 'nowrap',
          ...style
        }}
      >
        {/* A calm square (the universal "stop"), not a red alarm. */}
        <span aria-hidden="true" style={{
          width: 10, height: 10, background: 'var(--cth-coral)',
          boxShadow: 'inset 0 0 0 1px var(--cth-ink-500)'
        }} />
        {t('stopAll.button')}
      </button>

      <div
        role="status"
        aria-live="polite"
        data-testid="stop-everything-note"
        style={note ? {
          position: 'fixed', top: 44, insetInlineEnd: 12, zIndex: 900, maxWidth: 'min(420px, 86vw)',
          display: 'flex', alignItems: 'flex-start', gap: 10, padding: '8px 12px',
          background: 'var(--cth-mint-light)', boxShadow: 'inset 0 0 0 1.5px var(--cth-ink-500)',
          fontSize: 13, lineHeight: '19px', color: 'var(--cth-ink-900)'
        } : { position: 'absolute', width: 1, height: 1, overflow: 'hidden', clip: 'rect(0 0 0 0)' }}
      >
        {note?.kind === 'done' && (
          <>
            <span style={{ flex: 1 }}>{note.n > 0 ? t('stopAll.done') : t('stopAll.nothing')}</span>
            <button
              type="button"
              onClick={() => setNote(null)}
              aria-label={t('common.close')}
              style={{ border: 'none', background: 'transparent', cursor: 'pointer', color: 'var(--cth-ink-900)', fontSize: 14, lineHeight: '19px', padding: 0 }}
            >✕</button>
          </>
        )}
      </div>
      {note?.kind === 'error' && (
        <div style={{ position: 'fixed', top: 44, insetInlineEnd: 12, zIndex: 900, maxWidth: 'min(420px, 86vw)' }}>
          <ErrorNotice error={note.error} context="stop" friendly onRetry={() => { setNote(null); setOpen(true); }} testId="stop-everything-error" />
          <button
            type="button"
            onClick={() => setNote(null)}
            style={{ marginTop: 4, border: 'none', background: 'transparent', cursor: 'pointer', fontSize: 12, color: 'var(--cth-ink-700)', textDecoration: 'underline' }}
          >{t('common.close')}</button>
        </div>
      )}

      {open && createPortal(
        <div
          onClick={busy ? undefined : close}
          style={{
            position: 'fixed', inset: 0, background: 'rgba(15, 28, 46, 0.6)',
            display: 'flex', alignItems: 'center', justifyContent: 'center', zIndex: 950
          }}
        >
          <div
            role="alertdialog"
            aria-modal="true"
            aria-labelledby={titleId}
            aria-describedby={bodyId}
            data-testid="stop-everything-dialog"
            onClick={(e) => e.stopPropagation()}
            onKeyDown={trap}
            style={{ width: 440, maxWidth: '92vw' }}
          >
            <PixelPanel variant="dialog" noPadding>
              <div style={{ padding: 20, display: 'flex', flexDirection: 'column', gap: 14 }}>
                <div id={titleId} style={{
                  fontFamily: 'var(--cth-font-display)', fontSize: 12, lineHeight: '20px', color: 'var(--cth-ink-900)'
                }}>{t('stopAll.confirmTitle')}</div>
                <p id={bodyId} style={{ margin: 0, fontSize: 15, lineHeight: '22px', color: 'var(--cth-ink-700)' }}>
                  {t('stopAll.confirmBody')}
                </p>
                <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end', flexWrap: 'wrap' }}>
                  <button ref={keepRef} type="button" onClick={close} disabled={busy} style={dialogBtn(false)}>
                    {t('stopAll.confirmNo')}
                  </button>
                  <button ref={yesRef} type="button" onClick={() => { void confirm(); }} disabled={busy} style={dialogBtn(true)}>
                    {busy ? t('stopAll.stopping') : t('stopAll.confirmYes')}
                  </button>
                </div>
              </div>
            </PixelPanel>
          </div>
        </div>,
        document.body
      )}
    </>
  );
}
