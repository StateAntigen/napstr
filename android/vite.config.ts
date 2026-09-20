import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte()],
  resolve: { dedupe: ['svelte'] },
  publicDir: '../static',
  clearScreen: false,
  server: {
    host: '0.0.0.0',
    port: 1421,
    strictPort: true,
    // Tauri watches native code; generated native builds can exhaust file watchers.
    watch: { ignored: ['**/src-tauri/**', '**/.cache/**'] },
    fs: { allow: ['..'] }
  },
  envPrefix: ['VITE_', 'TAURI_'],
  build: {
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari13',
    minify: process.env.TAURI_ENV_DEBUG ? false : 'esbuild',
    sourcemap: Boolean(process.env.TAURI_ENV_DEBUG)
  }
});
