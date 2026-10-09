import type { Plugin } from 'vite';
import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// Built page carries a strict CSP (docs/41 section 12): same-origin scripts and styles, no remote
// origins. connect-src allows the loopback API (the dev server talks to loomward-serve cross-origin)
// and Tauri IPC. Dev keeps no CSP because Vite injects inline styles for HMR.
const csp = [
  "default-src 'none'",
  "script-src 'self'",
  "style-src 'self'",
  "img-src 'self' data:",
  "font-src 'self'",
  "connect-src 'self' http://127.0.0.1:* http://localhost:* http://[::1]:* ipc: http://ipc.localhost",
  "base-uri 'none'",
  "form-action 'none'",
].join('; ');

const buildCsp = (): Plugin => ({
  name: 'loomward-csp',
  apply: 'build',
  transformIndexHtml: () => [
    { tag: 'meta', attrs: { 'http-equiv': 'Content-Security-Policy', content: csp }, injectTo: 'head-prepend' },
  ],
});

export default defineConfig({
  base: './',
  plugins: [svelte(), buildCsp()],
  build: { target: 'es2022', modulePreload: { polyfill: false } },
  test: { include: ['tests/unit/**/*.test.ts'], environment: 'node' },
});
