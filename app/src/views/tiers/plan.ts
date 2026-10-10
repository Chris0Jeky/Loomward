import type { PlacementPlan } from '../../lib/types.views';

export type Outcome = { plan: PlacementPlan } | { error: string };

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const isBytes = (v: unknown) => typeof v === 'string' && /^(0|[1-9][0-9]{0,19})$/.test(v);
const arrayOf = (v: unknown, each: (x: unknown) => boolean) => Array.isArray(v) && v.every(each);

/**
 * A plan is only trusted for display when it has the expected shape, says it is a simulation that
 * changed nothing, and every proposal is consent-gated and not executable. Anything else is refused
 * whole (and never throws): a malformed reply is a refused reply.
 */
export function isSound(p: unknown): p is PlacementPlan {
  if (!isObj(p)) return false;
  const proposal = (x: unknown) =>
    isObj(x) && x.executable === false && x.requires_consent === true && isBytes(x.source_bytes_relieved) && isBytes(x.destination_bytes_required) && isBytes(x.transfer_bytes)
    && typeof x.group_id === 'string' && typeof x.source_id === 'string' && typeof x.target_id === 'string';
  const reasoned = (x: unknown) => isObj(x) && typeof x.reason === 'string' && typeof x.group_id === 'string';
  return (
    p.mode === 'simulation' && p.filesystem_changed === false
    && arrayOf(p.proposals, proposal) && arrayOf(p.rejected, reasoned) && arrayOf(p.pre_rejected, reasoned)
    && arrayOf(p.excluded_volumes, (x) => isObj(x) && typeof x.reason === 'string' && typeof x.volume_id === 'string')
    && arrayOf(p.assumptions, (x) => typeof x === 'string')
    && isObj(p.projected_free_bytes) && Object.values(p.projected_free_bytes).every(isBytes)
    && isObj(p.search) && typeof p.search.complete === 'boolean' && typeof p.search.reason === 'string'
    && [p.search.nodes_visited, p.search.node_budget, p.search.eligible_groups, p.search.eligible_targets].every((n) => typeof n === 'number')
    && [p.target_free_bytes, p.shortfall_bytes, p.transfer_bytes, p.baseline_shortfall_bytes, p.shortfall_improvement_bytes].every(isBytes)
    && typeof p.satisfied === 'boolean' && typeof p.optimality_claim === 'boolean' && typeof p.optimality_scope === 'string'
    && typeof p.assumption === 'string' && typeof p.heat_policy === 'string' && typeof p.relief_policy === 'string'
  );
}

/** The first plan that may contribute figures to the page. A refused reply contributes nothing, not even a baseline. */
export function firstTrusted(outcomes: Record<string, Outcome> | null): PlacementPlan | null {
  if (!outcomes) return null;
  for (const o of Object.values(outcomes)) if (isObj(o) && 'plan' in o && isSound(o.plan)) return o.plan;
  return null;
}
