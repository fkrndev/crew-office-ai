import { useEffect, useRef } from 'react';
import { isComposingKey } from '@shared/imeGuard';

/**
 * Esc dismisses a modal.
 *
 * Capture phase + stopImmediatePropagation, the same way AddAgentModal does it:
 * the fullscreen terminal's window-level Esc handler (and any other bubble-phase
 * listener underneath) must not ALSO act on the keypress that closed this layer.
 * Pass `null` (or `enabled: false`) while the modal must not be dismissable —
 * mid-spawn, mid-shutdown — and the key is left alone entirely.
 *
 * An Esc that cancels an IME composition belongs to the IME, not to us.
 */
export function useEscape(onEscape: (() => void) | null, enabled = true): void {
  // Latest handler without re-subscribing on every render.
  const ref = useRef(onEscape);
  ref.current = onEscape;
  const active = enabled && onEscape !== null;

  useEffect(() => {
    if (!active) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || isComposingKey(e)) return;
      const fn = ref.current;
      if (!fn) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      fn();
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [active]);
}
