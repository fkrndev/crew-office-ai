import { create } from 'zustand';
import { useNow } from '@/hooks/useNow';
import {
  listenEngineSetup,
  reduceSetupEvent,
  setupStep,
  shouldShowSetup,
  type EngineSetupBridge,
  type EngineSetupEvent,
  type EngineSetupState
} from '@shared/engineSetup';

/**
 * Per-agent first-run setup state, fed by the backend's `engine:setup` events.
 * Cleared when the agent is ready (or stopped); a failure stays until the user
 * acts on it. Its own small store (not the big app store): it has one writer and
 * a handful of readers, and it can be unit-tested without the rest of the app.
 */
interface EngineSetupStore {
  byAgent: Record<string, EngineSetupState>;
  apply: (ev: EngineSetupEvent) => void;
  clear: (agentId: string) => void;
  reset: () => void;
}

export const useEngineSetup = create<EngineSetupStore>((set) => ({
  byAgent: {},
  apply: (ev) => set((s) => {
    if (!ev?.agentId) return s;
    const next = reduceSetupEvent(s.byAgent[ev.agentId], ev);
    if (next === s.byAgent[ev.agentId]) return s;
    const byAgent = { ...s.byAgent };
    if (next) byAgent[ev.agentId] = next; else delete byAgent[ev.agentId];
    return { byAgent };
  }),
  clear: (agentId) => set((s) => {
    if (!s.byAgent[agentId]) return s;
    const byAgent = { ...s.byAgent };
    delete byAgent[agentId];
    return { byAgent };
  }),
  reset: () => set({ byAgent: {} })
}));

/** Wire a bridge (the real `window.cth`, or a fake in tests) to the store. */
export function startEngineSetupFeed(bridge: EngineSetupBridge): () => void {
  return listenEngineSetup(bridge, (e) => useEngineSetup.getState().apply(e));
}

/** Badges for every agent whose setup is worth showing right now (a quick start
 *  stays invisible through its grace period). Ticks only while a setup exists. */
export function useSetupBadges(): Record<string, NonNullable<ReturnType<typeof setupBadge>>> {
  const byAgent = useEngineSetup((s) => s.byAgent);
  const now = useNow(Object.keys(byAgent).length > 0, 500);
  const out: Record<string, NonNullable<ReturnType<typeof setupBadge>>> = {};
  for (const [id, s] of Object.entries(byAgent)) {
    const b = shouldShowSetup(s, now) ? setupBadge(s) : undefined;
    if (b) out[id] = b;
  }
  return out;
}

/** The one-line status an agent CARD shows while its setup is active
 *  (`undefined` = nothing to show). Pure so the strip and tests share it. */
export function setupBadge(
  s: EngineSetupState | undefined
): { failed: boolean; current: number; total: number } | undefined {
  if (!s) return undefined;
  const { current, total } = setupStep(s);
  return { failed: s.phase === 'failed', current, total };
}
