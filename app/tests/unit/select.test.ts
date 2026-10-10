import { describe, expect, it } from 'vitest';
import { chooseTransport, tokenFromHash, type Env } from '../../src/lib/transport/select';

const TOKEN = 'a1b2c3d4e5f60718293a4b5c6d7e8f90';
const env = (o: Partial<Env> = {}): Env => ({ search: '', hash: '', origin: 'http://127.0.0.1:5000', hasTauri: false, storedToken: null, ...o });

describe('chooseTransport', () => {
  it('never falls back to mock: no engine means none', () => {
    expect(chooseTransport(env()).mode).toBe('none');
  });
  it('uses mock only when asked explicitly', () => {
    expect(chooseTransport(env({ search: '?transport=mock' }))).toEqual({ mode: 'mock' });
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
  it('rejects malformed tokens', () => {
    expect(tokenFromHash('#token=short')).toBeNull();
    expect(tokenFromHash('#/explorer')).toBeNull();
    expect(tokenFromHash(`#token=${TOKEN}`)).toBe(TOKEN);
  });
});
