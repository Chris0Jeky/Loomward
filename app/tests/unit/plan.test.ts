import { describe, expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import type { PlacementPlan } from '../../src/lib/types.views';
import { firstTrusted, isSound, type Outcome } from '../../src/views/tiers/plan';

const GiB = 2 ** 30;
async function plan(): Promise<PlacementPlan> {
  const c = new Client(createMockTransport());
  return (await c.call('placement.simulate', {
    source_volume_id: 'vo_c', target_free_bytes: String(380 * GiB), max_transfer_bytes: String(400 * GiB), heat_policy: 'unknown_is_ineligible',
    relief_policy: 'verified_only', candidate_basis: 'largest_dirs', max_groups: 50, overrides: [{ group_id: 'cg_models', heat: 0.1 }], node_budget: 1000, save: false,
  })).result;
}
const bad = (p: PlacementPlan, change: (q: PlacementPlan) => void): PlacementPlan => {
  const q = structuredClone(p) as unknown as Record<string, unknown> & PlacementPlan;
  change(q);
  return q;
};

describe('placement plan trust', () => {
  it('accepts a real simulation', async () => expect(isSound(await plan())).toBe(true));

  it('refuses every way a reply can claim an effect or skip consent', async () => {
    const p = await plan();
    expect(p.proposals.length).toBeGreaterThan(0);
    expect(isSound(bad(p, (q) => ((q as { filesystem_changed: boolean }).filesystem_changed = true)))).toBe(false);
    expect(isSound(bad(p, (q) => ((q as { mode: string }).mode = 'execution')))).toBe(false);
    expect(isSound(bad(p, (q) => ((q.proposals[0] as { executable: boolean }).executable = true)))).toBe(false);
    expect(isSound(bad(p, (q) => ((q.proposals[0] as { requires_consent: boolean }).requires_consent = false)))).toBe(false);
  });

  it('takes no figure from a refused reply, even as the do-nothing baseline', async () => {
    const good = await plan();
    const refused = bad(good, (q) => ((q.proposals[0] as { executable: boolean }).executable = true));
    const only: Record<string, Outcome> = { a: { plan: refused }, b: { error: 'nope' } };
    expect(firstTrusted(only)).toBeNull();
    const mixed: Record<string, Outcome> = { a: { plan: refused }, b: { plan: good } };
    expect(firstTrusted(mixed)).toBe(good);
    expect(firstTrusted(null)).toBeNull();
  });
});
