import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    adapter: adapter({
      pages: 'build',
      assets: 'build',
      fallback: 'index.html',
      precompress: false,
      strict: true
    }),
    // The UI is a thin client for the daemon's /api/v1. We disable
    // SSR so we can ship a single static bundle; the daemon is the
    // source of truth for state.
    prerender: {
      handleHttpError: 'warn'
    }
  }
};

export default config;
