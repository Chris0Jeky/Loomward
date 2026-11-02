import type { Collection, CollectionSearchPage, SessionInfoFeatures } from '../../lib/contracts.gen';

/** The absence of the additive capability on older engines is a refusal, never an all-roots fallback. */
export function searchableCollections(features: SessionInfoFeatures | undefined, collections: Collection[]): Collection[] {
  return features?.collection_search === true ? collections.filter(c => c.kind === 'human') : [];
}

export interface CollectionCoverage { total: number; examined: number; unavailable: number; partial: boolean }
export function collectionCoverage(page: CollectionSearchPage, previous?: CollectionCoverage | null): CollectionCoverage {
  return { total: page.total_members, examined: (previous?.examined ?? 0) + page.examined_members,
    unavailable: (previous?.unavailable ?? 0) + page.unavailable_members, partial: !!previous?.partial || page.partial };
}
