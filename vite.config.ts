import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'node:path';
import pkg from './package.json' with { type: 'json' };

// https://v2.tauri.app/start/frontend/vite/
export default defineConfig({
  plugins: [react()],
  // Upstream electron.vite.config.ts injected this for the renderer build;
  // the Tauri vite config never carried it over, so every __APP_VERSION__
  // read (UpdateBadge, runningAppVersion) was a runtime ReferenceError that
  // blanked the app the moment the main shell mounted.
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version)
  },
  resolve: {
    alias: {
      '@': path.resolve(__dirname, 'src'),
      '@shared': path.resolve(__dirname, 'src/shared'),
      '@brand': path.resolve(__dirname, 'docs')
    }
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] }
  },
  envPrefix: ['VITE_', 'TAURI_'],
  // .tmj (Tiled map JSON) is shipped as a static asset for the floor renderer.
  assetsInclude: ['**/*.tmj']
});
