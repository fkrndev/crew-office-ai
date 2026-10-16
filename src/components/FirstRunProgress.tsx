import { useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { PixelPanel } from './PixelPanel';
import { PixelButton } from './PixelButton';
import { ErrorNotice } from './FriendlyError';
import { useEngineSetup } from '@/store/engineSetup';
import { useNow } from '@/hooks/useNow';
import { useIsSimpleMode, openAiConnection } from '@/hooks/useUiMode';
import {
  LONG_WAIT_MS,
  failureAction,
  failureStem,
  retryEngineSetup,
  setupStep,
  setupStepTextKey,
  shouldShowSetup,
  type EngineSetupBridge,
  type EngineSetupState
} from '@shared/engineSetup';

/** Scoped styles: the indeterminate bar. Reduced motion gets a still bar — the
 *  global reduced-motion rule zeroes animation durations, which would freeze a
 *  sliding block at its end frame (empty bar), so it is replaced outright. */
const CSS = `
@keyframes cth-frp-slide { 0% { left: -35%; } 100% { left: 100%; } }
.cth-frp-fill { position: absolute; top: 0; bottom: 0; width: 35%; background: var(--cth-brand);
  animation: cth-frp-slide 1.8s steps(14, end) infinite; }
@media (prefers-reduced-motion: reduce) {
  .cth-frp-fill { animation: none !important; left: 0 !important; width: 100% !important;
    background: repeating-linear-gradient(45deg, var(--cth-brand) 0 6px, transparent 6px 12px) !important; opacity: .6; }
}
.cth-frp-dot { display: inline-block; width: 8px; height: 8px; background: var(--cth-brand); flex-shrink: 0;
  animation: cth-blink 1s steps(1, end) infinite; }
@media (prefers-reduced-motion: reduce) { .cth-frp-dot { animation: none !important; } }
`;

export interface FirstRunProgressProps {
  agentId: string;
  /** `full` = the main card (simple mode); `compact` = one line (advanced).
   *  Default: by the UI mode. */
  variant?: 'full' | 'compact';
  /** Test seam: where Try again goes. Default: the real bridge. */
  bridge?: EngineSetupBridge;
  /** Test seam: "no such agent" fallback (the app re-boots its orchestrator). */
  reload?: () => void;
}

/**
 * The explanation for the minutes after a first install: instead of a raw
 * terminal with scrolling text, say what is happening, how far along it is,
 * that it is normal for it to take a while — and, when something fails, ONE
 * clear thing to do about it. Renders nothing unless a setup is active for
 * `agentId` (and, for a quick start, not before the grace period).
 *
 * a11y: `role="status"` + polite live region carries the title, the step and the
 * hint (never the log lines); the bar is a progressbar with `aria-busy` and a
 * text alternative; it stays still under prefers-reduced-motion.
 */
export function FirstRunProgress({ agentId, variant, bridge, reload }: FirstRunProgressProps) {
  const { t } = useTranslation();
  const simple = useIsSimpleMode();
  const state = useEngineSetup((s) => s.byAgent[agentId]);
  const now = useNow(!!state);
  const [open, setOpen] = useState(false);
  const [retrying, setRetrying] = useState(false);
  const [retryError, setRetryError] = useState<string | undefined>();
  const detailsId = useId();
  // A new attempt (the backend restarts at `checking`) ends the "trying again" state.
  const attempt = state?.startedAt;
  useEffect(() => { setRetrying(false); setRetryError(undefined); }, [attempt]);
  useEffect(() => { if (state?.phase !== 'failed') setRetryError(undefined); }, [state?.phase]);

  if (!state || !shouldShowSetup(state, now)) return null;
  const compact = (variant ?? (simple ? 'full' : 'compact')) === 'compact';
  const failed = state.phase === 'failed';
  const step = setupStep(state);
  const stepText = t(setupStepTextKey(state));
  const counter = t('firstRun.stepCounter', { current: step.current, total: step.total });

  const retry = async () => {
    if (retrying) return;
    setRetrying(true);
    setRetryError(undefined);
    const r = await retryEngineSetup(bridge ?? (window.cth as unknown as EngineSetupBridge), agentId);
    if (r.ok) return; // the backend's next `checking` event clears `retrying`
    setRetrying(false);
    // The backend no longer has this agent (its first spawn never registered):
    // the app's own boot re-runs the whole setup.
    if (r.noAgent) { (reload ?? (() => window.location.reload()))(); return; }
    setRetryError(r.error);
  };
  const act = failed ? failureAction(state.reason) : undefined;
  const actionLabel =
    act === 'addKey' ? t('firstRun.action.addKey')
    : act === 'openSettings' ? t('firstRun.action.openSettings')
    : retrying ? t('firstRun.action.retrying') : t('firstRun.action.retry');
  const onAction = act === 'retry' ? () => { void retry(); } : () => openAiConnection();
  const stem = failureStem(state.reason);

  if (compact) {
    return (
      <>
      <style>{CSS}</style>
      <div
        role="status"
        aria-live="polite"
        aria-busy={!failed}
        data-testid="first-run-progress"
        data-variant="compact"
        data-phase={state.phase}
        style={{
          display: 'flex', alignItems: 'center', gap: 8, minWidth: 0,
          marginBottom: 8, flexShrink: 0,
          padding: '4px 10px', fontSize: 12, lineHeight: '18px',
          background: failed ? 'var(--cth-coral-light)' : 'var(--cth-lemon-light)',
          boxShadow: 'inset 0 0 0 1px var(--cth-ink-300)', color: 'var(--cth-ink-900)'
        }}
      >
        {!failed && <span className="cth-frp-dot" aria-hidden="true" />}
        <span style={{ flex: 1, minWidth: 0, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}
          title={failed ? t(`firstRun.failure.${stem}.body`) : t('firstRun.hint')}>
          {failed
            ? t('firstRun.compact.failed', { engine: state.engine, reason: t(`firstRun.failure.${stem}.title`) })
            : t('firstRun.compact.working', { engine: state.engine, step: stepText, current: step.current, total: step.total })}
        </span>
        {failed && act && (
          <PixelButton variant="primary" size="sm" onClick={onAction} disabled={act === 'retry' && retrying}>
            {actionLabel}
          </PixelButton>
        )}
      </div>
      </>
    );
  }

  const longWait = !failed && now - state.startedAt >= LONG_WAIT_MS;
  return (
    <>
    <style>{CSS}</style>
    <div data-testid="first-run-progress" data-variant="full" data-phase={state.phase} style={{ marginBottom: 8, flexShrink: 0 }}>
      <PixelPanel variant="dialog" title={failed ? t('firstRun.titleFailed') : t('firstRun.title')} noPadding>
        <div
          role="status"
          aria-live="polite"
          aria-busy={!failed}
          style={{ padding: 16, display: 'flex', flexDirection: 'column', gap: 10 }}
        >
          {failed ? (
            <>
              <div style={{ fontSize: 14, lineHeight: '20px', fontWeight: 700, color: 'var(--cth-ink-900)' }}>
                {t(`firstRun.failure.${stem}.title`)}
              </div>
              <p style={{ margin: 0, fontSize: 13, lineHeight: '20px', color: 'var(--cth-ink-700)' }}>
                {t(`firstRun.failure.${stem}.body`)}
              </p>
              <div style={{ display: 'flex', alignItems: 'center', gap: 10, flexWrap: 'wrap' }}>
                {act && (
                  <PixelButton variant="primary" size="md" onClick={onAction} disabled={act === 'retry' && retrying}>
                    {actionLabel}
                  </PixelButton>
                )}
              </div>
              {retryError && (
                <ErrorNotice
                  error={retryError}
                  legacy={t('firstRun.retryFailed', { error: retryError })}
                  layout="inline"
                  context="start"
                  style={{ padding: 0 }}
                />
              )}
            </>
          ) : (
            <>
              <div style={{ fontSize: 12, color: 'var(--cth-ink-500)' }}>{counter}</div>
              <div style={{ fontSize: 14, lineHeight: '20px', fontWeight: 700, color: 'var(--cth-ink-900)' }}>
                {stepText}
              </div>
              {/* Indeterminate: we cannot know how far a download or a dependency
                  build is, so the bar only says "working" — never a fake percentage. */}
              <div
                role="progressbar"
                aria-busy="true"
                aria-label={t('firstRun.barLabel')}
                aria-valuetext={`${counter}: ${stepText}`}
                style={{
                  position: 'relative', height: 10, overflow: 'hidden',
                  background: 'var(--cth-cream-200)', boxShadow: 'inset 0 0 0 1px var(--cth-ink-300)'
                }}
              >
                <span className="cth-frp-fill" />
              </div>
              <p style={{ margin: 0, fontSize: 12, lineHeight: '18px', color: 'var(--cth-ink-700)' }}>
                {longWait ? t('firstRun.hintLong') : t('firstRun.hint')}
              </p>
            </>
          )}
        </div>

        <div style={{ padding: '0 16px 14px', display: 'flex', flexDirection: 'column', gap: 6 }}>
          <button
            type="button"
            onClick={() => setOpen((o) => !o)}
            aria-expanded={open}
            aria-controls={detailsId}
            style={{
              alignSelf: 'flex-start', background: 'none', border: 'none', padding: 0, cursor: 'pointer',
              fontSize: 12, color: 'var(--cth-ink-700)', textDecoration: 'underline'
            }}
          >
            {open ? t('firstRun.hideDetails') : t('firstRun.showDetails')}
          </button>
          {open && <Details id={detailsId} state={state} />}
        </div>
      </PixelPanel>
    </div>
    </>
  );
}

function Details({ id, state }: { id: string; state: EngineSetupState }) {
  const { t } = useTranslation();
  // Newest line last, and in view: a log you have to scroll to find the end of
  // is the wrong way round for "what is it doing right now?".
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = box.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [state.lines]);
  return (
    <div
      ref={box}
      id={id}
      role="region"
      aria-label={t('firstRun.detailsLabel')}
      aria-live="off"
      tabIndex={0}
      style={{
        maxHeight: 130, overflowY: 'auto', padding: '6px 8px',
        background: 'var(--cth-paper-100)', boxShadow: 'inset 0 0 0 1px var(--cth-ink-300)',
        fontFamily: 'var(--cth-font-mono)', fontSize: 11, lineHeight: '15px', color: 'var(--cth-ink-700)'
      }}
    >
      {state.lines.length === 0
        ? <span>{t('firstRun.detailsEmpty')}</span>
        : state.lines.map((l, i) => (
          <div key={`${i}-${l}`} style={{ wordBreak: 'break-word' }}>{l}</div>
        ))}
    </div>
  );
}
