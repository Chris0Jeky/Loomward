// Explorer stale-breadcrumb fallback decision (issue #198).
// Node IDs are bound to the engine session and the catalogue instance: after an engine
// restart, a catalogue rebuild or a resync, a saved breadcrumb ID answers `not_found`.
// That case falls back to the deepest ancestor that still lists (or the starting points when
// none does) and says so; a `not_found` for a node the user just clicked (not a saved
// breadcrumb) keeps today's error behaviour.
import { LoomwardError } from '../../lib/transport/client';

export type RefetchDecision = 'fallback' | 'error';

/**
 * Decide what a refetch failure means. `isSavedBreadcrumb` is true when the failing call
 * used a saved breadcrumb ID: a refetch during a resync or epoch change, or any later call
 * made with that saved ID. Any `not_found` reason falls back; every other code, and every
 * non-engine error, stays an error.
 */
export function onRefetchError(error: unknown, isSavedBreadcrumb: boolean): RefetchDecision {
  if (isSavedBreadcrumb && error instanceof LoomwardError && error.code === 'not_found') return 'fallback';
  return 'error';
}

export interface Fallback<C> {
  /** The breadcrumb to try next: the failing one minus its last crumb. Empty means reload the starting points. */
  keep: C[];
  /** Name of the deepest folder that vanished; it stays the same while the walk climbs, so the notice names what the reader lost. */
  lost: string;
}

/** One step up from a stale breadcrumb. `lost` carries the first (deepest) name through the steps. */
export function fallbackStep<C extends { name: string }>(trail: C[], lost?: string): Fallback<C> {
  return { keep: trail.slice(0, -1), lost: lost ?? trail[trail.length - 1]!.name };
}
