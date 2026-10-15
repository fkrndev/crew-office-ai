import { useEffect } from 'react';
import { startEngineSetupFeed } from '@/store/engineSetup';
import type { EngineSetupBridge } from '@shared/engineSetup';

/** Feed the per-agent first-run setup store from the backend's `engine:setup`
 *  events (and replay what happened before this window was listening). Mount
 *  once, near the root. */
export function useEngineSetupFeed(): void {
  useEffect(() => {
    if (!window.cth) return;
    return startEngineSetupFeed(window.cth as unknown as EngineSetupBridge);
  }, []);
}
