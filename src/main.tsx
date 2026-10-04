import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import brandLogo from '@brand/logo.png?url';
import './design/global.css';
import './i18n';
import { installCth } from './cth';

// Upstream exposes the bridge via contextBridge in preload; in the Tauri port
// it's a same-process shim — must exist before ANY module touches window.cth.
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

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>
);
