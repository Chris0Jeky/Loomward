import type { Collection, CollectionMemberPage, EntryRow, ErrorCode, RequestEnvelope } from '../contracts.gen';
import type { SynthNode } from './synth';

type Fail = (code: ErrorCode, message: string, detail?: Record<string, unknown> | null) => never;
export function createCollectionMock(
  now: () => Date, fail: Fail, nodeFor: (id: unknown) => SynthNode,
  entryRow: (node: SynthNode, hint: boolean) => EntryRow,
  revoked: (node: SynthNode) => boolean, baseRev: () => string,
) {
  let revision = 0;
  const collections = new Map<string, { collection: Collection; nodes: Set<string> }>();
  const cursors = new Map<string, { id: string; revision: string; after: number }>();
  const stateRev = () => String(BigInt(baseRev()) + BigInt(revision));
  const get = (id: unknown) => collections.get(String(id)) ?? fail('not_found', 'Unknown collection.');
  const summary = (c: ReturnType<typeof get>): Collection => ({ ...c.collection, member_count: c.nodes.size,
    logical_bytes: [...c.nodes].some((id) => revoked(nodeFor(id))) ? null : [...c.nodes].reduce((sum, id) => sum + nodeFor(id).logical, 0n).toString() });
  function handle(req: RequestEnvelope): unknown {
    const p = req.payload;
    const keys: Record<string, string[]> = { 'collections.list': [], 'collections.create': ['name'], 'collections.members': ['collection_id', 'limit', 'cursor'], 'collections.update_members': ['collection_id', 'add', 'remove'] };
    if (Object.keys(p).some((key) => !keys[req.command]?.includes(key))) fail('invalid_request', 'Unknown collection field.');
    if (req.command === 'collections.list') return { collections: [...collections.values()].map(summary) };
    if (req.command === 'collections.create') {
      const name = p.name;
      if (typeof name !== 'string' || [...name].length < 1 || [...name].length > 64 || /[\u0000-\u001f]/u.test(name)) fail('invalid_request', 'Use a name of 1–64 characters without controls.');
      const duplicate = [...collections.values()].find((c) => c.collection.name === name);
      if (duplicate) fail('invalid_request', 'A collection with this name already exists.', { existing_id: duplicate.collection.collection_id });
      if (collections.size >= 256) fail('resource_budget', 'The 256 collection limit was reached.');
      const collection: Collection = { collection_id: `co_${collections.size + 1}`, name: name as string, kind: 'human', member_count: 0, logical_bytes: '0', created_at: now().toISOString() };
      collections.set(collection.collection_id, { collection, nodes: new Set() }); revision++;
      return collection;
    }
    const c = get(p.collection_id);
    if (req.command === 'collections.update_members') {
      if (req.expected_state_rev === undefined) fail('invalid_request', 'expected_state_rev is required.');
      if (req.expected_state_rev !== stateRev()) fail('stale_generation', 'State changed; read members again.', { reason: 'state_rev_changed' });
      const { add, remove } = p;
      for (const ids of [add, remove]) if (!Array.isArray(ids) || ids.length > 500 || new Set(ids).size !== ids.length) fail('invalid_request', 'Members must be unique bounded arrays.');
      const adds = add as string[], removes = remove as string[];
      if (adds.some((id) => removes.includes(id))) fail('invalid_request', 'Adds and removes overlap.');
      for (const id of [...adds, ...removes]) {
        const n = nodeFor(id);
        if (n.kind === 'atlas' || n.kind === 'root') fail('capability_unavailable', 'Durable identity unavailable.');
        if (revoked(n)) fail('not_found', 'The root is revoked.');
      }
      const before = [...c.nodes].sort().join(',');
      adds.forEach((id) => c.nodes.add(id)); removes.forEach((id) => c.nodes.delete(id));
      if ([...c.nodes].sort().join(',') !== before) revision++;
      return summary(c);
    }
    const limit = p.limit;
    if (!Number.isInteger(limit) || (limit as number) < 1 || (limit as number) > 200 || !Object.hasOwn(p, 'cursor')) fail('invalid_request', 'Invalid member page.');
    let after = 0;
    if (p.cursor !== null) {
      const cursor = cursors.get(String(p.cursor)) ?? fail('stale_generation', 'Cursor expired.', { reason: 'cursor_expired' });
      if (cursor.id !== p.collection_id) fail('invalid_request', 'Cursor belongs to another collection.');
      if (cursor.revision !== stateRev()) fail('stale_generation', 'State changed; restart paging.', { reason: 'state_rev_changed' });
      after = cursor.after;
    }
    const all = [...c.nodes].sort((a, b) => nodeFor(a).index - nodeFor(b).index);
    const ids = all.slice(after, after + (limit as number));
    const next = after + ids.length;
    const next_cursor = next < all.length ? `cu_mock_${cursors.size + 1}` : null;
    if (next_cursor) {
      if (cursors.size >= 2048) cursors.delete(cursors.keys().next().value!);
      cursors.set(next_cursor, { id: c.collection.collection_id, revision: stateRev(), after: next });
    }
    return { collection_id: c.collection.collection_id, items: ids.map((id) => { const n = nodeFor(id); const gone = revoked(n);
      return { member_id: `om_${n.index}`, status: gone ? 'root_revoked' : 'resolved', entry: gone ? null : entryRow(n, true) };
    }), total: all.length, next_cursor, budget_hit: false } satisfies CollectionMemberPage;
  }
  return { handle, stateRev };
}
