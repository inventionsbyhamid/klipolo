import { fileURLToPath } from 'node:url';
import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

const shim = (file: string) => fileURLToPath(new URL(`./src/i18n/shim/${file}`, import.meta.url));

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    // English only: lingui's imports resolve to a small runtime shim, so files keep their import lines and
    // lingui can come back by deleting these four lines (the message ids are kept).
    alias: [
      { find: /^@lingui\/core\/macro$/, replacement: shim('coreMacro.ts') },
      { find: /^@lingui\/react\/macro$/, replacement: shim('reactMacro.tsx') },
      { find: /^@lingui\/core$/, replacement: shim('core.ts') },
      { find: /^@lingui\/react$/, replacement: shim('react.tsx') },
    ],
  },
  // Tauri's dev window loads this exact origin, which the engine's frame socket allows in debug builds.
  server: {
    host: '127.0.0.1',
    port: 1420,
    strictPort: true,
    watch: { ignored: ['**/crates/**', '**/target/**'] },
  },
  clearScreen: false,
  // WKWebView on macOS 15 (Safari 18).
  build: { target: 'safari18', sourcemap: false },
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.{ts,tsx}'],
    setupFiles: ['src/test/setup.ts'],
    // A busy machine makes first renders slow; that shouldn't fail the suite.
    testTimeout: 15000,
  },
});
