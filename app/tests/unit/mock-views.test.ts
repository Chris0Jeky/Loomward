// The mock additions the Atlas and Observatory views rely on (lane L10).
import { describe, expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';

const now = () => new Date('2026-10-01T00:00:00Z');
const client = () => new Client(createMockTransport({ now }));

describe('mock slices for the atlas', () => {
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
    const second = (await c.call('telemetry.snapshot', { channels: ['system', 'gpu', 'disks'] })).result;
    expect(second.sample_seq).toBe(2);
    expect(second.system!.cpu.busy_fraction).toBeGreaterThanOrEqual(0);
    expect(second.system!.memory.total_bytes).toMatch(/^\d+$/);
  });
});
