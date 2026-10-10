import type { ByteCount, ErrorCode, Timestamp } from '../contracts.gen';
import type {
  CandidateGroup, GroupOverride, HeatBasis, OwnBudgets, PlacementCandidates, PlacementPlan, PlacementProposal,
  PlacementSimulateRequest, PoolName, PoolUse, ProcessExplanation, ProcessList, ProcessRow,
  RootRevokeResult, GrantRevokeResult, TelemetrySample, TelemetrySubscription, TierModel, Volume, VolumeList,
} from '../contracts.gen';
import type { PreRejectReason, RejectReason } from '../derived';
import { formatBytes } from '../format/bytes';
import { prng } from './synth';

// Everything below is invented, contract-shaped data for the Tiers, Companion and Grants views on the
// mock transport. Volumes follow the owner's machine in shape only (NVMe, NVMe, HDD); no real path or
// process is named. TODO(L1): serve contracts/v3/examples/ once lane L1 lands them.

export type Fail = (code: ErrorCode, message: string) => never;
type Handler = (payload: Record<string, unknown>) => unknown;

const GiB = 2n ** 30n;
const gib = (n: number): bigint => BigInt(Math.round(n * 2 ** 30));
const str = (b: bigint): ByteCount => b.toString();
const WATCH = 0.2;
const PRESSURE = 0.1;

interface VolumeSeed { id: string; name: string; fs: string; bus: Volume['device']['bus_type']; seek: boolean | null; capacity: bigint; freeFraction: number; declared: number | null; hint: number | null; removable: boolean }
const VOLUMES: VolumeSeed[] = [
  { id: 'vo_c', name: 'C:', fs: 'NTFS', bus: 'nvme', seek: false, capacity: BigInt(Math.round(1.86 * 2 ** 40)), freeFraction: 0.09, declared: null, hint: 1, removable: false },
  { id: 'vo_g', name: 'G:', fs: 'NTFS', bus: 'nvme', seek: false, capacity: 930n * GiB, freeFraction: 0.58, declared: 2, hint: 1, removable: false },
  { id: 'vo_e', name: 'E:', fs: 'NTFS', bus: 'sata', seek: true, capacity: BigInt(Math.round(1.86 * 2 ** 40)), freeFraction: 0.31, declared: null, hint: 5, removable: false },
  { id: 'vo_f', name: 'F:', fs: 'exFAT', bus: 'usb', seek: null, capacity: 238n * GiB, freeFraction: 0.7, declared: null, hint: null, removable: true },
];

interface GroupSeed {
  id: string; name: string; gib: number; basis: CandidateGroup['estimate_basis']; reliefKnown: boolean; coverage: CandidateGroup['coverage'];
  modified: string; pinned: boolean | null; active: boolean | null; protected: boolean | null; days: number | null; shared?: boolean;
}
const GROUPS: GroupSeed[] = [
  { id: 'cg_models', name: 'synthetic-model-store', gib: 212, basis: 'allocated_entries', reliefKnown: true, coverage: 'complete', modified: '2025-03-18T10:00:00Z', pinned: false, active: false, protected: false, days: 420 },
  { id: 'cg_render', name: 'synthetic-render-cache', gib: 148, basis: 'allocated_entries', reliefKnown: true, coverage: 'complete', modified: '2026-09-28T18:30:00Z', pinned: false, active: false, protected: false, days: 60 },
  { id: 'cg_vms', name: 'synthetic-vm-images', gib: 96, basis: 'logical_fallback', reliefKnown: false, coverage: 'complete', modified: '2025-06-02T08:15:00Z', pinned: false, active: false, protected: false, days: 300 },
  { id: 'cg_photos', name: 'synthetic-photo-archive', gib: 131, basis: 'allocated_entries', reliefKnown: true, coverage: 'complete', modified: '2024-11-30T12:00:00Z', pinned: true, active: false, protected: false, days: 500 },
  { id: 'cg_installers', name: 'synthetic-installers', gib: 38, basis: 'allocated_entries', reliefKnown: true, coverage: 'complete', modified: '2025-01-09T09:00:00Z', pinned: false, active: false, protected: false, days: 380 },
  { id: 'cg_build', name: 'synthetic-build-output', gib: 61, basis: 'allocated_entries', reliefKnown: true, coverage: 'complete', modified: '2026-02-11T14:00:00Z', pinned: false, active: false, protected: false, days: 200, shared: true },
  { id: 'cg_datasets', name: 'synthetic-datasets', gib: 74, basis: 'unknown', reliefKnown: false, coverage: 'partial', modified: '2025-08-21T16:45:00Z', pinned: false, active: false, protected: false, days: 150 },
  { id: 'cg_podcast', name: 'synthetic-podcast-raw', gib: 27, basis: 'allocated_entries', reliefKnown: true, coverage: 'complete', modified: '2025-02-14T11:00:00Z', pinned: false, active: null, protected: false, days: 400 },
  { id: 'cg_staging', name: 'synthetic-backups-staging', gib: 54, basis: 'allocated_entries', reliefKnown: true, coverage: 'complete', modified: '2025-04-03T07:00:00Z', pinned: false, active: false, protected: false, days: null },
];

export interface ViewMock {
  handlers: Record<string, Handler>;
  /** root_id / grant_id -> revoked_at, read by the roots.list and grants.list handlers in mock.ts. */
  revokedRoots: Map<string, Timestamp>;
  revokedGrants: Map<string, Timestamp>;
  /** state.db revision: starts at 1 and moves only when a revocation actually changes owner data. */
  stateRev(): string;
}

export function createViewMock(now: () => Date, fail: Fail): ViewMock {
  const iso = () => now().toISOString();
  const revokedRoots = new Map<string, Timestamp>();
  const revokedGrants = new Map<string, Timestamp>();
  const subscriptions = new Set<string>();
  let stateRev = 1;
  let sampleSeq = 0;

  const free = (v: VolumeSeed): bigint => BigInt(Math.round(Number(v.capacity) * v.freeFraction));
  const seed = (id: unknown): VolumeSeed => VOLUMES.find((v) => v.id === id) ?? fail('not_found', 'no such volume');
  const tierInfo = (v: VolumeSeed): Volume['tier'] => {
    const tier = v.declared ?? v.hint;
    return {
      tier, basis: v.declared !== null ? 'declared' : v.hint !== null ? 'device_hint' : 'unknown', declared_tier: v.declared, hint_tier: v.hint,
      note: v.declared !== null ? 'Declared by the owner.' : v.hint !== null ? 'Device hint, not a measured speed.' : 'The device reports nothing usable; declare a tier to include this volume.',
    };
  };
  const volumeRow = (v: VolumeSeed): Volume => ({
    volume_id: v.id, display_name: v.name, mount_points: [`${v.name}\\`], filesystem: v.fs, label: null, online: true, read_only: false, removable: v.removable,
    capacity_bytes: str(v.capacity), free_bytes: str(free(v)),
    device: { bus_type: v.bus, seek_penalty: v.seek, multiple_disks: v.bus === 'usb' ? null : false, basis: v.bus === 'usb' ? 'unavailable' : 'ioctl_storage_query_property' },
    tier: tierInfo(v), features: { file_ids_128: null, hard_links: null, sparse_files: null, compression: null, reparse_points: null, usn_journal: null }, // unknown, not invented
    observed_at: iso(),
  });

  const model = (): TierModel => ({
    volumes: VOLUMES.map((v) => {
      const frac = Number(free(v)) / Number(v.capacity);
      return {
        volume_id: v.id, display_name: v.name, tier: tierInfo(v), online: true, writable: true,
        capacity_bytes: str(v.capacity), free_bytes: str(free(v)), reserve_bytes: str(v.capacity / 50n), free_fraction: frac,
        pressure: frac < PRESSURE ? 'pressure' : frac < WATCH ? 'watch' : 'ok',
      };
    }),
    policy: {
      watch_free_fraction: WATCH, pressure_free_fraction: PRESSURE,
      reserve_note: 'Each volume keeps a reserve of 2% of its capacity that a plan never spends.',
      note: 'Tier 0 is fastest. A plan only demotes to an equal or slower tier. Tiers come from a declaration, else a device hint, else they are unknown.',
    },
  });

  const bytesOf = (g: GroupSeed) => gib(g.gib);
  const candidate = (g: GroupSeed): CandidateGroup => ({
    group_id: g.id, node_id: `nd_mock_${g.id}`, root_id: 'rt_mock_1', name: g.name,
    source_bytes: str(bytesOf(g)), destination_bytes: str(bytesOf(g) + bytesOf(g) / 256n), transfer_bytes: str((bytesOf(g) * 97n) / 100n),
    estimate_basis: g.basis, estimated_relief_bytes: g.reliefKnown ? str(bytesOf(g)) : null,
    relief_basis: g.reliefKnown ? 'verified_unique_allocation' : 'unknown',
    heat: null, heat_basis: 'unknown', newest_modified_at: g.modified, pinned: g.pinned, active: g.active, protected: g.protected, days_since_move: g.days, coverage: g.coverage,
  });

  const candidates = (p: Record<string, unknown>): PlacementCandidates => {
    seed(p.source_volume_id);
    const min = typeof p.min_bytes === 'string' && /^\d{1,19}$/.test(p.min_bytes) ? BigInt(p.min_bytes) : fail('invalid_request', 'min_bytes');
    const groups = p.source_volume_id === 'vo_c' ? GROUPS.filter((g) => bytesOf(g) >= min).slice(0, Number(p.max_groups) || 200).map(candidate) : [];
    return { source_volume_id: String(p.source_volume_id), root_generations: [], groups, note: 'Candidates are non-overlapping folders on the source volume. Sizes are bytes by directory entry; relief is only known where every object was verified.' };
  };

  // Planner v2's eligibility order and thresholds (python/loomward/planner_v2.py), on invented volumes.
  const simulate = (raw: Record<string, unknown>): PlacementPlan => {
    const p = raw as unknown as PlacementSimulateRequest;
    const source = seed(p.source_volume_id);
    const big = (v: unknown, what: string) => (typeof v === 'string' && /^\d{1,19}$/.test(v) ? BigInt(v) : fail('invalid_request', `${what} must be a byte string`));
    const goal = big(p.target_free_bytes, 'target_free_bytes');
    const budget = big(p.max_transfer_bytes, 'max_transfer_bytes');
    if (!['unknown_is_ineligible', 'mtime_proxy_whatif', 'assumed_only'].includes(p.heat_policy)) fail('invalid_request', 'heat_policy');
    if (!['verified_only', 'entry_allocation_whatif'].includes(p.relief_policy)) fail('invalid_request', 'relief_policy');
    const overrides = new Map<string, GroupOverride>((Array.isArray(p.overrides) ? p.overrides : []).map((o) => [o.group_id, o]));
    const tier = tierInfo(source).tier;
    const excluded: PlacementPlan['excluded_volumes'] = VOLUMES.filter((v) => v.id !== source.id && tierInfo(v).tier === null).map((v) => ({ volume_id: v.id, reason: 'tier_unknown' }));
    if (tier === null) fail('invalid_request', 'source tier unknown');
    const targets = VOLUMES.filter((v) => v.id !== source.id && (tierInfo(v).tier ?? -1) >= tier);

    const assumptions: string[] = [];
    const preRejected: PlacementPlan['pre_rejected'] = [];
    const rejected: PlacementPlan['rejected'] = [];
    const eligible: { g: GroupSeed; relief: bigint; heat: number }[] = [];
    for (const g of source.id === 'vo_c' ? GROUPS : []) {
      const ov = overrides.get(g.id);
      const relief = g.reliefKnown ? bytesOf(g) : p.relief_policy === 'entry_allocation_whatif' && g.basis !== 'unknown' ? bytesOf(g) : null;
      let pre: PreRejectReason | null = null;
      if (g.coverage !== 'complete') pre = 'group_coverage_incomplete';
      else if (g.shared) pre = 'shares_objects_with_other_group';
      else if (relief === null) pre = 'relief_unknown';
      if (pre || relief === null) { preRejected.push({ group_id: g.id, reason: pre ?? 'relief_unknown' }); continue; }
      if (!g.reliefKnown) assumptions.push(`${g.name}: relief taken as its entry allocation (what-if, not verified).`);

      let heat: number | null = null;
      let basis: HeatBasis = 'unknown';
      if (ov && typeof ov.heat === 'number') { heat = ov.heat; basis = 'assumed'; assumptions.push(`${g.name}: heat assumed ${ov.heat} by the owner.`); }
      else if (p.heat_policy === 'mtime_proxy_whatif') {
        const age = (now().getTime() - Date.parse(g.modified)) / 86_400_000;
        heat = Math.max(0, Math.min(1, 1 - age / 365)); basis = 'mtime_proxy';
      }
      if (basis === 'mtime_proxy' && !assumptions.some((a) => a.startsWith('mtime proxy'))) assumptions.push('mtime proxy: modification age stands in for heat. It is not access heat.');
      const pinned = ov?.pinned !== undefined ? ov.pinned : g.pinned;
      const active = ov?.active !== undefined ? ov.active : g.active;
      const prot = ov?.protected !== undefined ? ov.protected : g.protected;
      if (ov && (ov.pinned !== undefined || ov.active !== undefined || ov.protected !== undefined)) assumptions.push(`${g.name}: pinned ${pinned}, active ${active}, protected ${prot} by the owner's assumption.`);
      let reason: RejectReason | null = null;
      if (pinned !== false || active !== false || prot !== false) reason = 'pinned_active_protected_or_unspecified';
      else if (heat === null) reason = 'heat_unknown';
      else if (heat > 0.25) reason = 'not_cold';
      else if (g.days === null || g.days < 7) reason = 'cooldown_or_history_unknown';
      if (reason) rejected.push({ group_id: g.id, reason });
      else eligible.push({ g, relief, heat: heat! });
    }

    // Exhaustive over subsets (at most 2^9), first target with room: lexicographic shortfall, transfer, count.
    const need = goal > free(source) ? goal - free(source) : 0n;
    const room = new Map(targets.map((t) => [t.id, free(t) - t.capacity / 50n]));
    let best: { picks: { g: GroupSeed; to: string; relief: bigint }[]; short: bigint; transfer: bigint } = { picks: [], short: need, transfer: 0n };
    let visited = 0;
    for (let mask = 1; mask < 1 << eligible.length; mask++) {
      visited++;
      const left = new Map(room);
      const picks: typeof best.picks = [];
      let transfer = 0n;
      let relief = 0n;
      let ok = true;
      for (let i = 0; i < eligible.length && ok; i++) {
        if (!(mask & (1 << i))) continue;
        const e = eligible[i]!;
        const c = candidate(e.g);
        const to = targets.find((t) => (left.get(t.id) ?? 0n) >= BigInt(c.destination_bytes));
        transfer += BigInt(c.transfer_bytes);
        if (!to || transfer > budget) { ok = false; break; }
        left.set(to.id, left.get(to.id)! - BigInt(c.destination_bytes));
        picks.push({ g: e.g, to: to.id, relief: e.relief });
        relief += e.relief;
      }
      if (!ok) continue;
      const short = need > relief ? need - relief : 0n;
      if (short < best.short || (short === best.short && (transfer < best.transfer || (transfer === best.transfer && picks.length < best.picks.length)))) best = { picks, short, transfer };
    }
    const chosen = new Set(best.picks.map((x) => x.g.id));
    for (const e of eligible) if (!chosen.has(e.g.id)) rejected.push({ group_id: e.g.id, reason: 'not_selected_by_bounded_allocator' });

    const proposals: PlacementProposal[] = best.picks.map((x) => {
      const c = candidate(x.g);
      return { group_id: x.g.id, source_id: source.id, target_id: x.to, source_bytes_relieved: str(x.relief), destination_bytes_required: c.destination_bytes, transfer_bytes: c.transfer_bytes, reason: 'bounded_capacity_budget_allocation', requires_consent: true, executable: false };
    });
    const projected: Record<string, ByteCount> = {};
    for (const v of VOLUMES) if (v.id === source.id || targets.some((t) => t.id === v.id)) projected[v.id] = str(free(v));
    projected[source.id] = str(free(source) + best.picks.reduce((s, x) => s + x.relief, 0n));
    for (const x of best.picks) projected[x.to] = str(BigInt(projected[x.to]!) - BigInt(candidate(x.g).destination_bytes));
    const transfer = best.picks.reduce((s, x) => s + BigInt(candidate(x.g).transfer_bytes), 0n);
    return {
      mode: 'simulation', algorithm: 'bounded_portfolio_search_v2', optimality_claim: false, shortfall_optimal: true,
      optimality_scope: 'Static supplied estimates; lexicographic shortfall, transfer, group count, heat, IDs only. Not a claim about real disks.',
      search: { complete: true, nodes_visited: visited, node_budget: p.node_budget, reason: 'exhausted', eligible_groups: eligible.length, eligible_targets: targets.length, lower_bound_shortfall_bytes: str(best.short) },
      proposals, rejected, pre_rejected: preRejected, excluded_volumes: excluded, relief_policy: p.relief_policy,
      projected_free_bytes: projected, target_free_bytes: str(goal), shortfall_bytes: str(best.short), satisfied: best.short === 0n, transfer_bytes: str(transfer),
      filesystem_changed: false, baseline_shortfall_bytes: str(need), shortfall_improvement_bytes: str(need - best.short),
      assumption: 'Simulation only. No file is moved; every proposal needs the owner’s consent and is not executable in this build.',
      heat_policy: p.heat_policy, assumptions: [...new Set(assumptions)].slice(0, 16), root_generations: [], proposal_id: null,
    };
  };

  // --- companion ------------------------------------------------------------------------------
  const NAMES = ['browser-main', 'render-worker', 'game-launcher', 'chat-client', 'code-editor', 'sync-agent', 'media-player', 'build-server', 'db-engine', 'vm-host', 'updater', 'indexer', 'print-spooler', 'audio-svc', 'shell-host'];
  const BIG_FIXED: [string, number][] = [['browser-main.exe', 2.4], ['vm-host.exe', 6.8], ['db-engine.exe', 3.1]];
  const processRows = (): ProcessRow[] => {
    const rnd = prng(7);
    const rows: ProcessRow[] = [];
    const add = (name: string, pid: number, access: ProcessRow['access'], commit: number, owned = false) => {
      const denied = access === 'denied';
      const limited = access === 'limited';
      const ws = commit * (0.55 + rnd() * 0.5);
      rows.push({
        process_ref: `pc_${pid}_${(pid * 2654435761 >>> 0).toString(36)}`, pid, name, started_at: denied ? null : new Date(Date.UTC(2026, 9, 1, 6, 0) + pid * 1000).toISOString(),
        private_commit_bytes: denied ? null : str(gib(commit)), private_working_set_bytes: denied || limited ? null : str(gib(ws * 0.8)), working_set_bytes: denied ? null : str(gib(ws)),
        cpu_fraction: denied ? null : Math.round(rnd() * rnd() * 400) / 1000, io_read_bytes_per_s: denied ? null : Math.round(rnd() * rnd() * 4e7), io_write_bytes_per_s: denied ? null : Math.round(rnd() * rnd() * 2e7),
        gpu_dedicated_bytes: denied || rnd() < 0.7 ? null : str(gib(rnd() * 1.5)), access, loomward_owned: owned,
      });
    };
    BIG_FIXED.forEach(([n, g], i) => add(n, 4100 + i * 4, 'full', g));
    add('loomward-mock.exe', 5200, 'full', 0.09, true);
    add('protected-service.exe', 4, 'denied', 0);
    add('security-agent.exe', 1888, 'denied', 0);
    add('svchost\u200B.exe', 6012, 'limited', 0.04); // a look-alike name: the hidden character is shown as a badge
    for (let i = 0; i < 52; i++) add(`${NAMES[i % NAMES.length]}${i < NAMES.length ? '' : '-' + i}.exe`, 7000 + i * 12, i % 11 === 5 ? 'limited' : 'full', 0.02 + rnd() * rnd() * 1.6);
    return rows;
  };
  const ROWS = processRows();
  const num = (s: string | null) => (s === null ? -1 : Number(s));
  const SORT: Record<string, (r: ProcessRow) => number> = {
    private_desc: (r) => num(r.private_commit_bytes), working_set_desc: (r) => num(r.working_set_bytes), cpu_desc: (r) => r.cpu_fraction ?? -1,
    io_desc: (r) => (r.io_read_bytes_per_s ?? -1) + (r.io_write_bytes_per_s ?? -1), gpu_desc: (r) => num(r.gpu_dedicated_bytes),
  };
  const processes = (p: Record<string, unknown>): ProcessList => {
    const limit = Number.isInteger(p.limit) && (p.limit as number) >= 1 && (p.limit as number) <= 200 ? (p.limit as number) : fail('invalid_request', 'limit must be 1..200');
    const sort = typeof p.sort === 'string' && (p.sort === 'name_asc' || p.sort in SORT) ? p.sort : fail('invalid_request', 'unknown sort');
    const sorted = [...ROWS].sort(sort === 'name_asc' ? (a, b) => a.name.localeCompare(b.name) : (a, b) => SORT[sort]!(b) - SORT[sort]!(a) || a.pid - b.pid);
    return {
      sample_seq: sampleSeq, observed_at: iso(), rows: sorted.slice(0, limit), observed_count: ROWS.length, denied_count: ROWS.filter((r) => r.access === 'denied').length,
      truncated: limit < ROWS.length, note: 'Processes the operating system would not describe are counted and shown as unknown, never as zero.',
    };
  };
  const explain = (p: Record<string, unknown>): ProcessExplanation => {
    const r = ROWS.find((x) => x.process_ref === p.process_ref) ?? fail('not_found', 'no such process in the last sample');
    const facts: ProcessExplanation['facts'] = [];
    const caveats = ['Rule-based text from one sample. It says what was measured, not whether the process is needed.'];
    if (r.access === 'denied') {
      facts.push({ code: 'access_denied', text: 'Windows did not let Loomward query this process, so its figures are unknown.' });
      caveats.push('Unknown is not zero: this process may be large or idle.');
    } else {
      facts.push({ code: 'private_commit', text: `Private commit is ${formatBytes(r.private_commit_bytes)}: memory it has promised, not necessarily resident.` });
      facts.push({ code: 'working_set', text: 'Working set includes shared pages, so adding processes up overstates total use.' });
      if (r.cpu_fraction !== null) facts.push({ code: 'cpu', text: `CPU was ${(r.cpu_fraction * 100).toFixed(1)}% of the whole machine over the last interval.` });
      if (r.access === 'limited') caveats.push('Access was limited: the private working set is not reported for this process.');
    }
    if (r.loomward_owned) facts.push({ code: 'loomward_owned', text: 'This is Loomward itself. Its own budgets are below.' });
    return {
      process_ref: r.process_ref, observed_at: iso(), facts, caveats, available_actions: [],
      summary: r.access === 'denied' ? `${r.name} could not be observed.` : `${r.name} (pid ${r.pid}) is observed read-only; Loomward offers no action on it.`,
    };
  };

  const telemetry = (): TelemetrySample => {
    sampleSeq++;
    const wobble = Math.sin(sampleSeq / 3);
    const pools: PoolUse[] = [
      { pool: 'scan_enumerate', max_workers: 4, busy_workers: 0, queue_depth: 0, queue_capacity: 1024 },
      { pool: 'catalog_write', max_workers: 1, busy_workers: 0, queue_depth: 0, queue_capacity: 1024 },
      { pool: 'learning', max_workers: 1, busy_workers: 0, queue_depth: 0, queue_capacity: 16 },
      { pool: 'telemetry', max_workers: 1, busy_workers: 1, queue_depth: 0, queue_capacity: 4 },
    ];
    return {
      sample_seq: sampleSeq, observed_at: iso(), elapsed_ms: sampleSeq > 1 ? 2000 : null,
      system: {
        memory: { total_bytes: str(32n * GiB), available_bytes: str(gib(10.2 + wobble * 0.6)), commit_bytes: str(gib(27.4 + wobble * 0.3)), commit_limit_bytes: str(40n * GiB), load_fraction: 0.68 },
        cpu: { logical_cpus: 16, busy_fraction: sampleSeq > 1 ? Math.round((0.16 + wobble * 0.05) * 1000) / 1000 : null },
      },
      gpu: null, disks: null, processes: null,
      engine: { private_commit_bytes: str(gib(0.09)), working_set_bytes: str(gib(0.07)), cpu_fraction: sampleSeq > 1 ? 0.004 : null, threads: 18, pools },
    };
  };

  const budgets = (): OwnBudgets => {
    const pool = (p: PoolName, max: number, def: number, cur: number, cap: number) => ({ pool: p, max_workers: max, default_workers: def, current_workers: cur, queue_capacity: cap, scope: 'loomward_own_threads' as const });
    return {
      pools: [pool('scan_enumerate', 8, 4, 4, 1024), pool('catalog_write', 1, 1, 1, 1024), pool('learning', 2, 1, 1, 16), pool('telemetry', 1, 1, 1, 4)],
      teacher: { max_in_flight: 1, timeout_s: 120, memory_limit_bytes: str(2n * GiB), enforcement: 'job_object' },
      enforcement_note: 'These caps limit Loomward’s own threads only. No operating-system priority, affinity or memory call is made on any process.',
    };
  };

  const handlers: Record<string, Handler> = {
    'volumes.list': (): VolumeList => ({ volumes: VOLUMES.map(volumeRow) }),
    'tiers.model': model,
    'placement.candidates': candidates,
    'placement.simulate': simulate,
    'telemetry.subscribe': (p): TelemetrySubscription => {
      const id = typeof p.subscription_id === 'string' ? p.subscription_id : `sub_mock_${subscriptions.size + 1}`;
      if (typeof p.subscription_id === 'string' && !subscriptions.has(id)) fail('not_found', 'unknown or expired subscription');
      subscriptions.add(id);
      return { subscription_id: id, channels: p.channels as TelemetrySubscription['channels'], interval_ms: p.interval_ms as TelemetrySubscription['interval_ms'], expires_at: new Date(now().getTime() + 60_000).toISOString() };
    },
    'telemetry.unsubscribe': (p) => ({ subscription_id: String(p.subscription_id), stopped: subscriptions.delete(String(p.subscription_id)) }),
    'telemetry.snapshot': telemetry,
    'processes.list': processes,
    'processes.explain': explain,
    'budgets.get': budgets,
    'roots.revoke': (p): RootRevokeResult => {
      const id = typeof p.root_id === 'string' ? p.root_id : fail('invalid_request', 'root_id');
      if (typeof p.purge_catalog !== 'boolean') fail('invalid_request', 'purge_catalog must be a boolean');
      if (!/^rt_mock_\d+$/.test(id)) fail('not_found', 'no such root');
      if (!revokedRoots.has(id)) stateRev++; // revoking a revoked grant returns the original revoked_at, no commit
      const at = revokedRoots.get(id) ?? iso();
      revokedRoots.set(id, at);
      return { root_id: id, revoked_at: at, purged: p.purge_catalog };
    },
    'grants.revoke': (p): GrantRevokeResult => {
      const id = typeof p.grant_id === 'string' ? p.grant_id : fail('invalid_request', 'grant_id');
      if (!/^gr_mock_/.test(id)) fail('not_found', 'no such grant');
      if (!revokedGrants.has(id)) stateRev++;
      const at = revokedGrants.get(id) ?? iso();
      revokedGrants.set(id, at);
      return { grant_id: id, revoked_at: at };
    },
  };
  return { handlers, revokedRoots, revokedGrants, stateRev: () => String(stateRev) };
}
