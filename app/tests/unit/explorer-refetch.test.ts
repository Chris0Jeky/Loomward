// Explorer stale-breadcrumb fallback (issue #198): a `not_found` while refetching a saved
// breadcrumb falls back to the starting points; any other error, or a `not_found` for a
// node the user just clicked, keeps today's error behaviour.
import { describe, expect, it } from 'vitest';
import { LoomwardError } from '../../src/lib/transport/client';
import { FALLBACK_NOTICE, onRefetchError } from '../../src/views/explorer/refetch';

const boom = (code: 'not_found' | 'stale_generation' | 'invalid_request' | 'internal_error', detail: Record<string, unknown> | null = null) =>
  new LoomwardError({ code, message: `fake engine: ${code}`, retryable: false, detail });

describe('onRefetchError', () => {
  it('falls back on not_found for a saved breadcrumb, whatever the reason', () => {
    expect(onRefetchError(boom('not_found', { reason: 'no such node' }), true)).toBe('fallback');
    expect(onRefetchError(boom('not_found', { reason: 'epoch_changed' }), true)).toBe('fallback');
    expect(onRefetchError(boom('not_found'), true)).toBe('fallback');
  });

  it('keeps the error for a not_found on a node just clicked', () => {
    expect(onRefetchError(boom('not_found', { reason: 'no such node' }), false)).toBe('error');
  });

  it('keeps the error for other codes even with a saved breadcrumb', () => {
    expect(onRefetchError(boom('stale_generation'), true)).toBe('error');
    expect(onRefetchError(boom('invalid_request'), true)).toBe('error');
    expect(onRefetchError(boom('internal_error'), true)).toBe('error');
  });

  it('keeps the error for non-engine failures even with a saved breadcrumb', () => {
    expect(onRefetchError(new Error('engine down'), true)).toBe('error');
  });

  it('announces the fallback with the agreed status note', () => {
    expect(FALLBACK_NOTICE).toBe('The catalogue changed, so the view went back to the top level.');
  });
});
