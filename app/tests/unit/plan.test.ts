import { describe, expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import type { PlacementPlan } from '../../src/lib/contracts.gen';
import { firstTrusted, isSound, reliefAgeText, type Outcome } from '../../src/views/tiers/plan';

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

describe('malformed plans are refused, never thrown on', () => {
  it('refuses missing or mistyped fields without throwing', async () => {
    const good = await plan();
    const cases: [string, (q: Record<string, unknown>) => void][] = [
      ['proposals not an array', (q) => (q.proposals = 'none')],
      ['proposals missing', (q) => delete q.proposals],
      ['a null proposal', (q) => (q.proposals = [null])],
      ['rejected not an array', (q) => (q.rejected = {})],
      ['pre_rejected missing', (q) => delete q.pre_rejected],
      ['excluded_volumes missing', (q) => delete q.excluded_volumes],
      ['assumptions not an array', (q) => (q.assumptions = 'x')],
      ['projected_free_bytes null', (q) => (q.projected_free_bytes = null)],
      ['search missing', (q) => delete q.search],
      ['a byte count that is not a decimal string', (q) => (q.baseline_shortfall_bytes = 12)],
      ['a proposal byte count missing', (q) => delete (q.proposals as Record<string, unknown>[])[0]!.transfer_bytes],
    ];
    for (const [name, change] of cases) {
      const q = structuredClone(good) as unknown as Record<string, unknown>;
      change(q);
      expect(() => isSound(q as unknown as PlacementPlan), name).not.toThrow();
      expect(isSound(q as unknown as PlacementPlan), name).toBe(false);
    }
    expect(isSound(null as unknown as PlacementPlan)).toBe(false);
    expect(firstTrusted({ a: { plan: null as unknown as PlacementPlan } })).toBeNull();
  });
});

describe('Tiers stored-verdict age (#1029)', () => {
  const at = '2026-10-29T00:00:00Z';
  it('words a stored verdict like the Reclaim view and warns that later hard links are not reflected', () => {
    expect(reliefAgeText(at, Date.parse(at) + 3 * 3_600_000)).toBe('verified 3 hours ago \u00b7 hard links made since then are not reflected');
    expect(reliefAgeText(at, Date.parse(at) + 2 * 86_400_000)).toBe('verified 2 days ago \u00b7 hard links made since then are not reflected');
    expect(reliefAgeText('bad', Date.parse(at))).toContain('verified at an unknown time');
  });
  it('shows nothing when the figure is not from a stored verdict', () => {
    expect(reliefAgeText(null, Date.parse(at))).toBeNull();
  });
  it('the mock carries a time only for relief that came from a stored verdict', async () => {
    const c = new Client(createMockTransport());
    const { result } = await c.call('placement.candidates', { source_volume_id: 'vo_c', basis: 'largest_dirs', max_groups: 50, min_bytes: '0' });
    expect(result.groups.some((g) => g.relief_observed_at !== null)).toBe(true);
    for (const g of result.groups) {
      if (g.relief_basis !== 'verified_unique_allocation') expect(g.relief_observed_at).toBeNull();
    }
    expect(result.groups.some((g) => g.relief_basis === 'verified_unique_allocation' && g.relief_observed_at === null)).toBe(true);
  });
});
