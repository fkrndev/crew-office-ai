/**
 * Vitest setup — runs before every test file.
 *
 * - Real i18n (English resources bundled inline — no Suspense needed).
 * - A default FakeBridge as window.cth; tests replace/override via
 *   installFakeBridge() when they need real responses.
 * - jsdom shims for APIs the app touches but jsdom lacks.
 */
import '@/i18n';
import i18n from 'i18next';
import { vi } from 'vitest';
import { installFakeBridge } from './fakeBridge';
import { cleanup } from '@testing-library/react';
import { afterEach } from 'vitest';

// Pin the test language to English — tests assert English strings, and the
// app default is Bahasa Indonesia.
void i18n.changeLanguage('en');

installFakeBridge({
  // These are SYNC methods upstream (sendSync → loopback XHR): they must
  // return values, not Promises. A promise-shaped roster crashes the store's
  // module-load roster pick — the exact blank-page class we're guarding.
  rosterReadSync: () => null,
  readClipboardSync: () => '',
  harnessHomeSync: () => null
});

// jsdom lacks these; components probe them unconditionally.
if (!window.matchMedia) {
  window.matchMedia = (q: string) =>
    ({
      matches: false,
      media: q,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
      onchange: null
    }) as MediaQueryList;
}
if (!window.ResizeObserver) {
  class RO {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  window.ResizeObserver = RO as unknown as typeof ResizeObserver;
}
// Canvas 2D context — pixi/xterm ask for it; jsdom returns null which pixi
// interprets as "no WebGL" and throws. Mock the heavy components in tests
// that render them; for anything else a stub getContext is enough.
HTMLCanvasElement.prototype.getContext = vi.fn().mockReturnValue(null) as never;

// Keep tests isolated: unmount trees so effects/listeners don't leak across files.
afterEach(() => cleanup());
