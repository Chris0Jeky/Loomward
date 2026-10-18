// The mock additions the Atlas and Observatory views rely on (lane L10). L13's are in mock-views.test.ts.
import { describe, expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';

const now = () => new Date('2026-10-01T00:00:00Z');
const client = () => new Client(createMockTransport({ now }));

describe('mock slices for the atlas', () => {
  it('does not confuse a node index with an unrelated revoked root ID', async () => {
    const c = client();
    const before = (await c.call('node.inspect', { node_id: 'nd_9' })).result;
    expect(before.root_id).toBe('rt_mock_1');
    await c.call('roots.revoke', { root_id: 'rt_mock_9', purge_catalog: true });
    expect((await c.call('node.inspect', { node_id: 'nd_9' })).result).toEqual(before);
  });
  it.each(['revoke', 'purge', 'forget', 'grant'])('excludes an inactive root from catalogue traversal and direct reads: %s', async (via) => {
    const c = client();
    const request = { anchor: { kind: 'atlas' as const }, depth: 8, max_nodes: 6000, min_share: 0, basis: 'logical' as const, include_files: true };
    const before = (await c.call('tree.slice', request)).result;
    const alpha = before.nodes.find(n => n.name === 'Synthetic Alpha')!;
    const descendant = before.nodes.find(n => n.parent === before.nodes.indexOf(alpha))!;
    const beta = before.nodes.find(n => n.name === 'Synthetic Beta')!;
    if (via === 'grant') await c.call('grants.revoke', { grant_id: 'gr_mock_root_0' });
    else await c.call('roots.revoke', { root_id: 'rt_mock_1', purge_catalog: via === 'purge' });
    if (via === 'forget') await c.call('roots.forget', { root_id: 'rt_mock_1' });
    const after = (await c.call('tree.slice', request)).result;
    expect(after.nodes.some(n => n.node_id === alpha.node_id || n.node_id === descendant.node_id)).toBe(false);
    expect(after.root_generations.map(r => r.root_id)).not.toContain('rt_mock_1');
    expect(after.nodes[0]?.logical_bytes).toBe(beta.logical_bytes);
    expect(after.nodes[0]?.allocated_bytes).toBe(beta.allocated_bytes);
    expect(after.nodes[0]?.size_unknown_files).toBe(beta.size_unknown_files);
    expect(after.nodes[0]?.files).toBe(beta.files);
    expect(after.nodes[0]?.dirs).toBe(beta.dirs + 1);
    expect(after.nodes[0]?.child_count).toBe(1);
    expect((await c.call('tree.children', { node_id: 'nd_0', sort: 'name_asc', basis: 'logical', limit: 100, cursor: null })).result.items.map(n => n.node_id)).toEqual([beta.node_id]);
    for (const node_id of [alpha.node_id, descendant.node_id]) {
      await expect(c.call('node.inspect', { node_id })).rejects.toMatchObject({ code: 'not_found' });
      await expect(c.call('tree.children', { node_id, sort: 'name_asc', basis: 'logical', limit: 100, cursor: null })).rejects.toMatchObject({ code: 'not_found' });
      await expect(c.call('tree.slice', { ...request, anchor: { kind: 'node', node_id } })).rejects.toMatchObject({ code: 'not_found' });
      await expect(c.call('threads.meaning', { node_ids: [node_id] })).rejects.toMatchObject({ code: 'not_found' });
    }
    await expect(c.call('tree.slice', { ...request, anchor: { kind: 'root', root_id: 'rt_mock_1' } })).rejects.toMatchObject({ code: 'not_found' });
  });
  it('marks slices under the scanning root provisional, and the rest consistent', async () => {
    const c = client();
    const top = (await c.call('tree.slice', { anchor: { kind: 'atlas' }, depth: 1, max_nodes: 16, min_share: 0, basis: 'logical', include_files: false })).result;
    expect(top.aggregate_state).toBe('consistent');
    const beta = top.nodes.find((n) => n.name === 'Synthetic Beta')!;
    const b = (await c.call('tree.slice', { anchor: { kind: 'node', node_id: beta.node_id }, depth: 2, max_nodes: 200, min_share: 0, basis: 'logical', include_files: true })).result;
    expect(b.aggregate_state).toBe('provisional_live');
    expect(b.ordering).toBe('approximate_live');
    expect(b.nodes.every((n) => n.live)).toBe(true);
  });

  it('weaves synthetic threads: labels, tiers, unknown residency and denied permission all occur', async () => {
    const { result } = await client().call('tree.slice', { anchor: { kind: 'atlas' }, depth: 8, max_nodes: 6000, min_share: 0, basis: 'logical', include_files: true });
    const states = new Set(result.nodes.map((n) => n.threads.meaning.state));
    for (const s of ['labelled', 'suggested', 'mixed', 'unknown']) expect(states.has(s as never)).toBe(true);
    const tiers = new Set(result.nodes.map((n) => n.threads.residency.tier));
    for (const t of [0, 1, 2, null]) expect(tiers.has(t)).toBe(true);
    expect(result.nodes.some((n) => n.threads.permission.state === 'denied')).toBe(true);
    expect(result.nodes.some((n) => n.threads.permission.reason === 'cloud_placeholder')).toBe(true);
  });
});

describe('mock telemetry', () => {
  it('serves synthetic samples whose rates are null until two samples exist', async () => {
    const c = client();
    const first = (await c.call('telemetry.snapshot', { channels: ['system', 'gpu', 'disks'] })).result;
    expect(first.elapsed_ms).toBeNull();
    expect(first.system!.cpu.busy_fraction).toBeNull();
    expect(first.disks!.disks.every((d) => d.read_bytes_per_s === null)).toBe(true);
    expect(first.gpu!.adapters[0]!.engine_busy_fraction).toBeNull();
    expect(first.engine).not.toBeNull(); // the view mock's engine channel survives the gpu and disk layer
    const second = (await c.call('telemetry.snapshot', { channels: ['system', 'gpu', 'disks'] })).result;
    expect(second.sample_seq).toBe(2);
    expect(second.system!.cpu.busy_fraction).toBeGreaterThanOrEqual(0);
    expect(second.system!.memory.total_bytes).toMatch(/^\d+$/);
    expect(second.gpu!.adapters[0]!.engine_busy_fraction).toBeGreaterThanOrEqual(0);
  });
});
