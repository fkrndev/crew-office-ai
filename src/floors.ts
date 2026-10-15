/**
 * Multi-window "floors" — renderer side (backend: src-tauri/src/floors.rs).
 *
 * `main` is the primary window; extra windows are `floor-<n>` (n >= 2), opened
 * only while the `multiWindow` config flag is on. Every window runs this same
 * bundle, so anything that is per-floor state has to be told apart by the
 * window's label:
 *
 *  - localStorage is ONE profile for all WebView2 windows, so the roster keys
 *    (agents, archive, restorable list, selection, queues…) would collide.
 *    `floorKey()` leaves `main`'s keys byte-for-byte unchanged (existing
 *    installs keep their roster) and suffixes a floor's with `@<label>`.
 *  - the backend routes a PTY's output only to the window that spawned it
 *    (`emit_to(<owner label>)`), but a JS listener registered WITHOUT a target
 *    hears every emit regardless. `listenTarget()` is the label to pass as the
 *    `target` option so a window only hears what is addressed to it (or
 *    broadcast to everyone).
 */

export const MAIN_LABEL = 'main';
export const FLOOR_PREFIX = 'floor-';

type TauriMeta = {
  __TAURI_INTERNALS__?: {
    metadata?: { currentWebview?: { label?: string }; currentWindow?: { label?: string } };
  };
};

/** This window's label. Synchronous (Tauri injects it before any script runs);
 *  `main` when not running under Tauri (tests, plain browser). */
export function windowLabel(): string {
  try {
    const meta = (window as unknown as TauriMeta).__TAURI_INTERNALS__?.metadata;
    const label = meta?.currentWebview?.label ?? meta?.currentWindow?.label;
    return typeof label === 'string' && label ? label : MAIN_LABEL;
  } catch {
    return MAIN_LABEL;
  }
}

/** `floor-3` -> 3. `main` and anything malformed -> null (main is floor 1). */
export function floorNumber(label: string = windowLabel()): number | null {
  if (!label.startsWith(FLOOR_PREFIX)) return null;
  const n = Number(label.slice(FLOOR_PREFIX.length));
  return Number.isInteger(n) && n >= 2 ? n : null;
}

export function isFloorWindow(label: string = windowLabel()): boolean {
  return floorNumber(label) !== null;
}

/** The localStorage key for per-floor state. `main` keeps `key` as-is. */
export function floorKey(key: string, label: string = windowLabel()): string {
  return isFloorWindow(label) ? `${key}@${label}` : key;
}

/** `target` option for `listen()` — scopes a listener to this window. */
export function listenTarget(label: string = windowLabel()): string {
  return label;
}

/** The New Floor entry points (header button, Ctrl+Shift+N) exist only while the
 *  `multiWindow` flag is on — default OFF, so absent reads as off. The backend
 *  refuses `window_new_floor` too; this just keeps the UI honest. */
export function newFloorEnabled(config: { multiWindow?: boolean } | null | undefined): boolean {
  return config?.multiWindow === true;
}

/** Ctrl+Shift+N (Cmd+Shift+N on macOS). */
export function isNewFloorShortcut(
  e: Pick<KeyboardEvent, 'key' | 'ctrlKey' | 'metaKey' | 'shiftKey' | 'altKey'>
): boolean {
  return (e.ctrlKey || e.metaKey) && e.shiftKey && !e.altKey && e.key.toLowerCase() === 'n';
}

/** The "confirm closing a floor" payload main sends (`floor:closeRequested`). */
export interface FloorCloseRequest {
  ptyCount: number;
}
