// Explorer stale-breadcrumb fallback (issue #198): a `not_found` while refetching a saved
// breadcrumb climbs to the deepest ancestor that still lists (the starting points when none
// does) and names the folder that vanished; any other error, or a `not_found` for a
// node the user just clicked, keeps today's error behaviour.
import { describe, expect, it } from 'vitest';
import { LoomwardError } from '../../src/lib/transport/client';
import { fallbackStep, onRefetchError } from '../../src/views/explorer/refetch';

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

  it('climbs one crumb per step and keeps naming the deepest folder that vanished', () => {
    const trail = [{ id: 'a', name: 'Root' }, { id: 'b', name: 'Docs' }, { id: 'c', name: 'Old' }];
    const first = fallbackStep(trail);
    expect(first).toEqual({ keep: trail.slice(0, 2), lost: 'Old' });
    const second = fallbackStep(first.keep, first.lost);
    expect(second).toEqual({ keep: trail.slice(0, 1), lost: 'Old' });
    const top = fallbackStep(second.keep, second.lost);
    expect(top).toEqual({ keep: [], lost: 'Old' }); // nothing left: reload the starting points
  });
});
