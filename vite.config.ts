import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [sveltekit()],
  server: {
    // Tauri watches native code; generated native builds can exhaust file watchers.
    watch: { ignored: ['**/src-tauri/**', '**/.cache/**'] },
    fs: {
      // Tauri's persistent development webview can briefly request a module
      // from the previous Vite graph after a hot reload. The repository root
      // is trusted development source and includes package metadata.
      allow: ['.']
    }
  }
});
