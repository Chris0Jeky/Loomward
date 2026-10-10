// Explorer stale-breadcrumb fallback decision (issue #198).
// Node IDs are bound to the engine session and the catalogue instance: after an engine
// restart, a catalogue rebuild or a resync, a saved breadcrumb ID answers `not_found`.
// That case falls back to the starting points; a `not_found` for a node the user just
// clicked (not a saved breadcrumb) keeps today's error behaviour.
import { LoomwardError } from '../../lib/transport/client';

export const FALLBACK_NOTICE = 'The catalogue changed, so the view went back to the top level.';

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
