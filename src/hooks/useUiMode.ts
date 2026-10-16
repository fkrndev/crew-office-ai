import { useStore } from '@/store/store';
import { resolveUiMode, type UiMode } from '@shared/uiMode';

/**
 * The live UI mode ('simple' | 'advanced') — see src/shared/uiMode.ts for how it
 * resolves. It reads the store's mirror of the RESOLVED value, which App refreshes
 * on every config read / config:changed and Settings refreshes on save, so every
 * component that calls this re-renders the moment the layout switch flips.
 *
 * The store default is 'advanced': anything rendered before a config has been
 * read (and every legacy install) gets the full, unchanged layout.
 */
export function useUiMode(): UiMode {
  return useStore((s) => s.uiMode);
}

/** The i18next options that select the plain-language `key_simple` variants, read
 *  at call time (for strings built outside a component, e.g. a status line written
 *  from an event handler). `undefined` in advanced mode. */
export function simpleContextNow(): { context: 'simple' } | undefined {
  return useStore.getState().uiMode === 'simple' ? { context: 'simple' } : undefined;
}

/** Convenience for the many call sites that only branch on simple. */
export function useIsSimpleMode(): boolean {
  return useStore((s) => s.uiMode) === 'simple';
}

/**
 * Persist a mode switch in one step (the "Need more? Show advanced options" links
 * and the one-click switch at the bottom of the simple Settings use this; the
 * Settings → General toggle stages the same keys through its own Save).
 *
 * Turning simple mode ON also sets `audience: 'non-technical'` so the
 * orchestrator speaks plainly too; turning it OFF never touches the audience —
 * somebody who wanted plain language may well still want it with every control
 * visible.
 *
 * The store flips first (optimistic, so the layout answers the click at once) and
 * is put back if the write is rejected. Resolves to whether it stuck.
 */
export async function persistUiMode(mode: UiMode): Promise<boolean> {
  const store = useStore.getState();
  const previous = store.uiMode;
  store.setUiMode(mode);
  try {
    const patch: Record<string, unknown> = { uiMode: mode };
    if (mode === 'simple') patch.audience = 'non-technical';
    await window.cth.updateConfig(patch);
    return true;
  } catch {
    useStore.getState().setUiMode(previous);
    return false;
  }
}

/** Mirror a freshly read config into the store (App calls this on every read). */
export function mirrorUiMode(config: Parameters<typeof resolveUiMode>[0]): void {
  const next = resolveUiMode(config);
  if (useStore.getState().uiMode !== next) useStore.getState().setUiMode(next);
}

/** Open Settings on the plain-language "AI connection" tab (simple mode) — the
 *  call to action behind every "Connect your AI" button. Goes through the same
 *  `cth:open-settings` window event as every other deep link into Settings. */
export function openAiConnection(): void {
  window.dispatchEvent(new CustomEvent('cth:open-settings', { detail: { section: 'AI connection' } }));
}
