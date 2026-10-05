/**
 * FakeBridge — a controllable `window.cth` stand-in for tests.
 *
 * The real bridge (src/cth.ts) wraps every Tauri command with invoke(); tests
 * need the same SHAPE without the backend. Every method is a vi.fn() returning
 * `Promise.resolve(undefined)` by default, every `on*` returns an unsubscribe,
 * and tests override the handful they care about:
 *
 *   const cth = fakeBridge({
 *     getConfig: async () => ({ onboardingComplete: true, autoMode: true }),
 *     updateConfig: async (patch) => patch,
 *   });
 *
 * `calls` records every (method, args) pair — assert `updateConfig` got the
 * exact wizard patch without mocking shapes by hand.
 */
import { vi } from 'vitest';

export interface FakeBridge {
  [key: string]: unknown;
  /** (method, args[]) in call order — the whole conversation with main. */
  calls: Array<{ method: string; args: unknown[] }>;
  /** Handlers registered via on*(cb) — emit synthetic backend events. */
  emit: (method: string, payload: unknown) => void;
}

export function fakeBridge(overrides: Record<string, unknown> = {}): FakeBridge {
  const listeners = new Map<string, Set<(e: unknown) => void>>();
  const calls: FakeBridge['calls'] = [];

  const bridge = new Proxy(
    { calls, emit: (m: string, p: unknown) => listeners.get(m)?.forEach((cb) => cb(p)) } as FakeBridge,
    {
      get(target, prop: string) {
        if (prop in target) return target[prop as keyof FakeBridge];
        if (prop in overrides) return overrides[prop];
        // `on*` handlers: register + return unsubscribe.
        if (prop.startsWith('on')) {
          return (cb: (e: unknown) => void) => {
            let set = listeners.get(prop);
            if (!set) listeners.set(prop, (set = new Set()));
            set.add(cb);
            return () => set.delete(cb);
          };
        }
        // Non-method properties upstream exposes as values.
        if (prop === 'platform') return 'win32';
        if (prop === 'arch') return 'x64';
        if (prop === 'isPackaged') return false;
        // Every command: resolved-promise stub that records the call.
        return vi.fn(async (...args: unknown[]) => {
          calls.push({ method: prop, args });
          return undefined;
        });
      }
    }
  );
  return bridge;
}

/** Install a fake bridge as window.cth; returns it for assertions/overrides. */
export function installFakeBridge(overrides: Record<string, unknown> = {}): FakeBridge {
  const bridge = fakeBridge(overrides);
  (window as unknown as { cth: unknown }).cth = bridge;
  return bridge;
}
