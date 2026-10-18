// Explorer stale-breadcrumb fallback (issue #198): a `not_found` while refetching a saved
// breadcrumb climbs to the deepest ancestor that still lists (the starting points when none
// does) and names the folder that vanished; any other error, or a `not_found` for a
// node the user just clicked, keeps today's error behaviour.
import { describe, expect, it } from 'vitest';
import { LoomwardError } from '../../src/lib/transport/client';
import { fallbackStep, onRefetchError, resolveTrail } from '../../src/views/explorer/refetch';

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

describe('resolveTrail after node IDs change', () => {
  const saved = [{ id: 'old-root', name: 'Root' }, { id: 'old-docs', name: 'Docs' }, { id: 'old-leaf', name: 'Old' }];
  const root = { id: 'new-root', name: 'Root' };
  const docs = { id: 'new-docs', name: 'Docs' };
  const leaf = { id: 'new-leaf', name: 'Old' };
  const page = (folders: typeof saved, next_cursor: string | null = null) => ({
    items: folders.map(({ id, name }) => ({ node_id: id, name, kind: 'dir' as const })), next_cursor,
  });

  it('keeps the deepest existing path with fresh IDs instead of the first starting point', async () => {
    const children = async (id: string) => page(id === root.id ? [docs] : []);
    expect(await resolveTrail(saved, [{ id: 'other', name: 'Other' }, root], children)).toEqual([root, docs]);
  });

  it('retains the whole path when only its IDs changed', async () => {
    const children = async (id: string) => page(id === root.id ? [docs] : id === docs.id ? [leaf] : []);
    expect(await resolveTrail(saved, [root], children)).toEqual([root, docs, leaf]);
  });

  it('finds ancestors beyond the first page and ignores same-named files', async () => {
    const calls: (string | null)[] = [];
    const children = async (id: string, cursor: string | null) => {
      if (id !== root.id) return page([]);
      calls.push(cursor);
      return cursor ? page([docs]) : { items: [{ node_id: 'file', name: 'Docs', kind: 'file' as const }], next_cursor: 'next' };
    };
    expect(await resolveTrail(saved, [root], children)).toEqual([root, docs]);
    expect(calls).toEqual([null, 'next']);
  });

  it('does not guess when the starting point disappeared or its name is ambiguous', async () => {
    const children = async () => { throw new Error('must not list an unrelated root'); };
    expect(await resolveTrail(saved, [], children)).toEqual([]);
    expect(await resolveTrail(saved, [root, { ...root, id: 'another' }], children)).toEqual([]);
  });

  it('keeps the listed ancestor if a matching child vanishes during recovery', async () => {
    const children = async (id: string) => {
      if (id === root.id) return page([docs]);
      throw boom('not_found');
    };
    expect(await resolveTrail(saved, [root], children)).toEqual([root]);
  });

  it('does not pass off an unavailable catalogue as a missing folder', async () => {
    await expect(resolveTrail(saved, [root], async () => { throw boom('internal_error'); })).rejects.toMatchObject({ code: 'internal_error' });
  });

  describe('bound to the granted root path, not its name or an ID (#198)', () => {
    const bound = [{ id: 'old-root', name: 'Root', root: 'synthetic/root-1' }, ...saved.slice(1)];
    const twin = { id: 'twin-root', name: 'Root' };
    const rootOf = async (id: string) => (id === root.id ? 'synthetic/root-1' : id === twin.id ? 'synthetic/root-2' : null);

    it('picks the root with the saved granted path among same-named roots', async () => {
      const children = async (id: string) => page(id === root.id ? [docs] : []);
      expect(await resolveTrail(bound, [twin, root], children, rootOf)).toEqual([{ ...root, root: 'synthetic/root-1' }, docs]);
    });

    it('does not restore a different root of the same name and sub-path when the original root is gone', async () => {
      const children = async (id: string) => page(id === twin.id ? [docs] : id === docs.id ? [leaf] : []);
      expect(await resolveTrail(bound, [twin], children, rootOf)).toEqual([]);
    });

    it('never picks between two starting points that report the same granted path', async () => {
      expect(await resolveTrail(bound, [twin, root], async () => page([]), async () => 'synthetic/root-1')).toEqual([]);
    });

    it('recovers the whole trail after an engine restart re-mints every ID but not the granted path', async () => {
      const restarted = { id: 'rt_after_restart', name: 'Root' }, docs2 = { id: 'nd_docs_after', name: 'Docs' }, leaf2 = { id: 'nd_leaf_after', name: 'Old' };
      const children = async (id: string) => page(id === restarted.id ? [docs2] : id === docs2.id ? [leaf2] : []);
      expect(await resolveTrail(bound, [twin, restarted], children, async (id) => (id === restarted.id ? 'synthetic/root-1' : 'synthetic/root-2'))).toEqual([{ ...restarted, root: 'synthetic/root-1' }, docs2, leaf2]);
    });

    it('keeps a root that was renamed, and falls back to the name when the saved root was never bound', async () => {
      const renamed = { id: root.id, name: 'Renamed' };
      expect(await resolveTrail(bound, [renamed], async () => page([]), rootOf)).toEqual([{ ...renamed, root: 'synthetic/root-1' }]);
      expect(await resolveTrail(saved, [root], async () => page([]), rootOf)).toEqual([root]);
    });

    it('skips a starting point the engine no longer knows, but surfaces any other failure', async () => {
      const lookup = async (id: string) => { if (id === twin.id) throw boom('not_found'); return 'synthetic/root-1'; };
      expect(await resolveTrail(bound, [twin, root], async () => page([]), lookup)).toEqual([{ ...root, root: 'synthetic/root-1' }]);
      await expect(resolveTrail(bound, [root], async () => page([]), async () => { throw boom('internal_error'); })).rejects.toMatchObject({ code: 'internal_error' });
    });
  });
});
