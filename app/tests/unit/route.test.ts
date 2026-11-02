import { afterEach, expect, it, vi } from 'vitest';

afterEach(() => { vi.unstubAllGlobals(); vi.resetModules(); });

it('retains raw question marks in query values on initial and later routes', async () => {
  const location = { hash: '#/explorer?q=report?draft' };
  const callbacks: Array<() => void> = [];
  vi.stubGlobal('location', location);
  vi.stubGlobal('window', { addEventListener: (_: string, callback: () => void) => callbacks.push(callback) });
  const { route } = await import('../../src/lib/stores/route.svelte');
  expect(new URLSearchParams(route.search).get('q')).toBe('report?draft');
  location.hash = '#/today'; callbacks[0]!();
  expect(route.name).toBe('today'); expect(route.search).toBe('');
  location.hash = '#/explorer?q=ext:pdf?draft&node=nd_test'; callbacks[0]!();
  expect(route.name).toBe('explorer');
  expect(new URLSearchParams(route.search).get('q')).toBe('ext:pdf?draft');
  expect(new URLSearchParams(route.search).get('node')).toBe('nd_test');
});
