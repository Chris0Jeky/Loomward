import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { session } from '../../src/lib/stores/session.svelte';

// Fragment-token alphabet enforced by select.tokenFromHash.
const TOKEN = 'a1b2c3d4e5f60718293a4b5c6d7e8f90';
const TOKEN_KEY = 'loomward.token';

type MutableSession = {
  streamEpoch: string | null;
  retryTimer: ReturnType<typeof setTimeout> | undefined;
  transport: unknown;
  stopStream: (() => void) | null;
  attempt: number;
  busy: boolean;
};

function resetSession(): void {
  const inner = session as unknown as MutableSession;
  if (inner.retryTimer !== undefined) {
    clearTimeout(inner.retryTimer);
    inner.retryTimer = undefined;
  }
  if (inner.stopStream) {
    try {
      inner.stopStream();
    } catch {
      // A leftover stream from an earlier start; dropping it is the reset.
    }
    inner.stopStream = null;
  }
  // Same approach as session.test.ts: reach the private epoch through a cast.
  inner.streamEpoch = null;
  inner.transport = null;
  inner.attempt = 0;
  inner.busy = false;
  session.state = 'connecting';
  session.mode = 'none';
  session.info = null;
  session.reason = '';
  session.client = null;
}

function stubBrowser(hash: string) {
  const replaceState = vi.fn();
  const getItem = vi.fn((): string | null => null);
  const setItem = vi.fn();
  vi.stubGlobal('location', { hash, search: '', pathname: '/', origin: 'http://127.0.0.1:5000' });
  vi.stubGlobal('sessionStorage', { getItem, setItem });
  // No __TAURI_INTERNALS__ key: the desktop bridge must stay off this path.
  vi.stubGlobal('window', {});
  vi.stubGlobal('history', { replaceState });
  return { replaceState, getItem, setItem };
}

describe('session.start token handling', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    resetSession();
  });

  afterEach(() => {
    resetSession();
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  it('scrubs a valid fragment token to "#/" and stores it for the tab', async () => {
    const { replaceState, setItem } = stubBrowser(`#token=${TOKEN}`);
    // No real engine in unit tests: the http call fails fast. The assertions
    // below cover only the scrub, not the connection outcome.
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('Failed to fetch')));

    await session.start();

    expect(replaceState).toHaveBeenCalledTimes(1);
    const scrubbed = replaceState.mock.calls[0]?.[2] as unknown;
    expect(typeof scrubbed).toBe('string');
    expect(scrubbed as string).toMatch(/#\/$/);
    expect(scrubbed as string).not.toContain(TOKEN);
    expect(setItem).toHaveBeenCalledTimes(1);
    expect(setItem).toHaveBeenCalledWith(TOKEN_KEY, TOKEN);
  });

  it('stays unavailable with no retry when no engine is configured', async () => {
    const { replaceState, setItem } = stubBrowser('#/');

    await session.start();

    expect(session.state).toBe('unavailable');
    expect(session.mode).toBe('none');
    expect(session.client).toBeNull();
    expect(session.info).toBeNull();
    expect(replaceState).not.toHaveBeenCalled();
    expect(setItem).not.toHaveBeenCalled();
    // markUnavailable(reason, false): no retry timer may be scheduled.
    expect(vi.getTimerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(session.state).toBe('unavailable');
    expect(vi.getTimerCount()).toBe(0);
  });
});
