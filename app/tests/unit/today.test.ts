import { describe, expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import type { AlertItem, AlertReport, Job, ReclaimPlan, Root, Volume } from '../../src/lib/contracts.gen';
import { ageText, jobNeed, largestGroup, linkName, missingSources, moreViews, needsEmptyText, nearlyFull, needsYou, reclaimTargets, rootNeed, unevaluatedAlerts, unevaluatedLine, unplacedRoots, usedFraction, volumeRows, type Sources } from '../../src/views/today/model';

const NOW = Date.parse('2026-10-30T12:00:00Z');
const iso = (minutesAgo: number) => new Date(NOW - minutesAgo * 60_000).toISOString();
const GiB = 2n ** 30n;
const client = () => new Client(createMockTransport({ now: () => new Date(NOW) }));

function volume(over: Partial<Volume> = {}): Volume {
  return {
    volume_id: 'vo_a', display_name: 'A:', mount_points: ['A:\\'], filesystem: 'NTFS', label: null, online: true, read_only: false, removable: false,
    capacity_bytes: String(100n * GiB), free_bytes: String(40n * GiB),
    device: { bus_type: 'nvme', seek_penalty: false, multiple_disks: false, basis: 'ioctl_storage_query_property' },
    tier: { tier: 1, basis: 'declared', declared_tier: 1, hint_tier: 1, note: '' },
    features: { file_ids_128: true, hard_links: true, sparse_files: true, compression: false, reparse_points: true, usn_journal: true },
    observed_at: iso(1), ...over,
  };
}
function root(over: Omit<Partial<Root>, 'scan'> & { scan?: Partial<Root['scan']> } = {}): Root {
  const { scan, ...rest } = over;
  return {
    root_id: 'rt_1', display_path: { text: 'A:\\data', truncated: false }, origin: 'owner_granted', dataset_class: 'personal', volume_id: 'vo_a',
    granted_at: iso(9000), granted_via: 'desktop_picker', grant_state: 'active', totals: null,
    scan: { state: 'complete', last_job_id: null, generation: '1', finished_at: iso(30), coverage: 'complete', ...scan }, ...rest,
  };
}
const job = (over: Partial<Job> = {}): Job => ({
  job_id: 'jb_1', seq: '1', rev: '1', kind: 'scan', state: 'running', root_id: 'rt_1', created_at: iso(10), started_at: iso(5), finished_at: null,
  progress: null, coverage: null, error: null, ...over,
} as Job);

describe('ageText', () => {
  it('says unknown for a missing or unparsable time, never "just now"', () => {
    expect(ageText(null, NOW)).toBe('unknown');
    expect(ageText('not a time', NOW)).toBe('unknown');
  });
  it('steps from minutes to hours to days', () => {
    expect(ageText(iso(0), NOW)).toBe('just now');
    expect(ageText(iso(5), NOW)).toBe('5 min ago');
    expect(ageText(iso(180), NOW)).toBe('3 h ago');
    expect(ageText(iso(60 * 24 * 5), NOW)).toBe('5 d ago');
  });
});

describe('volume rows', () => {
  it('derives the fill level and states a granted folder with a recent scan', () => {
    expect(usedFraction(volume())).toBe(0.6);
    const [row] = volumeRows([volume()], [root()], NOW);
    expect(row).toMatchObject({ name: 'A:', online: true, used: 0.6, space: '40.0 GiB free of 100 GiB', tier: 'Tier 1 · declared', scan: '30 min ago', unknowns: [] });
  });

  it('keeps unknown capacity and free space unknown, with no fill level and no zero', () => {
    const [row] = volumeRows([volume({ capacity_bytes: null, free_bytes: null })], [root()], NOW);
    expect(row!.used).toBeNull();
    expect(row!.space).toBe('unknown free of unknown');
    expect(row!.unknowns).toContain('Free space and capacity were not observed.');
    expect(volumeRows([volume({ free_bytes: null })], [], NOW)[0]!.unknowns).toContain('Free space was not observed.');
    expect(volumeRows([volume({ capacity_bytes: null })], [], NOW)[0]!.unknowns).toContain('Capacity was not observed.');
  });

  it('draws no fill level when free space exceeds capacity or capacity is zero', () => {
    expect(usedFraction(volume({ free_bytes: String(200n * GiB) }))).toBeNull();
    expect(usedFraction(volume({ capacity_bytes: '0', free_bytes: '0' }))).toBeNull();
    expect(volumeRows([volume({ free_bytes: String(200n * GiB) })], [], NOW)[0]!.unknowns[0]).toMatch(/contradict/);
  });

  it('says an offline volume is showing its last observation, and an unknown tier is unknown', () => {
    const [row] = volumeRows([volume({ online: false, observed_at: iso(120), tier: { tier: null, basis: 'unknown', declared_tier: null, hint_tier: null, note: '' } })], [], NOW);
    expect(row).toMatchObject({ online: false, tier: 'Tier unknown' });
    expect(row!.unknowns).toEqual(['Offline: space figures are from the last observation, 2 h ago.', 'No tier is declared or hinted.']);
  });

  it('draws no fill level for an offline volume and words its figures as the last observation', () => {
    const [row] = volumeRows([volume({ online: false, observed_at: iso(120) })], [], NOW);
    expect(row!.used).toBeNull();
    expect(row!.space).toBe('Last observed: 40.0 GiB free of 100 GiB');
    expect(volumeRows([volume()], [], NOW)[0]!.space).toBe('40.0 GiB free of 100 GiB');
  });

  it('says what the scan age rests on: no folder, never scanned, unread, partial, newest of several', () => {
    const v = [volume()];
    expect(volumeRows(v, [], NOW)[0]!.scan).toBe('No granted folder');
    expect(volumeRows(v, [root({ volume_id: 'vo_other' })], NOW)[0]!.scan).toBe('No granted folder');
    expect(volumeRows(v, [root({ grant_state: 'revoked' })], NOW)[0]!.scan).toBe('No granted folder');
    expect(volumeRows(v, [root({ scan: { state: 'never_scanned', finished_at: null } })], NOW)[0]!.scan).toBe('Never scanned');
    const unread = volumeRows(v, null, NOW)[0]!;
    expect(unread.scan).toBe('unknown');
    expect(unread.unknowns).toContain('Granted folders could not be read, so scan age is unknown.');
    const two = volumeRows(v, [root(), root({ root_id: 'rt_2', scan: { finished_at: iso(10), coverage: 'partial', state: 'partial' } }), root({ root_id: 'rt_3', scan: { state: 'never_scanned', finished_at: null } })], NOW)[0]!;
    expect(two.scan).toBe('10 min ago');
    expect(two.unknowns).toEqual(['1 granted folder not scanned yet.', '1 granted folder scanned only in part.']);
  });

  it('counts granted folders that sit on no listed volume', () => {
    expect(unplacedRoots([volume()], [root(), root({ root_id: 'rt_2', volume_id: null }), root({ root_id: 'rt_3', volume_id: 'vo_gone' }), root({ root_id: 'rt_4', volume_id: null, grant_state: 'revoked' })])).toBe(2);
  });
});

describe('reclaim targets', () => {
  const settings = { notifications_enabled: false, low_space_free_percent: 10, low_space_free_bytes: String(10n * GiB), growth_window_days: 7, growth_bytes: '1' };
  const report = { settings } as AlertReport;

  it('is nearly full below the larger of the percentage and the size, and never for unknown or offline', () => {
    expect(nearlyFull(volume({ capacity_bytes: String(1000n * GiB), free_bytes: String(50n * GiB) }), settings)).toBe(true); // 5% is under 10%
    expect(nearlyFull(volume({ capacity_bytes: String(1000n * GiB), free_bytes: String(150n * GiB) }), settings)).toBe(false);
    expect(nearlyFull(volume({ capacity_bytes: String(50n * GiB), free_bytes: String(8n * GiB) }), settings)).toBe(true); // 10 GiB beats 5 GiB
    expect(nearlyFull(volume({ free_bytes: null }), settings)).toBe(false);
    expect(nearlyFull(volume({ capacity_bytes: null }), settings)).toBe(false);
    expect(nearlyFull(volume({ capacity_bytes: String(100n * GiB), free_bytes: '0', online: false }), settings)).toBe(false);
  });

  it('asks only about nearly full volumes that hold an active granted folder', () => {
    const full = volume({ volume_id: 'vo_full', free_bytes: String(2n * GiB) });
    const noFolder = volume({ volume_id: 'vo_bare', free_bytes: String(2n * GiB) });
    const roomy = volume({ volume_id: 'vo_roomy' });
    const roots = [root({ volume_id: 'vo_full' }), root({ root_id: 'rt_2', volume_id: 'vo_roomy' }), root({ root_id: 'rt_3', volume_id: 'vo_bare', grant_state: 'revoked' })];
    expect(reclaimTargets([full, noFolder, roomy], roots, report)).toEqual(['vo_full']);
  });
});

describe('needs you', () => {
  it('flags a changed identity first, then the scans that need redoing, and nothing for healthy, scanning or revoked roots', () => {
    expect(rootNeed(root())).toBeNull();
    expect(rootNeed(root({ scan: { state: 'scanning' } }))).toBeNull();
    expect(rootNeed(root({ grant_state: 'revoked' }))).toBeNull();
    expect(rootNeed(root({ grant_state: 'identity_changed' }))!.text).toMatch(/Grant it again/);
    for (const state of ['never_scanned', 'stale', 'failed', 'partial'] as const) expect(rootNeed(root({ scan: { state } }))).not.toBeNull();
    const out = needsYou({ volumes: null, roots: [root({ scan: { state: 'stale' } }), root({ root_id: 'rt_2', grant_state: 'identity_changed' })], alerts: null, jobs: null, reclaim: new Map() }, NOW);
    expect(out).toHaveLength(1);
    expect(out[0]!.items.map((i) => i.key)).toEqual(['root-rt_2', 'root-rt_1']);
  });

  it('does not call a complete scan with partial or unknown coverage clean', () => {
    expect(rootNeed(root({ scan: { state: 'complete', coverage: 'complete' } }))).toBeNull();
    for (const coverage of ['partial', 'unknown', 'stale'] as const) {
      expect(rootNeed(root({ scan: { state: 'complete', coverage } }))!.text).toBe(`finished, but its coverage is ${coverage}, so its numbers may be incomplete.`);
    }
    expect(rootNeed(root({ scan: { state: 'scanning', coverage: 'partial' } }))).toBeNull();
    expect(rootNeed(root({ grant_state: 'revoked', scan: { state: 'complete', coverage: 'partial' } }))).toBeNull();
  });

  it('names each linked view for people and points "and N more" at every view among the hidden items', () => {
    expect(['alerts', 'health', 'duplicates', 'reclaim', 'review'].map(linkName)).toEqual(['Alerts', 'Grants & health', 'Duplicates', 'Reclaim', 'Learning review']);
    expect(linkName('something_new')).toBe('something_new');
    const kinds = ['scan', 'scan', 'scan', 'scan', 'scan', 'duplicates', 'refit', 'duplicates'] as const;
    const items = kinds.map((kind, n) => jobNeed(job({ job_id: `jb_${n}`, kind, root_id: null }), null, NOW)!);
    expect(moreViews(items, 5)).toEqual(['duplicates', 'review']);
    expect(moreViews(items, 8)).toEqual([]);
  });

  it('lists only unfinished jobs, with the view that shows each', () => {
    expect(jobNeed(job({ state: 'completed' }), null, NOW)).toBeNull();
    expect(jobNeed(job({ state: 'failed' }), null, NOW)).toBeNull();
    expect(jobNeed(job({ state: 'cancelled' }), null, NOW)).toBeNull();
    expect(jobNeed(job(), [root()], NOW)).toMatchObject({ name: 'A:\\data', text: 'scan is running, started 5 min ago.', view: 'health' });
    expect(jobNeed(job({ kind: 'duplicates', state: 'queued', root_id: null, started_at: null }), null, NOW)).toMatchObject({ name: null, text: 'Duplicate inspection is queued.', view: 'duplicates' });
    expect(jobNeed(job({ kind: 'reclaim', state: 'cancel_requested' }), null, NOW)!.view).toBe('reclaim');
    expect(jobNeed(job({ kind: 'refit' }), null, NOW)!.view).toBe('review');
  });

  it('assembles alerts, roots, jobs and reclaim from the mock transport, in that order', async () => {
    const c = client();
    const vols = (await c.call('volumes.list', {})).result.volumes;
    const roots = (await c.call('roots.list', {})).result.roots;
    const alerts = (await c.call('alerts.list', {})).result;
    const targets = reclaimTargets(vols, roots, alerts);
    expect(targets).toContain('vo_c'); // 9% free is under the 10% default, and the mock grants a folder on it
    const reclaim = new Map<string, ReclaimPlan | null>();
    for (const id of targets) reclaim.set(id, (await c.call('reclaim.plan', { volume_id: id, limit: 20 })).result);
    const src: Sources = { volumes: vols, roots, alerts, jobs: [job({ root_id: roots[0]!.root_id })], reclaim };
    const out = needsYou(src, NOW);
    expect(out.map((s) => s.section)).toEqual(['alerts', 'roots', 'jobs', 'reclaim']);
    expect(out[1]!.items.length).toBeGreaterThan(0);    expect(out[0]!.items[0]).toMatchObject({ name: 'Synthetic volume A', view: 'alerts' });
    expect(out[0]!.items[0]!.text).toBe('has 4.00 GiB free of 100 GiB, below 10.0 GiB.');
    const growth = { ...alerts.items[0]!, kind: 'fast_growth' as const, status: 'alert' as const, label: 'Projects', measured_bytes: String(7n * GiB), threshold_bytes: String(5n * GiB) };
    expect(needsYou({ ...src, alerts: { ...alerts, items: [growth] }, roots: [], jobs: [], reclaim: new Map() }, NOW)[0]!.items[0]!.text).toBe('grew +7.00 GiB between complete scans, flagged at 5.00 GiB or more.');
    expect(out[3]!.items[0]).toMatchObject({ name: 'C:', view: 'reclaim' });
    expect(out[3]!.items[0]!.text).toBe('largest group is tool-owned cleanup, 1 candidate, reclaimable size not observed yet (the folders hold 7.81 KiB).');
    expect(missingSources(src, 0)).toEqual([]);
  });

  it('picks the group with the most known reclaimable bytes, and says when a size is not observed', async () => {
    const plan = (await client().call('reclaim.plan', { volume_id: 'vo_c', limit: 20 })).result;
    expect(largestGroup(plan)).toEqual({ name: 'Tool-owned cleanup', text: '1 candidate, reclaimable size not observed yet (the folders hold 7.81 KiB).' });
    const cand = (reclaimable: string | null, allocated: string | null) => ({ ...plan.groups[0]!.candidates[0]!, reclaimable_bytes: reclaimable, allocated_bytes: allocated, logical_bytes: allocated });
    const two: ReclaimPlan = { ...plan, partial: true, groups: [
      { kind: 'tool_owned_cleanup', candidates: [cand('1000', '5000')] },
      { kind: 'archive_candidates', candidates: [cand('9000', '9000'), cand(null, '100')] },
      { kind: 'informational', candidates: [cand('999999', '999999')] },
    ] };
    expect(largestGroup(two)).toEqual({ name: 'Archive candidates', text: '2 candidates, 8.79 KiB reclaimable, 1 not observed yet. The plan is partial.' });
    expect(largestGroup({ ...plan, groups: [{ kind: 'relocate', candidates: [] }] })).toBeNull();
    // A stale root clears every size: unknown, never "0 B"; a partly known group says "at least".
    expect(largestGroup({ ...plan, groups: [{ kind: 'relocate', candidates: [cand(null, null)] }] }))
      .toEqual({ name: 'Relocate', text: '1 candidate, reclaimable size not observed yet (their size is not observed either).' });
    expect(largestGroup({ ...plan, groups: [{ kind: 'relocate', candidates: [cand(null, null), cand(null, '1024')] }] }))
      .toEqual({ name: 'Relocate', text: '2 candidates, reclaimable size not observed yet (the folders hold at least 1.00 KiB).' });
  });

  it('keeps an incomplete reclaim plan with no candidates visible, never as a clean result', async () => {
    const plan = (await client().call('reclaim.plan', { volume_id: 'vo_c', limit: 20 })).result;
    const empty: ReclaimPlan = { ...plan, partial: true, budget_hit: true, groups: [] };
    const out = needsYou({ volumes: [volume({ volume_id: 'vo_a' })], roots: [], alerts: null, jobs: [], reclaim: new Map([['vo_a', empty]]) }, NOW);
    expect(out.find((s) => s.section === 'reclaim')?.items[0]?.text).toBe('reclaim plan is incomplete and found no candidates yet; that is not a clean result.');
    expect(needsYou({ volumes: [volume({ volume_id: 'vo_a' })], roots: [], alerts: null, jobs: [], reclaim: new Map([['vo_a', { ...empty, partial: false, budget_hit: false, truncated: false }]]) }, NOW)).toEqual([]);
  });

  it('names a source that could not be read, and treats a failed reclaim plan as missing, not as empty', () => {
    const src: Sources = { volumes: [volume()], roots: null, alerts: null, jobs: [], reclaim: new Map([['vo_a', null]]) };
    expect(missingSources(src, 1)).toEqual(['granted folders', 'alerts', 'one reclaim plan']);
    expect(needsYou(src, NOW)).toEqual([]);
  });
});

describe('alerts that could not be evaluated', () => {
  const alertItem = (status: AlertItem['status']): AlertItem =>
    ({ kind: 'low_space', status, label: 'C:', volume_id: 'vo_a', root_id: null, measured_bytes: null, threshold_bytes: null, capacity_bytes: null, window_start: null, window_end: null, coverage: 'complete' }) as AlertItem;
  const report = (...items: AlertItem[]) => ({ settings: { notifications_enabled: false, low_space_free_percent: 10, low_space_free_bytes: String(10n * GiB), growth_window_days: 7, growth_bytes: '1' }, items }) as AlertReport;

  it('counts the alerts that were not evaluated, and neither the ok nor the active ones', () => {
    expect(unevaluatedAlerts(null)).toBe(0);
    expect(unevaluatedAlerts(report(alertItem('ok'), alertItem('alert')))).toBe(0);
    expect(unevaluatedAlerts(report(alertItem('unknown'), alertItem('insufficient_history'), alertItem('incomplete_coverage'), alertItem('ok')))).toBe(3);
  });

  it('says how many could not be evaluated, and says nothing when every alert was evaluated', () => {
    expect(unevaluatedLine(0)).toBeNull();
    expect(unevaluatedLine(1)).toBe('1 alert could not be evaluated; see Alerts.');
    expect(unevaluatedLine(2)).toBe('2 alerts could not be evaluated; see Alerts.');
  });

  it('keeps unevaluated alerts out of the needs list, and never shows "Nothing needs you." alone beside them', () => {
    const src: Sources = { volumes: null, roots: null, alerts: report(alertItem('unknown'), alertItem('incomplete_coverage')), jobs: null, reclaim: new Map() };
    expect(needsYou(src, NOW)).toEqual([]);
    expect(needsEmptyText(missingSources(src, 0).length, unevaluatedAlerts(src.alerts))).toBe('Nothing needs you in what was read.');
    expect(unevaluatedLine(unevaluatedAlerts(src.alerts))).toBe('2 alerts could not be evaluated; see Alerts.');
  });

  it('says "Nothing needs you." only when nothing was missing or unevaluated', () => {
    expect(needsEmptyText(0, 0)).toBe('Nothing needs you.');
    expect(needsEmptyText(0, 2)).toBe('Nothing needs you in what was read.');
    expect(needsEmptyText(1, 0)).toBe('Nothing needs you in what was read.');
  });
});
