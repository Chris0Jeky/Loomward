import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const KEY = 'loomward.theme';

function memoryStorage(seed: Record<string, string> = {}) {
  const map = new Map(Object.entries(seed));
  return {
    getItem: (k: string): string | null => (map.has(k) ? map.get(k)! : null),
    setItem: (k: string, v: string): void => {
      map.set(k, String(v));
    },
    removeItem: (k: string): void => {
      map.delete(k);
    },
    clear: (): void => {
      map.clear();
    },
  };
}

function stubDocument() {
  vi.stubGlobal('document', { documentElement: { dataset: {} as Record<string, string> } });
}

async function freshTheme() {
  vi.resetModules();
  return (await import('../../src/lib/stores/theme.svelte')).theme;
}

beforeEach(() => {
  stubDocument();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('theme store', () => {
  it('persists the chosen theme under the exact storage key', async () => {
    const storage = memoryStorage();
    vi.stubGlobal('localStorage', storage);
    const theme = await freshTheme();
    theme.set('observatory');
    expect(storage.getItem(KEY)).toBe('observatory');
  });

  it('defaults to woven-atlas when storage is empty', async () => {
    const storage = memoryStorage();
    vi.stubGlobal('localStorage', storage);
    storage.clear();
    const theme = await freshTheme();
    expect(theme.current).toBe('woven-atlas');
  });

  it('reapplies the stored theme on load', async () => {
    vi.stubGlobal('localStorage', memoryStorage({ [KEY]: 'observatory' }));
    const theme = await freshTheme();
    expect(theme.current).toBe('observatory');
  });
});
