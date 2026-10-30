import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [react()],
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
  test: { environment: 'jsdom' },
});
