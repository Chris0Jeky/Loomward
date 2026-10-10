import { describe, expect, it } from 'vitest';
import { Client, LoomwardError } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { HOSTILE_NAMES } from '../../src/lib/transport/synth';
import type { EntryPage, EntryRow } from '../../src/lib/contracts.gen';

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

  describe('revision fields (semantics.md section 5)', () => {
    it('stamps responses with the scopes they were read at, null where the command touches neither', async () => {
      const c = client();
      expect((await slice(c, 16)).meta).toMatchObject({ catalog_rev: '1', state_rev: '1' }); // threads read state.db
      expect((await c.call('tree.children', { node_id: 'nd_1', sort: 'size_desc', basis: 'logical', limit: 5, cursor: null })).meta).toMatchObject({ catalog_rev: '1', state_rev: null });
      expect((await c.call('volumes.list', {})).meta).toMatchObject({ catalog_rev: null, state_rev: '1' });
      expect((await c.call('processes.list', { sort: 'name_asc', limit: 5 })).meta).toMatchObject({ catalog_rev: null, state_rev: null });
    });

    it('names the root generations a slice was read at', async () => {
      const { result } = await slice(client(), 16);
      expect(result.root_generations.length).toBeGreaterThan(0);
      expect(result.root_generations.every((g) => g.generation === '1' && /^rt_mock_\d+$/.test(g.root_id))).toBe(true);
    });

    it('moves state_rev only when a revocation commits, and reports the committed revision', async () => {
      const c = client();
      const revoke = () => c.call('grants.revoke', { grant_id: 'gr_mock_teacher_0' });
      expect((await revoke()).meta.state_rev).toBe('2');
      expect((await revoke()).meta.state_rev).toBe('2'); // monotone: revoking again changes nothing
      expect((await c.call('grants.list', {})).meta.state_rev).toBe('2');
      expect((await c.call('roots.revoke', { root_id: 'rt_mock_1', purge_catalog: false })).meta.state_rev).toBe('3');
      expect((await c.call('roots.list', {})).meta).toMatchObject({ catalog_rev: '1', state_rev: '3' });
    });

    it('opens the stream with a stream.hello that names the epoch; a resume from another epoch gets epoch_changed', async () => {
      const t = createMockTransport({ now });
      const seen: string[] = [];
      const run = async (resume?: { epoch: string; seq: number }) => {
        seen.length = 0;
        const stop = t.subscribe((e) => seen.push(`${e.event}:${e.epoch}`), () => {}, resume);
        await Promise.resolve();
        stop();
      };
      await run();
      expect(seen).toEqual(['stream.hello:e_mock_epoch']);
      // StreamEpoch in contracts/v3/view-service.schema.json.
      expect(seen[0]?.split(':')[1]).toMatch(/^[A-Za-z0-9_-]{8,64}$/);
      await run({ epoch: 'e_mock_epoch', seq: 0 });
      expect(seen).toEqual(['stream.hello:e_mock_epoch']);
      await run({ epoch: 'e_old', seq: 9 });
      expect(seen).toEqual(['stream.hello:e_mock_epoch', 'stream.lagged:e_mock_epoch']);
    });
  });
});
