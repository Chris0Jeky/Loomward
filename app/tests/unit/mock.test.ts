import { describe, expect, it } from 'vitest';
import { Client, LoomwardError } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { HOSTILE_NAMES } from '../../src/lib/transport/synth';
import type { EntryPage, EntryRow } from '../../src/lib/types';

const now = () => new Date('2026-10-01T00:00:00Z');
const client = (o = {}) => new Client(createMockTransport({ now, ...o }));
const slice = (c: Client, max = 6000) =>
  c.call('tree.slice', { anchor: { kind: 'atlas' }, depth: 8, max_nodes: max, min_share: 0, basis: 'logical', include_files: true });

describe('mock transport', () => {
  it('is deterministic for a seed and differs across seeds', async () => {
    const a = JSON.stringify((await slice(client())).result);
    expect(JSON.stringify((await slice(client())).result)).toBe(a);
    expect(JSON.stringify((await slice(client({ seed: 2 }))).result)).not.toBe(a);
  });

  it('serves a 6,000-node slice with parents before children and exact byte strings', async () => {
    const { result } = await slice(client());
    expect(result.nodes).toHaveLength(6000);
    result.nodes.forEach((n, i) => {
      if (i === 0) expect(n.parent).toBeNull();
      else expect(n.parent).toBeLessThan(i);
      expect(n.size_bytes).toMatch(/^(0|[1-9][0-9]*)$/);
      expect(n.logical_bytes).toMatch(/^(0|[1-9][0-9]*)$/);
      expect(n.name.length).toBeLessThanOrEqual(260);
    });
    const di = result.nodes.findIndex((n) => n.kind === 'dir' && n.child_count);
    const kids = result.nodes.filter((n) => n.parent === di);
    expect(kids.reduce((s, k) => s + BigInt(k.logical_bytes), 0n)).toBe(BigInt(result.nodes[di]!.logical_bytes));
  });

  it('honours max_nodes and flags truncation', async () => {
    const { result } = await slice(client(), 100);
    expect(result.nodes).toHaveLength(100);
    expect(result.truncated).toBe(true);
    expect(result.complete).toBe(false);
  });

  it('keeps unknown allocation unknown', async () => {
    const { result } = await slice(client());
    expect(result.nodes.some((n) => n.allocated_bytes === null && n.size_unknown_files > 0)).toBe(true);
  });

  it('pages children without gaps or repeats, and rejects a forged cursor', async () => {
    const c = client();
    const { result: top } = await c.call('tree.slice', { anchor: { kind: 'atlas' }, depth: 1, max_nodes: 16, min_share: 0, basis: 'logical', include_files: false });
    const alpha = top.nodes.find((n) => n.name === 'Synthetic Alpha')!;
    const seen: EntryRow[] = [];
    let cursor: string | null = null;
    do {
      const page: EntryPage = (await c.call('tree.children', { node_id: alpha.node_id, sort: 'name_asc', basis: 'logical', limit: 7, cursor })).result;
      seen.push(...page.items);
      cursor = page.next_cursor;
    } while (cursor);
    expect(seen.length).toBe(alpha.child_count);
    expect(new Set(seen.map((r) => r.node_id)).size).toBe(seen.length);
    for (const h of HOSTILE_NAMES) expect(seen.some((r) => r.name === h)).toBe(true);
    await expect(c.call('tree.children', { node_id: alpha.node_id, sort: 'name_asc', basis: 'logical', limit: 7, cursor: 'forged!' })).rejects.toMatchObject({ code: 'invalid_request' });
  });

  it('searches names with a location hint', async () => {
    const { result } = await client().call('search.query', { root_id: null, text: 'SCRIPT', extension: null, min_bytes: null, kind: 'any', limit: 10, cursor: null });
    expect(result.items.some((r) => r.name.startsWith('<script>'))).toBe(true);
    expect(result.items[0]!.location_hint).not.toBeNull();
  });

  it('reports dataset class synthetic, all effects off, and no pretend features', async () => {
    const { result, meta } = await client().call('session.hello', {});
    expect(meta.dataset_class).toBe('synthetic');
    expect(result.dataset_class).toBe('synthetic');
    expect(Object.values(result.capabilities.effects).every((v) => v === false)).toBe(true);
    expect({ ...result.features, telemetry_available: false, gpu_available: false }).toEqual(Object.fromEntries(Object.keys(result.features).map((k) => [k, false]))); // only (synthetic) telemetry, GPU included, is served
  });

  it('answers unimplemented commands with an error, never invented data', async () => {
    const err = await client().call('learning.status' as never, {} as never).catch((e) => e);
    expect(err).toBeInstanceOf(LoomwardError);
    expect(err.code).toBe('capability_unavailable');
  });
});
