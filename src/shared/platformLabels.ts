/**
 * Platform-specific wording and examples. The copy was written for macOS
 * ("hold Option", "/path/to/..."); Windows and Linux users read the same
 * strings, so the few platform-bound words are interpolated rather than baked in.
 */
import { isWindowsHost } from './pathUtil';

/** True on macOS, from the bridge's platform (falls back to the UA). */
export function isMacHost(): boolean {
  try {
    const p = (window as unknown as { cth?: { platform?: string } }).cth?.platform;
    if (p) return p === 'darwin';
    return typeof navigator !== 'undefined' && /Mac/i.test(navigator.userAgent);
  } catch {
    return false;
  }
}

/** The modifier the hold-to-talk gesture uses, as the user's keyboard labels it. */
export function altKeyLabel(): string {
  return isMacHost() ? 'Option (⌥)' : 'Alt';
}

/** The modifier that turns a terminal click into "open this link", as labelled. */
export function linkModifierLabel(): string {
  return isMacHost() ? 'Cmd' : 'Ctrl';
}

/** An example absolute folder path in the host's own spelling, for placeholders. */
export function examplePath(name: string): string {
  return isWindowsHost() ? `C:\\Users\\you\\${name}` : `/path/to/${name}`;
}
