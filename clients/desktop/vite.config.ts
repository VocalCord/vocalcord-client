import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Tauri 2 picks up VITE_* env vars automatically. Port and HMR
// behaviour mirror the official Tauri scaffold so `tauri dev` can
// proxy the front-end correctly.
export default defineConfig(async () => ({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: false,
    hmr: { protocol: 'ws', host: 'localhost', port: 1421 },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
}));
