import { describe, expect, it } from 'vitest';
import { collectionCoverage, searchableCollections } from '../../src/views/explorer/collection-search';
import type { Collection, CollectionSearchPage, SessionInfoFeatures } from '../../src/lib/contracts.gen';

const human = { collection_id: 'co_1', name: 'Reading', kind: 'human' } as Collection;
const label = { collection_id: 'co_2', name: 'Labels', kind: 'label_view' } as Collection;
describe('Explorer collection scope', () => {
  it('offers only human collections after an explicit capability handshake', () => {
    expect(searchableCollections(undefined, [human])).toEqual([]);
    expect(searchableCollections({} as SessionInfoFeatures, [human])).toEqual([]);
    expect(searchableCollections({ collection_search: false } as SessionInfoFeatures, [human])).toEqual([]);
    expect(searchableCollections({ collection_search: true } as SessionInfoFeatures, [human, label])).toEqual([human]);
  });
  it('counts examined memberships across empty budget pages without calling them matches', () => {
    const first = collectionCoverage({ total_members: 4000, examined_members: 2000, unavailable_members: 17, partial: true } as CollectionSearchPage);
    expect(first).toEqual({ total: 4000, examined: 2000, unavailable: 17, partial: true });
    expect(collectionCoverage({ total_members: 4000, examined_members: 25, unavailable_members: 0, partial: false } as CollectionSearchPage, first))
      .toEqual({ total: 4000, examined: 2025, unavailable: 17, partial: true });
    expect(collectionCoverage({ total_members: 10, examined_members: 10, unavailable_members: 0, partial: false } as CollectionSearchPage))
      .toEqual({ total: 10, examined: 10, unavailable: 0, partial: false });
  });
});
