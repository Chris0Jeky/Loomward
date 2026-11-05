import { describe, expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import type { RequestEnvelope } from '../../src/lib/contracts.gen';

const filters = { text: '', extension: null, min_bytes: null, max_bytes: null, modified_after: null, modified_before: null, kind: 'file' as const, limit: 1, cursor: null };
const transport = () => createMockTransport({ now: () => new Date('2026-10-01T00:00:00Z') });

describe('owner-local collection search', () => {
  it('advertises direct-member search while keeping federated search off', async () => {
    const c = new Client(transport());
    const features = (await c.call('session.hello', {})).result.features;
    expect(features.federated_search).toBe(false);
    expect((features as unknown as Record<string, unknown>).collection_search).toBe(true);
  });

  it('narrows to direct members before the page limit', async () => {
    const t = transport();
    const c = new Client(t);
    const all = (await c.call('search.query', { root_id: null, ...filters, limit: 10 })).result.hits;
    expect(all.length).toBeGreaterThan(2);
    const member = all.at(-1)!.row;
    const collection_id = (await c.call('collections.create', { name: 'Reading' })).result.collection_id;
    const { meta } = await c.call('collections.members', { collection_id, limit: 1, cursor: null });
    await c.call('collections.update_members', { collection_id, add: [member.node_id], remove: [] }, { expectedStateRev: meta.state_rev! });
    const response = await t.call({ protocol: 'loomward/3', request_id: 'search_collection', command: 'collections.search', payload: { collection_id, ...filters } } as unknown as RequestEnvelope);
    expect(response.ok).toBe(true);
    if (!response.ok) throw new Error(response.error.code);
    expect(response.result).toMatchObject({ collection_id, hits: [{ row: { node_id: member.node_id } }], next_cursor: null });
  });
});

describe('collection paging boundaries', () => {
  async function fixture() {
    const t = transport(), c = new Client(t);
    const rows = (await c.call('search.query', { root_id: null, ...filters, limit: 100 })).result.hits.map(h => h.row);
    const a = (await c.call('collections.create', { name: 'A' })).result.collection_id;
    const b = (await c.call('collections.create', { name: 'B' })).result.collection_id;
    const rev = (await c.call('collections.members', { collection_id: a, limit: 1, cursor: null })).meta.state_rev!;
    await c.call('collections.update_members', { collection_id: a, add: rows.slice(0, 3).map(r => r.node_id), remove: [] }, { expectedStateRev: rev });
    const search = (collection_id = a, extra: Record<string, unknown> = {}) => t.call({ protocol: 'loomward/3', request_id: `cs_${Math.random()}`, command: 'collections.search', payload: { collection_id, ...filters, ...extra } } as unknown as RequestEnvelope);
    return { c, t, a, b, rows, search };
  }
  it('matches short case-insensitive literal names and reports honest membership counts', async () => {
    const { rows, search } = await fixture();
    const result = await search(undefined, { text: rows[2]!.name.toUpperCase(), limit: 100 });
    expect(result.ok).toBe(true);
    if (!result.ok) throw new Error(result.error.code);
    expect(result.result).toMatchObject({ total_members: 3, examined_members: 3, unavailable_members: 0, hits: [{ row: { node_id: rows[2]!.node_id } }], next_cursor: null });
    const literal = await search(undefined, { text: '%_', limit: 100 });
    expect(literal.ok && literal.result.hits).toEqual([]);
  });
  it('fences cursors by collection, filters, revision and session', async () => {
    const { a, b, c, rows, search } = await fixture();
    const first = await search();
    expect(first.ok).toBe(true);
    if (!first.ok) throw new Error(first.error.code);
    const cursor = first.result.next_cursor;
    expect(cursor).toBeTypeOf('string');
    for (const response of [await search(b, { cursor }), await search(a, { cursor, text: 'different' })]) {
      expect(!response.ok && response.error.code).toBe('invalid_request');
    }
    const rev = (await c.call('collections.members', { collection_id: a, limit: 1, cursor: null })).meta.state_rev!;
    await c.call('collections.update_members', { collection_id: a, add: [], remove: [rows[0]!.node_id] }, { expectedStateRev: rev });
    const stale = await search(a, { cursor });
    expect(!stale.ok && stale.error.code).toBe('stale_generation');
    const invalid = await search('co_999999');
    expect(!invalid.ok && invalid.error.code).toBe('not_found');
  });
});
