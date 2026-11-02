import type { Collection, CollectionMemberPage, CollectionSearchRequest, CollectionSearchPage, SearchHit, EntryRow, ErrorCode, RequestEnvelope } from '../contracts.gen';
import type { SynthNode } from './synth';

type Fail = (code: ErrorCode, message: string, detail?: Record<string, unknown> | null) => never;
export function createCollectionMock(
  now: () => Date, fail: Fail, nodeFor: (id: unknown) => SynthNode,
  entryRow: (node: SynthNode, hint: boolean) => EntryRow,
  revoked: (node: SynthNode) => boolean, baseRev: () => string,
  catalogRev: () => string, searchLocation: (node: SynthNode) => Pick<SearchHit, 'root_id' | 'path'>,
) {
  let revision = 0;
  const collections = new Map<string, { collection: Collection; nodes: Set<string> }>();
  const cursors = new Map<string, { id: string; revision: string; after: number }>();
  const searchCursors = new Map<string, { bind: string; revision: string; catalog: string; after: number }>();
  const stateRev = () => String(BigInt(baseRev()) + BigInt(revision));
  const get = (id: unknown) => collections.get(String(id)) ?? fail('not_found', 'Unknown collection.');
  const summary = (c: ReturnType<typeof get>): Collection => ({ ...c.collection, member_count: c.nodes.size,
    logical_bytes: [...c.nodes].some((id) => revoked(nodeFor(id))) ? null : [...c.nodes].reduce((sum, id) => sum + nodeFor(id).logical, 0n).toString() });
  function handle(req: RequestEnvelope): unknown {
    const p = req.payload;
    const keys: Record<string, string[]> = { 'collections.search': ['collection_id', 'text', 'extension', 'min_bytes', 'max_bytes', 'modified_after', 'modified_before', 'kind', 'limit', 'cursor'], 'collections.list': [], 'collections.create': ['name'], 'collections.members': ['collection_id', 'limit', 'cursor'], 'collections.update_members': ['collection_id', 'add', 'remove', 'remove_members'] };
    if (Object.keys(p).some((key) => !keys[req.command]?.includes(key))) fail('invalid_request', 'Unknown collection field.');
    if (req.command === 'collections.list') return { collections: [...collections.values()].map(summary) };
    if (req.command === 'collections.create') {
      const name = p.name;
      if (typeof name !== 'string' || [...name].length < 1 || [...name].length > 64 || /\p{Cc}/u.test(name)) fail('invalid_request', 'Use a name of 1–64 characters without controls.');
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
      const { add, remove, remove_members = [] } = p;
      for (const ids of [add, remove, remove_members]) if (!Array.isArray(ids) || ids.length > 500 || new Set(ids).size !== ids.length) fail('invalid_request', 'Members must be unique bounded arrays.');
      const adds = add as string[], removes = remove as string[];
      const removeMembers = remove_members as string[];
      if (removeMembers.some((id) => typeof id !== 'string' || !/^om_[1-9][0-9]*$/.test(id) || id.length > 64)) fail('invalid_request', 'Invalid member ID.');
      if (adds.some((id) => removes.includes(id))) fail('invalid_request', 'Adds and removes overlap.');
      for (const id of [...adds, ...removes]) {
        const n = nodeFor(id);
        if (n.kind === 'atlas' || n.kind === 'root') fail('capability_unavailable', 'Durable identity unavailable.');
        if (revoked(n)) fail('not_found', 'The root is revoked.');
        if (adds.includes(id) && removeMembers.includes(`om_${n.index}`)) fail('invalid_request', 'Adds and removes overlap.');
      }
      const before = [...c.nodes].sort().join(',');
      adds.forEach((id) => c.nodes.add(id)); removes.forEach((id) => c.nodes.delete(id));
      for (const id of c.nodes) if (removeMembers.includes(`om_${nodeFor(id).index}`)) c.nodes.delete(id);
      if ([...c.nodes].sort().join(',') !== before) revision++;
      return summary(c);
    }
    if (req.command === 'collections.search') {
      const p = req.payload as unknown as CollectionSearchRequest;
      if (typeof p.text !== 'string' || [...p.text].length > 256 || !['any', 'file', 'dir'].includes(p.kind)
        || !Number.isInteger(p.limit) || p.limit < 1 || p.limit > 100 || !Object.hasOwn(p, 'cursor')) fail('invalid_request', 'Invalid collection search.');
      for (const key of ['extension', 'min_bytes', 'max_bytes', 'modified_after', 'modified_before'] as const) {
        const value = p[key];
        if (value !== null && typeof value !== 'string') fail('invalid_request', 'Missing or invalid search filter.');
      }
      if (p.extension !== null && [...p.extension].length > 32) fail('invalid_request', 'Invalid extension.');
      for (const value of [p.min_bytes, p.max_bytes]) if (value !== null && (!/^(0|[1-9][0-9]*)$/.test(value) || BigInt(value) > 9223372036854775807n)) fail('invalid_request', 'Invalid byte count.');
      if (p.min_bytes !== null && p.max_bytes !== null && BigInt(p.min_bytes) > BigInt(p.max_bytes)) fail('invalid_request', 'Reversed size range.');
      for (const value of [p.modified_after, p.modified_before]) if (value !== null && !Number.isFinite(Date.parse(value))) fail('invalid_request', 'Invalid date.');
      if (p.modified_after !== null && p.modified_before !== null && Date.parse(p.modified_after) > Date.parse(p.modified_before)) fail('invalid_request', 'Reversed date range.');
      const bind = JSON.stringify([p.collection_id, p.text, p.extension, p.min_bytes, p.max_bytes, p.modified_after, p.modified_before, p.kind]);
      let after = 0;
      if (p.cursor !== null) {
        const cursor = searchCursors.get(p.cursor) ?? fail('stale_generation', 'Cursor expired.', { reason: 'cursor_expired' });
        if (cursor.bind !== bind) fail('invalid_request', 'Cursor belongs to another request.');
        if (cursor.revision !== stateRev() || cursor.catalog !== catalogRev()) fail('stale_generation', 'Collection or catalogue changed; restart searching.');
        after = cursor.after;
      }
      const all = [...c.nodes].map(nodeFor).sort((a, b) => a.index - b.index);
      const candidates = all.filter(n => n.index > after);
      const hits: SearchHit[] = [];
      let examined_members = 0, unavailable_members = 0, partial = false, last = after;
      for (const n of candidates.slice(0, 2000)) {
        if (hits.length === p.limit) break;
        examined_members++; last = n.index;
        if (revoked(n) || ['excluded', 'denied', 'unscanned', 'unknown'].includes(n.coverage)) { unavailable_members++; continue; }
        partial ||= n.coverage !== 'complete';
        if (!n.name.toLowerCase().includes(p.text.trim().toLowerCase()) || (p.kind !== 'any' && p.kind !== n.kind)
          || (p.extension !== null && n.extension?.toLowerCase() !== p.extension.toLowerCase())
          || (p.min_bytes !== null && n.logical < BigInt(p.min_bytes)) || (p.max_bytes !== null && n.logical > BigInt(p.max_bytes))
          || ((p.modified_after !== null || p.modified_before !== null) && (n.modified === null
            || (p.modified_after !== null && Date.parse(n.modified) < Date.parse(p.modified_after))
            || (p.modified_before !== null && Date.parse(n.modified) > Date.parse(p.modified_before))))) continue;
        hits.push({ row: entryRow(n, true), ...searchLocation(n), match_kind: p.text.trim() ? 'substring' : 'filter', highlights: [] });
      }
      const more = candidates.length > examined_members;
      const budget_hit = more && examined_members === 2000;
      const next_cursor = more ? `cu_collection_search_${searchCursors.size + 1}` : null;
      if (next_cursor) {
        if (searchCursors.size >= 2048) searchCursors.delete(searchCursors.keys().next().value!);
        searchCursors.set(next_cursor, { bind, revision: stateRev(), catalog: catalogRev(), after: last });
      }
      return { collection_id: c.collection.collection_id, hits, total_members: all.length, examined_members, unavailable_members,
        partial: partial || unavailable_members > 0, next_cursor, budget_hit } satisfies CollectionSearchPage;
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
