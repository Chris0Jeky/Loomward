import { describe, expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { loadFootprint } from '../../src/views/collections/footprint';

describe('explicit project discovery', () => {
  it('explains an existing mock project without collections or grant changes and refuses a revoked root', async () => {
    const client = new Client(createMockTransport());
    const root = (await client.call('roots.list', {})).result.roots[0]!;
    const page = (await client.call('projects.find', { root_id: root.root_id, limit: 50, cursor: null })).result;
    const node = page.candidates[0]!.node_id;
    const grants = (await client.call('grants.list', {})).result;
    const brief = await loadFootprint(client, node, root.root_id, page.generation!, new AbortController().signal);
    expect(brief.detail.row.node_id).toBe(node);
    expect(brief.families.by).toBe('ext_family');
    expect(brief.children.items.length).toBeLessThanOrEqual(12);
    expect((await client.call('grants.list', {})).result).toEqual(grants);
    expect((await client.call('collections.list', {})).result.collections).toEqual([]);
    await client.call('roots.revoke', { root_id: root.root_id, purge_catalog: false });
    await expect(loadFootprint(client, node, root.root_id, page.generation!, new AbortController().signal)).rejects.toMatchObject({ code: 'not_found' });
  });
  it('pages without creating collections and binds the cursor to root and state', async () => {
    const client = new Client(createMockTransport());
    const roots = (await client.call('roots.list', {})).result.roots;
    const root_id = roots[0]!.root_id;
    const first = (await client.call('projects.find', { root_id, limit: 1, cursor: null })).result;
    expect(first.candidates).toHaveLength(1);
    expect(first.candidates[0]!.markers).toHaveLength(2);
    expect(first.next_cursor).not.toBeNull();
    const next = (await client.call('projects.find', { root_id, limit: 1, cursor: first.next_cursor })).result;
    expect(next.candidates[0]!.node_id).not.toBe(first.candidates[0]!.node_id);
    expect((await client.call('collections.list', {})).result.collections).toHaveLength(0);
    await expect(client.call('projects.find', { root_id: roots[1]!.root_id, limit: 1, cursor: first.next_cursor })).rejects.toMatchObject({ code: 'invalid_request' });
    await client.call('collections.create', { name: 'Projects' });
    await expect(client.call('projects.find', { root_id, limit: 1, cursor: first.next_cursor })).rejects.toMatchObject({ code: 'stale_generation' });
  });
  it('inspects an existing node and adds only with both current preconditions', async () => {
    const client = new Client(createMockTransport());
    const root_id = (await client.call('roots.list', {})).result.roots[0]!.root_id;
    const page = (await client.call('projects.find', { root_id, limit: 50, cursor: null })).result;
    const node_id = page.candidates[0]!.node_id;
    expect((await client.call('node.inspect', { node_id }, { expectedGeneration: page.generation! })).result.row.node_id).toBe(node_id);
    const collection_id = (await client.call('collections.create', { name: 'Projects' })).result.collection_id;
    const revision = (await client.call('collections.members', { collection_id, limit: 1, cursor: null })).meta.state_rev!;
    const payload = { collection_id, add: [node_id], remove: [] };
    await expect(client.call('collections.update_members', payload, { expectedStateRev: revision, expectedGeneration: '0' })).rejects.toMatchObject({ code: 'stale_generation' });
    expect((await client.call('collections.members', { collection_id, limit: 1, cursor: null })).result.total).toBe(0);
    const added = await client.call('collections.update_members', payload, { expectedStateRev: revision, expectedGeneration: page.generation! });
    expect(added.result.member_count).toBe(1);
    await expect(client.call('collections.update_members', payload, { expectedStateRev: revision, expectedGeneration: page.generation! })).rejects.toMatchObject({ code: 'stale_generation' });
  });
});
