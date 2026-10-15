import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { PixelPanel } from './PixelPanel';
import { PixelButton } from './PixelButton';
import { Icon } from './Icon';
import { useEscape } from '@/hooks/useEscape';

export interface FloorCloseModalProps {
  /** Live terminals this floor still owns. */
  ptyCount: number;
  /** Keep the floor open. */
  onCancel: () => void;
  /** Stop this floor's agents and close the window (`window_close_floor`).
   *  A rejection is shown and the buttons come back. */
  onConfirm: () => void | Promise<unknown>;
}

/** Shown when a floor with running terminals is asked to close (its backend
 *  holds the close until this answers). Scoped to THIS floor: other windows and
 *  their agents are untouched — unlike QuitWarningModal, which is the app-wide
 *  quit gate on the main window. */
export function FloorCloseModal({ ptyCount, onCancel, onConfirm }: FloorCloseModalProps) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEscape(busy ? null : onCancel);

  const confirm = async () => {
    setBusy(true);
    setError(null);
    try {
      await onConfirm(); // success destroys this window
    } catch (e) {
      setError(t('floor.closeFailed', { error: e instanceof Error ? e.message : String(e) }));
      setBusy(false);
    }
  };

  return (
    <div
      role="alertdialog"
      aria-label={t('floor.closeTitle')}
      onClick={busy ? undefined : onCancel}
      style={{
        position: 'fixed', inset: 0,
        background: 'rgba(15, 28, 46, 0.7)',
        display: 'flex', alignItems: 'center', justifyContent: 'center',
        zIndex: 1000
      }}
    >
      <div onClick={(e) => e.stopPropagation()} style={{ width: 460, maxWidth: '92vw' }}>
        <PixelPanel variant="dialog" title={t('floor.closeTitle')} noPadding>
          <div style={{ padding: 20, display: 'flex', flexDirection: 'column', gap: 16 }}>
            <div style={{ display: 'flex', gap: 12, alignItems: 'flex-start' }}>
              <div style={{
                width: 32, height: 32,
                background: 'var(--cth-coral-light)',
                boxShadow: 'inset 0 0 0 1.5px var(--cth-ink-500)',
                display: 'flex', alignItems: 'center', justifyContent: 'center',
                flexShrink: 0
              }}>
                <Icon name="bell" />
              </div>
              <div style={{ fontSize: 15, lineHeight: '22px', color: 'var(--cth-ink-700)' }}>
                {ptyCount === 1
                  ? t('floor.closeBody', { count: ptyCount })
                  : t('floor.closeBodyPlural', { count: ptyCount })}
              </div>
            </div>
            {error && (
              <div role="alert" style={{
                padding: 8,
                background: 'var(--cth-coral-light)',
                boxShadow: 'inset 0 0 0 1px var(--cth-ink-100)',
                fontSize: 12, lineHeight: '18px',
                color: 'var(--cth-ink-900)'
              }}>
                {error}
              </div>
            )}
            <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, flexWrap: 'wrap' }}>
              <PixelButton variant="secondary" size="md" onClick={onCancel} disabled={busy}>
                {t('floor.keepOpen')}
              </PixelButton>
              <PixelButton variant="destructive" size="md" onClick={confirm} disabled={busy}>
                {busy ? t('floor.closing') : t('floor.stopAndClose')}
              </PixelButton>
            </div>
          </div>
        </PixelPanel>
      </div>
    </div>
  );
}
