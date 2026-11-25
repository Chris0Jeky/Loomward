import { describe, expect, it, vi } from 'vitest';
import { buildTransport, chooseTransport, tokenFromHash, type Env } from '../../src/lib/transport/select';

vi.mock('../../src/lib/transport/tauri', () => ({
  createTauriTransport: async () => ({ mode: 'tauri', call: vi.fn(), subscribe: vi.fn() }),
}));

const TOKEN = 'a1b2c3d4e5f60718293a4b5c6d7e8f90';
const env = (o: Partial<Env> = {}): Env => ({ search: '', hash: '', origin: 'http://127.0.0.1:5000', hasTauri: false, storedToken: null, ...o });

describe('chooseTransport', () => {
  it('never falls back to mock: no engine means none', () => {
    expect(chooseTransport(env()).mode).toBe('none');
  });
  it('uses mock only when asked explicitly', () => {
    expect(chooseTransport(env({ search: '?transport=mock' }))).toEqual({ mode: 'mock' });
  });
  it('ignores the mock override in production and retains the real transport', () => {
    expect(chooseTransport(env({ search: '?transport=mock', hasTauri: true }), false)).toEqual({ mode: 'tauri' });
    expect(chooseTransport(env({ search: '?transport=mock' }), false).mode).toBe('none');
    expect(chooseTransport(env({ search: '?transport=mock' }), true).mode).toBe('mock');
  });
  it('prefers the desktop bridge, then a fragment token, then the tab-stored token', () => {
    expect(chooseTransport(env({ hasTauri: true })).mode).toBe('tauri');
    expect(chooseTransport(env({ hash: `#token=${TOKEN}` }))).toEqual({ mode: 'http', base: 'http://127.0.0.1:5000', token: TOKEN });
    expect(chooseTransport(env({ storedToken: TOKEN })).mode).toBe('http');
  });
  it('accepts a cross-origin api only on loopback', () => {
    const hash = `#token=${TOKEN}`;
    expect(chooseTransport(env({ hash, search: '?api=http://127.0.0.1:7777' }))).toMatchObject({ mode: 'http', base: 'http://127.0.0.1:7777' });
    expect(chooseTransport(env({ hash, search: '?api=https://evil.example' })).mode).toBe('none');
    expect(chooseTransport(env({ hash, search: '?api=http://127.0.0.1.evil.example' })).mode).toBe('none');
  });
  it.each([':8765', ''])('normalizes an accepted localhost API address for the exact Host gate (port: %s)', (port) => {
    expect(chooseTransport(env({ hash: `#token=${TOKEN}`, search: `?api=http://localhost${port}` })))
      .toEqual({ mode: 'http', base: `http://127.0.0.1${port}`, token: TOKEN });
  });
  it('rejects malformed tokens', () => {
    expect(tokenFromHash('#token=short')).toBeNull();
    expect(tokenFromHash('#/explorer')).toBeNull();
    expect(tokenFromHash(`#token=${TOKEN}`)).toBe(TOKEN);
  });
});

describe('buildTransport', () => {
  it('resolves mode mock to a mock transport', async () => {
    const t = await buildTransport({ mode: 'mock' });
    expect(t).toBeDefined();
    expect(t.mode).toBe('mock');
    expect(typeof t.call).toBe('function');
    expect(typeof t.subscribe).toBe('function');
  });
  it('resolves mode http to an http transport', async () => {
    const t = await buildTransport({ mode: 'http', base: 'http://127.0.0.1:5000', token: 't' });
    expect(t).toBeDefined();
    expect(t.mode).toBe('http');
    expect(typeof t.call).toBe('function');
    expect(typeof t.subscribe).toBe('function');
  });
  it('resolves mode tauri through the lazy desktop transport module', async () => {
    const t = await buildTransport({ mode: 'tauri' });
    expect(t.mode).toBe('tauri');
  });
});
