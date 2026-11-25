import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// Ingress serves the app under a per-session prefix, so every URL must be relative.
export default defineConfig({
  base: './',
  plugins: [svelte()],
  server: {
    proxy: {
      '/api': { target: 'http://localhost:8099', ws: true },
    },
  },
  build: { target: 'es2022', chunkSizeWarningLimit: 1500 },
});
