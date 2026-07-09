import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [sveltekit()],
  server: {
    // Local dev proxy: the UI calls /api/* on the same origin, and
    // Vite forwards to the daemon. In production the daemon (or
    // Caddy) serves the API at the same path.
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:7300',
        changeOrigin: true
      }
    }
  }
});
