import type { PlacementPlan } from '../../lib/types.views';

export type Outcome = { plan: PlacementPlan } | { error: string };

/**
 * A plan is only trusted for display when it says it is a simulation that changed nothing and every
 * proposal is consent-gated and not executable. Anything else is refused whole.
 */
export const isSound = (p: PlacementPlan): boolean =>
  p.mode === 'simulation' && p.filesystem_changed === false && p.proposals.every((x) => x.executable === false && x.requires_consent === true);

/** The first plan that may contribute figures to the page. A refused reply contributes nothing, not even a baseline. */
export function firstTrusted(outcomes: Record<string, Outcome> | null): PlacementPlan | null {
  if (!outcomes) return null;
  for (const o of Object.values(outcomes)) if ('plan' in o && isSound(o.plan)) return o.plan;
  return null;
}
