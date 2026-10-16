/**
 * "Stop everything" (simple mode) — the renderer side of `control_stop_all`.
 *
 * Stopping is a clean halt of every live agent plus a hold on queued messages
 * (src-tauri/src/control.rs `halt_all`); nothing is killed or deleted. The hold is
 * lifted again by the person's NEXT message — the composer calls
 * `releaseStopIfNeeded()` when it sends — so stopping can never leave the team
 * deaf to the very next thing they type.
 */

export interface StopAllBridge {
  controlStopAll?(): Promise<unknown>;
  controlResumeStopped?(): Promise<unknown>;
}

export type StopAllOutcome =
  | { ok: true; stopped: number }
  | { ok: false; error: unknown };

/** True from a successful stop until the next message releases it. Module state:
 *  one window, one answer; a reload resets it exactly as the backend's in-memory
 *  halt flags are reset by a restart. */
let stoppedNow = false;

export const isStopped = (): boolean => stoppedNow;
/** Test seam. */
export const resetStopState = (): void => { stoppedNow = false; };

function countOf(r: unknown): number {
  const n = (r as { stopped?: unknown } | null | undefined)?.stopped;
  return typeof n === 'number' && Number.isFinite(n) && n >= 0 ? n : 0;
}

/** Ask the backend to halt everyone. Safe to repeat. Never throws. */
export async function stopEverything(bridge: StopAllBridge): Promise<StopAllOutcome> {
  try {
    if (!bridge.controlStopAll) return { ok: false, error: 'stop is not available' };
    const r = await bridge.controlStopAll();
    stoppedNow = true;
    return { ok: true, stopped: countOf(r) };
  } catch (error) {
    return { ok: false, error };
  }
}

/** Lift a previous stop, if there is one — called right before a message is
 *  queued. No-op (and no IPC) when nothing was stopped. Never throws. */
export async function releaseStopIfNeeded(bridge: StopAllBridge): Promise<void> {
  if (!stoppedNow) return;
  try {
    await bridge.controlResumeStopped?.();
    stoppedNow = false;
  } catch { /* still stopped as far as we know: the next message tries again */ }
}
