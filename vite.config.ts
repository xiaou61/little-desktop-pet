import { svelte } from '@sveltejs/vite-plugin-svelte';
import { svelteTesting } from '@testing-library/svelte/vite';
import { defineConfig } from 'vite';
import { fileURLToPath } from 'node:url';

export default defineConfig({
  plugins: [svelte(), svelteTesting()],
  clearScreen: false,
  build: {
    rollupOptions: {
      input: {
        dashboard: fileURLToPath(new URL('./index.html', import.meta.url)),
        quickPanel: fileURLToPath(new URL('./quick-panel.html', import.meta.url)),
        pluginManager: fileURLToPath(new URL('./plugin-manager.html', import.meta.url)),
        diagnostics: fileURLToPath(new URL('./diagnostics.html', import.meta.url))
      }
    }
  },
  server: {
    strictPort: true,
    watch: {
      ignored: ['**/src-tauri/**']
    }
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/__tests__/setup.ts'],
    include: ['src/**/*.test.ts'],
    exclude: ['.bun-cache/**', '.node_modules-incomplete-*/**', 'node_modules/**'],
    css: true
  }
});
