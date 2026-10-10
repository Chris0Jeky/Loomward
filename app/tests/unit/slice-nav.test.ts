// SliceNav (lane L10): the breadcrumb only moves when the slice for it has arrived.
import { describe, expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { session } from '../../src/lib/stores/session.svelte';
import { SliceNav, pathTo } from '../../src/views/atlas/shared.svelte';
import type { NodeInfo, SliceLike } from '../../viz/types.js';

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
  /** A slice from [id, name, parent index] triples; index 0 is the anchor. */
  const slice = (...rows: [string, string, number | null][]) => ({ nodes: rows.map(([node_id, name, parent]) => ({ node_id, name, parent })) }) as unknown as SliceLike;
  const real = (id: string, name: string, parentId: string | null) => ({ id, name, parentId, src: {} }) as unknown as NodeInfo;
  const deep = slice(['r', 'Root', null], ['a', 'A', 0], ['b', 'B', 1], ['c', 'C', 2]);

  it('lists every ancestor of a deep descendant, from the slice parents', () => {
    expect(pathTo(real('c', 'C', 'b'), deep, ['Elsewhere', 'Root'])).toEqual(['Elsewhere', 'Root', 'A', 'B', 'C']);
  });
  it('does not repeat the slice root when the node is the root', () => {
    expect(pathTo(real('r', 'Root', null), deep, ['All roots'])).toEqual(['All roots']);
  });
  it('a synthetic cell (not in the slice) is named after the real node it sits in', () => {
    expect(pathTo({ id: 'b#rest', name: 'Not in this slice', parentId: 'b', src: null }, deep, ['Root'])).toEqual(['Root', 'A', 'B', 'Not in this slice']);
  });
  it('click then drill: a node selected before it became the anchor is not repeated (the stale info says its parent is elsewhere)', () => {
    const afterDrill = slice(['x', 'X', null], ['y', 'Y', 0]);
    const staleX = real('x', 'X', 'p'); // taken from the previous slice, where X had a parent
    expect(pathTo(staleX, afterDrill, ['All roots', 'P', 'X'])).toEqual(['All roots', 'P', 'X']);
    expect(pathTo(real('y', 'Y', 'x'), afterDrill, ['All roots', 'P', 'X'])).toEqual(['All roots', 'P', 'X', 'Y']);
  });
  it('a node the slice does not hold is named under its parent if the slice has it, else under the trail, never invented parents', () => {
    expect(pathTo(real('gone', 'Gone', 'b'), deep, ['Root'])).toEqual(['Root', 'A', 'B', 'Gone']);
    expect(pathTo(real('gone', 'Gone', 'elsewhere'), deep, ['Root'])).toEqual(['Root', 'Gone']);
    expect(pathTo(real('x', 'X', null), null, ['Root'])).toEqual(['Root', 'X']);
  });
  it('cycle guard: a parent at or after its child, or a second root, ends the walk', () => {
    const selfLoop = slice(['r', 'Root', null], ['a', 'A', 1], ['b', 'B', 2]);
    expect(pathTo(real('b', 'B', 'a'), selfLoop, ['T'])).toEqual(['T', 'B']);
    const forward = slice(['r', 'Root', null], ['a', 'A', 2], ['b', 'B', 1]);
    expect(pathTo(real('a', 'A', 'b'), forward, ['T'])).toEqual(['T', 'A']);
    const twoRoots = slice(['r', 'Root', null], ['s', 'S', null], ['c', 'C', 1]);
    expect(pathTo(real('c', 'C', 's'), twoRoots, ['T'])).toEqual(['T', 'S', 'C']);
  });
  it('duplicate node ids still terminate and stay bounded by the slice size', () => {
    const dup = slice(['r', 'Root', null], ['d', 'D1', 0], ['d', 'D2', 1], ['d', 'D3', 2]);
    const path = pathTo(real('d', 'D', null), dup, ['T']);
    expect(path.length).toBeLessThanOrEqual(1 + dup.nodes.length);
  });
});
