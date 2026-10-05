import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
import path from 'node:path';
import pkg from './package.json' with { type: 'json' };

export default defineConfig({
  plugins: [react()],
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
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['src/test/setup.ts'],
    // Pixi/xterm/Monaco leave timers + RAF loops behind; they are mocked per
    // test file anyway, and this keeps the suite from hanging the run.
    teardownTimeout: 5000
  },
  assetsInclude: ['**/*.tmj']
});
