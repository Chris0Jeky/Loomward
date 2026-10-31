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
