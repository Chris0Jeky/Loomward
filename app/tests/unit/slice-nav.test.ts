// SliceNav (lane L10): the breadcrumb only moves when the slice for it has arrived.
import { describe, expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { session } from '../../src/lib/stores/session.svelte';
import { SliceNav, pathTo } from '../../src/views/atlas/shared.svelte';
import type { NodeInfo } from '../../viz/types.js';

describe('SliceNav', () => {
  it('a failed drill leaves the trail where it was and drops the slice; a good one commits both', async () => {
    session.client = new Client(createMockTransport({ now: () => new Date('2026-10-01T00:00:00Z') }));
    const nav = new SliceNav({ depth: 2, maxNodes: 200, minShare: 0 });
    expect(await nav.start()).toBe(true);
    const before = nav.trail.map((c) => c.id);
    expect(nav.slice).not.toBeNull();

    expect(await nav.drill('nd_does_not_exist', 'ghost')).toBe(false);
    expect(nav.trail.map((c) => c.id)).toEqual(before);
    expect(nav.slice).toBeNull();
    expect(nav.error).not.toBe('');

    const ok = await nav.load();
    expect(ok).toBe(true);
    const child = nav.slice!.nodes.find((n) => n.parent === 0)!;
    expect(await nav.drill(child.node_id, child.name)).toBe(true);
    expect(nav.trail.at(-1)!.id).toBe(child.node_id);
    expect(nav.slice!.anchor_node_id).toBe(child.node_id);
    expect(nav.error).toBe('');

    expect(await nav.setBasis('allocated')).toBe(true);
    expect(nav.basis).toBe('allocated');
    expect(nav.slice!.basis).toBe('allocated');
    session.client = null;
  });
});

describe('SliceNav retry', () => {
  it('load() recovers a first load that failed: the trail is empty, so it starts at the root', async () => {
    const real = new Client(createMockTransport({ now: () => new Date('2026-10-01T00:00:00Z') }));
    let down = true;
    session.client = { call: (...a: Parameters<Client['call']>) => (down ? Promise.reject(new Error('engine down')) : real.call(...a)) } as unknown as Client;
    const nav = new SliceNav({ depth: 2, maxNodes: 200, minShare: 0 });
    expect(await nav.start()).toBe(false);
    expect(nav.trail).toEqual([]);
    expect(nav.error).not.toBe('');
    down = false;
    expect(await nav.load()).toBe(true);
    expect(nav.trail).toHaveLength(1);
    expect(nav.slice).not.toBeNull();
    expect(nav.error).toBe('');
    session.client = null;
  });
});

describe('pathTo', () => {
  const info = (id: string, name: string, parentId: string | null) => ({ id, name, parentId }) as NodeInfo;
  const all = new Map([['r', info('r', 'Root', null)], ['a', info('a', 'A', 'r')], ['b', info('b', 'B', 'a')], ['c', info('c', 'C', 'b')]]);
  const lookup = (id: string) => all.get(id) ?? null;

  it('lists every ancestor of a deep descendant, not just the trail and the node', () => {
    expect(pathTo(all.get('c')!, lookup, ['Elsewhere', 'Root'])).toEqual(['Elsewhere', 'Root', 'A', 'B', 'C']);
  });
  it('does not repeat the slice root when the node is the root', () => {
    expect(pathTo(all.get('r')!, lookup, ['All roots'])).toEqual(['All roots']);
  });
  it('a synthetic cell (not in the lookup) still gets its parents', () => {
    expect(pathTo({ ...info('b#rest', 'Not in this slice', 'b') } as NodeInfo, lookup, ['Root'])).toEqual(['Root', 'A', 'B', 'Not in this slice']);
  });
});
