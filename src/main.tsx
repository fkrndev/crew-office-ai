import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import brandLogo from '@brand/logo.png?url';
import './design/global.css';
import './i18n';
import { installCth, warmCth } from './cth';

// Upstream exposes the bridge via contextBridge in preload; in the Tauri port
// it's a same-process shim — must exist before ANY module touches window.cth.
// App (and through it store.ts / config.ts) is therefore imported dynamically
// below: a static `import { App }` is evaluated BEFORE this call, and those
// modules read window.cth at load time (roster, harness home, model catalog).
installCth();

const favicon = document.createElement('link');
favicon.rel = 'icon';
favicon.type = 'image/png';
favicon.href = brandLogo;
document.head.appendChild(favicon);

const splashMark = document.querySelector('#cth-splash .mk');
if (splashMark) {
  const img = document.createElement('img');
  img.src = brandLogo;
  img.alt = 'Agent Office';
  img.style.cssText = 'height:56px;width:auto;display:block';
  splashMark.replaceWith(img);
}

const root = document.getElementById('root');
if (!root) throw new Error('No root element');

async function boot(rootEl: HTMLElement): Promise<void> {
  // The sync reads (roster, harness home) are served from caches — fill them
  // before the store module is evaluated (bounded wait; a slow backend just
  // means the store falls back to localStorage as before).
  await warmCth();
  const { App } = await import('./App');
  createRoot(rootEl).render(
    <StrictMode>
      <App />
    </StrictMode>
  );
}

void boot(root);
