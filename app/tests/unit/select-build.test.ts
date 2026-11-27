import { describe, expect, it, vi } from 'vitest';
import { buildTransport, chooseTransport } from '../../src/lib/transport/select.js';
import type { Env } from '../../src/lib/transport/select.js';

// Mock the lazily-imported desktop bridge only so importing the select module in
// Vitest never loads Tauri APIs. No test below exercises the Tauri branch.
vi.mock('../../src/lib/transport/tauri', () => ({
  createTauriTransport: async () => ({ mode: 'tauri' as const, call: vi.fn(), subscribe: vi.fn() }),
}));

const TOKEN = 'a1b2c3d4e5f60718293a4b5c6d7e8f90';
const LOOPBACK_ORIGIN = 'http://127.0.0.1:5000';

describe('select built-mode contract', () => {
  it('reports an unavailable engine and excludes that choice from buildTransport', () => {
    const env: Env = { search: '', hash: '', origin: LOOPBACK_ORIGIN, hasTauri: false, storedToken: null };
    const choice = chooseTransport(env);
    if (choice.mode !== 'none') throw new Error('expected an unavailable engine');
    expect(choice.reason).toContain('No engine connection');

    const unavailableChoice = choice;
    if (false) {
      // @ts-expect-error - unavailable choices cannot be built
      void buildTransport(unavailableChoice);
    }
  });

  it('explicit DEV mock request builds a mock transport', async () => {
    const env: Env = { search: '?transport=mock', hash: '', origin: LOOPBACK_ORIGIN, hasTauri: false, storedToken: null };
    const choice = chooseTransport(env, true);
    expect(choice).toEqual({ mode: 'mock' });
    if (choice.mode !== 'mock') throw new Error('expected an explicit mock choice');
    const transport = await buildTransport(choice);
    expect(transport.mode).toBe('mock');
  });

  it('token hash builds an http transport', async () => {
    const env: Env = { search: '', hash: `#token=${TOKEN}`, origin: LOOPBACK_ORIGIN, hasTauri: false, storedToken: null };
    const choice = chooseTransport(env);
    expect(choice.mode).toBe('http');
    if (choice.mode !== 'http') throw new Error('expected an http choice');
    expect(choice.token).toBe(TOKEN);
    const transport = await buildTransport(choice);
    expect(transport.mode).toBe('http');
  });
});
