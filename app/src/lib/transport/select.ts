import { createHttpTransport } from './http';
import { createMockTransport } from './mock';
import type { Transport } from './transport';

export type Choice =
  | { mode: 'mock' }
  | { mode: 'tauri' }
  | { mode: 'http'; base: string; token: string }
  | { mode: 'none'; reason: string };

export interface Env {
  search: string;
  hash: string;
  origin: string;
  hasTauri: boolean;
  /** Token kept for this tab from an earlier load (the fragment is scrubbed after reading). */
  storedToken: string | null;
}

const LOOPBACK = /^http:\/\/(127\.0\.0\.1|localhost|\[::1\])(:\d{1,5})?$/;

/** Token from `#token=...`, or null. The fragment never reaches a server. */
export function tokenFromHash(hash: string): string | null {
  const t = new URLSearchParams(hash.replace(/^#\/?/, '')).get('token');
  return t && /^[A-Za-z0-9._~+=-]{16,512}$/.test(t) ? t : null;
}

/**
 * Mock data only ever appears when asked for with `?transport=mock`. With no engine and no explicit
 * mock the answer is `none`, which the shell renders as "unavailable": never demo data by default.
 */
export function chooseTransport(env: Env): Choice {
  const q = new URLSearchParams(env.search);
  if (q.get('transport') === 'mock') return { mode: 'mock' };
  if (env.hasTauri) return { mode: 'tauri' };
  const token = tokenFromHash(env.hash) ?? env.storedToken;
  if (!token) {
    return { mode: 'none', reason: 'No engine connection. Open the address printed by loomward-serve (it ends in #token=...) or start the desktop app.' };
  }
  // `?api=` lets the Vite dev server (a different loopback origin) reach loomward-serve; loopback only.
  const api = q.get('api');
  if (api !== null && !LOOPBACK.test(api)) return { mode: 'none', reason: 'The api parameter must be a loopback http address.' };
  return { mode: 'http', base: api ?? env.origin, token };
}

export async function buildTransport(c: Exclude<Choice, { mode: 'none' }>): Promise<Transport> {
  if (c.mode === 'mock') return createMockTransport();
  if (c.mode === 'http') return createHttpTransport({ base: c.base, token: c.token });
  return (await import('./tauri')).createTauriTransport();
}
