// SliceNav (lane L10): the breadcrumb only moves when the slice for it has arrived.
import { describe, expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { session } from '../../src/lib/stores/session.svelte';
import { SliceNav } from '../../src/views/atlas/shared.svelte';

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
