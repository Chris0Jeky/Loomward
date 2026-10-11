import { describe, expect, it } from 'vitest';
import { Client, LoomwardError } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import type { PlacementSimulateRequest } from '../../src/lib/contracts.gen';

const now = () => new Date('2026-10-01T00:00:00Z');
const client = () => new Client(createMockTransport({ now }));
const GiB = 2 ** 30;
const sim = (c: Client, o: Partial<PlacementSimulateRequest> = {}) =>
  c.call('placement.simulate', {
    source_volume_id: 'vo_c', target_free_bytes: String(Math.round(380 * GiB)), max_transfer_bytes: String(400 * GiB), heat_policy: 'unknown_is_ineligible',
    relief_policy: 'verified_only', candidate_basis: 'largest_dirs', max_groups: 50, overrides: [], node_budget: 100000, save: false, ...o,
  });

describe('mock tiers and placement', () => {
  it('models three real-shaped volumes plus one with an unknown tier', async () => {
    const { result } = await client().call('tiers.model', {});
    const by = Object.fromEntries(result.volumes.map((v) => [v.display_name, v]));
    expect(by['C:']!.tier.basis).toBe('device_hint');
    expect(by['G:']!.tier.basis).toBe('declared');
    expect(by['C:']!.pressure).toBe('pressure');
    expect(by['F:']!.tier.tier).toBeNull();
    expect(by['F:']!.tier.basis).toBe('unknown');
  });

  it('never makes a simulation executable or a filesystem change', async () => {
    const c = client();
    for (const heat_policy of ['unknown_is_ineligible', 'mtime_proxy_whatif', 'assumed_only'] as const) {
      const { result: p } = await sim(c, { heat_policy });
      expect(p.mode).toBe('simulation');
      expect(p.filesystem_changed).toBe(false);
      expect(p.proposals.every((x) => x.executable === false && x.requires_consent === true)).toBe(true);
    }
  });

  it('with unknown heat does nothing, and says why for every group', async () => {
    const c = client();
    const { result: cand } = await c.call('placement.candidates', { source_volume_id: 'vo_c', basis: 'largest_dirs', max_groups: 50, min_bytes: '0' });
    const { result: p } = await sim(c);
    expect(p.proposals).toEqual([]);
    expect(p.satisfied).toBe(false);
    expect(p.shortfall_bytes).toBe(p.baseline_shortfall_bytes);
    expect(p.shortfall_improvement_bytes).toBe('0');
    const accounted = [...p.rejected, ...p.pre_rejected].map((x) => x.group_id).sort();
    expect(accounted).toEqual(cand.groups.map((g) => g.group_id).sort());
    expect(p.pre_rejected.map((x) => x.reason).sort()).toEqual(['group_coverage_incomplete', 'relief_unknown', 'shares_objects_with_other_group']);
    expect(p.excluded_volumes).toEqual([{ volume_id: 'vo_f', reason: 'tier_unknown' }]);
  });

  it('plans from an owner heat assumption and records it as an assumption', async () => {
    const { result: p } = await sim(client(), { overrides: [{ group_id: 'cg_models', heat: 0.1 }] });
    expect(p.proposals.map((x) => x.group_id)).toEqual(['cg_models']);
    expect(p.proposals[0]!.target_id).toBe('vo_g');
    expect(p.satisfied).toBe(true);
    expect(p.assumptions.join(' ')).toContain('assumed 0.1');
    expect(BigInt(p.projected_free_bytes.vo_g!)).toBeLessThan(BigInt(Math.round(0.58 * 930 * GiB)));
  });

  it('keeps unknown relief out of a verified-only plan and lets the what-if policy in, labelled', async () => {
    const over = [{ group_id: 'cg_vms', heat: 0.1 }];
    const verified = (await sim(client(), { overrides: over })).result;
    expect(verified.pre_rejected).toContainEqual({ group_id: 'cg_vms', reason: 'relief_unknown' });
    const whatif = (await sim(client(), { overrides: over, relief_policy: 'entry_allocation_whatif' })).result;
    expect(whatif.pre_rejected.map((x) => x.group_id)).not.toContain('cg_vms');
    expect(whatif.assumptions.join(' ')).toContain('what-if');
  });

  it('rejects malformed requests', async () => {
    await expect(sim(client(), { source_volume_id: 'vo_nope' })).rejects.toBeInstanceOf(LoomwardError);
    await expect(sim(client(), { heat_policy: 'bogus' as never })).rejects.toMatchObject({ code: 'invalid_request' });
  });
});

describe('mock companion', () => {
  it('refreshes leased process timestamps on samples, retaining them between enumerations', async () => {
    let clock = new Date('2026-10-01T00:00:00Z');
    const c = new Client(createMockTransport({ now: () => clock }));
    const list = async () => (await c.call('processes.list', { sort: 'name_asc', limit: 200 })).result;
    const first = await list();
    const lease = (await c.call('telemetry.subscribe', { subscription_id: null, channels: ['processes'], interval_ms: 1000 })).result;
    clock = new Date('2026-10-01T00:00:10Z');
    await c.call('telemetry.snapshot', { channels: ['processes'] });
    expect((await list()).observed_at).toBe(first.observed_at);
    clock = new Date('2026-10-01T00:00:15Z');
    await c.call('telemetry.snapshot', { channels: ['processes'] });
    const explanation = (await c.call('processes.explain', { process_ref: first.rows[0]!.process_ref })).result;
    expect(explanation.observed_at).toBe(clock.toISOString());
    const refreshed = await list();
    expect(refreshed.rows.every((row) => row.observed_at === clock.toISOString())).toBe(true);
    await c.call('telemetry.unsubscribe', { subscription_id: lease.subscription_id });
    clock = new Date('2026-10-01T00:01:00Z');
    await c.call('telemetry.snapshot', { channels: ['system'] });
    expect((await list()).observed_at).toBe(refreshed.observed_at);
  });
  it('never reuses subscription IDs or lets a stale unsubscribe stop another lease', async () => {
    const c = client();
    const subscribe = () => c.call('telemetry.subscribe', { subscription_id: null, channels: ['system'], interval_ms: 1000 });
    const a = (await subscribe()).result.subscription_id;
    const b = (await subscribe()).result.subscription_id;
    await c.call('telemetry.unsubscribe', { subscription_id: a });
    const d = (await subscribe()).result.subscription_id;
    expect(new Set([a, b, d]).size).toBe(3);
    expect((await c.call('telemetry.unsubscribe', { subscription_id: a })).result.stopped).toBe(false);
    expect((await c.call('telemetry.unsubscribe', { subscription_id: b })).result.stopped).toBe(true);
    expect((await c.call('telemetry.unsubscribe', { subscription_id: d })).result.stopped).toBe(true);
    const e = (await subscribe()).result.subscription_id;
    expect([a, b, d]).not.toContain(e);
  });

  it('bounds the process list and reports coverage totals', async () => {
    const c = client();
    const { result: top } = await c.call('processes.list', { sort: 'private_desc', limit: 10 });
    expect(top.rows).toHaveLength(10);
    expect(top.truncated).toBe(true);
    expect(top.observed_count).toBeGreaterThan(10);
    expect(top.denied_count).toBeGreaterThan(0);
    const commits = top.rows.map((r) => Number(r.private_commit_bytes));
    expect([...commits].sort((a, b) => b - a)).toEqual(commits);
    const { result: all } = await c.call('processes.list', { sort: 'name_asc', limit: 200 });
    expect(all.truncated).toBe(false);
    const denied = all.rows.filter((r) => r.access === 'denied');
    expect(denied).toHaveLength(all.denied_count);
    expect(denied.every((r) => r.private_commit_bytes === null && r.cpu_fraction === null)).toBe(true);
  });

  it('explains rule-based and offers no action', async () => {
    const c = client();
    const { result: all } = await c.call('processes.list', { sort: 'name_asc', limit: 200 });
    const row = all.rows.find((r) => r.access === 'full')!;
    const { result: e } = await c.call('processes.explain', { process_ref: row.process_ref });
    expect(e.available_actions).toEqual([]);
    expect(e.facts.length).toBeGreaterThan(0);
    await expect(c.call('processes.explain', { process_ref: 'pc_nope' })).rejects.toMatchObject({ code: 'not_found' });
  });

  it('serves own budgets and a telemetry sample whose first rates are unknown', async () => {
    const c = client();
    const first = (await c.call('telemetry.snapshot', { channels: ['system', 'engine'] })).result;
    expect(first.system!.cpu.busy_fraction).toBeNull();
    expect(first.engine!.cpu_fraction).toBeNull();
    expect((await c.call('telemetry.snapshot', { channels: ['system'] })).result.system!.cpu.busy_fraction).not.toBeNull();
    const b = (await c.call('budgets.get', {})).result;
    expect(b.pools.every((p) => p.scope === 'loomward_own_threads')).toBe(true);
  });
});

describe('mock revocation', () => {
  it('shows a revoked root and grant in the lists afterwards', async () => {
    const c = client();
    const roots = (await c.call('roots.list', {})).result.roots;
    expect(roots.every((r) => r.grant_state === 'active')).toBe(true);
    await c.call('roots.revoke', { root_id: roots[0]!.root_id, purge_catalog: false });
    const after = (await c.call('roots.list', {})).result.roots;
    expect(after[0]!.grant_state).toBe('revoked');
    expect(after[1]!.grant_state).toBe('active');
    const g = (await c.call('grants.list', {})).result.grants;
    expect(g.find((x) => x.kind === 'metadata_root' && x.root_id === roots[0]!.root_id)!.revoked_at).not.toBeNull();
    await c.call('grants.revoke', { grant_id: 'gr_mock_teacher_0' });
    expect((await c.call('grants.list', {})).result.grants.find((x) => x.grant_id === 'gr_mock_teacher_0')!.revoked_at).not.toBeNull();
    await expect(c.call('roots.revoke', { root_id: 'rt_nope', purge_catalog: false })).rejects.toMatchObject({ code: 'not_found' });
  });

  it('moves catalog_rev on a first purge only, as a committed catalogue transaction (semantics 5)', async () => {
    const c = client();
    const revoke = (purge: boolean, root = 'rt_mock_1') => c.call('roots.revoke', { root_id: root, purge_catalog: purge });
    expect((await revoke(false)).meta.catalog_rev).toBe('1'); // keeping the rows changes nothing in catalog.db
    expect((await revoke(true)).meta.catalog_rev).toBe('2');
    expect((await revoke(true)).meta.catalog_rev).toBe('2'); // nothing left to delete
    expect((await revoke(true, 'rt_mock_9')).meta.catalog_rev).toBe('3');
    expect((await c.call('tree.children', { node_id: 'nd_1', sort: 'size_desc', basis: 'logical', limit: 1, cursor: null })).meta.catalog_rev).toBe('3');
  });

  it('binds paging cursors to catalog_rev: a cursor from before a purge is stale_generation (semantics 5)', async () => {
    const c = client();
    const kids = (cursor: string | null) => c.call('tree.children', { node_id: 'nd_1', sort: 'size_desc', basis: 'logical', limit: 2, cursor });
    const hits = (cursor: string | null) => c.call('search.query', { root_id: null, text: 'a', extension: null, min_bytes: null, kind: 'any', limit: 2, cursor });
    const k = (await kids(null)).result.next_cursor!;
    const s = (await hits(null)).result.next_cursor!;
    expect((await kids(k)).result.items.length).toBeGreaterThan(0); // same revision: fine
    await c.call('roots.revoke', { root_id: 'rt_mock_1', purge_catalog: true });
    await expect(kids(k)).rejects.toMatchObject({ code: 'stale_generation' });
    await expect(hits(s)).rejects.toMatchObject({ code: 'stale_generation' });
    await expect(kids('forged!')).rejects.toMatchObject({ code: 'invalid_request' });
    expect((await kids((await kids(null)).result.next_cursor)).result.items.length).toBeGreaterThan(0); // a fresh cursor works
  });

  it('shows a revoked root grant in grants.list and in roots.list, whichever call revoked it', async () => {
    const c = client();
    await c.call('grants.revoke', { grant_id: 'gr_mock_root_0' });
    const grants = (await c.call('grants.list', {})).result.grants;
    const g = grants.find((x) => x.grant_id === 'gr_mock_root_0')!;
    expect(g.revoked_at).not.toBeNull();
    const roots = (await c.call('roots.list', {})).result.roots;
    expect(roots.find((r) => g.kind === 'metadata_root' && r.root_id === g.root_id)!.grant_state).toBe('revoked');
    expect(grants.find((x) => x.grant_id === 'gr_mock_root_1')!.revoked_at).toBeNull();
  });
});
