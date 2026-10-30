import coverageFixture from '../../../../contracts/v3/examples/commands/coverage.result.result.json';
import type {
  CoverageReport, DuplicateReport, CoverageState, ErrorBody, ErrorCode, EventKey, EventMap, Job, JobList, JobResult, Root, RootScanState, ScanProgress, SubtreeTotals,
} from '../contracts.gen';

// Invented, contract-shaped scan jobs for the mock transport: two synthetic lab roots the mock "service"
// will scan (the other mock roots are fixtures and are refused with the service's real reason). Nothing
// here reads a disk. A running job carries an interim `scan.progress` sample; final progress is followed
// by `tree.invalidated` and the terminal `job.state`, as in the engine.

export type FailWith = (code: ErrorCode, message: string, detail?: ErrorBody['detail']) => never;
export type Emit = <E extends EventKey>(event: E, data: EventMap[E]) => void;

interface LabSeed {
  id: string; name: string; files: number; dirs: number; bytes: bigint; unreadable: number; coverage: 'complete' | 'partial';
}
// rt_mock_<n> so the view mock's root revocation (which accepts that shape) works on them too.
const LABS: LabSeed[] = [
  { id: 'rt_mock_10', name: '[mock] lab-complete', files: 1284, dirs: 212, bytes: 3_412_001_792n, unreadable: 0, coverage: 'complete' },
  { id: 'rt_mock_11', name: '[mock] lab-unreadable-folders', files: 947, dirs: 160, bytes: 1_907_388_416n, unreadable: 4, coverage: 'partial' },
];
export const DUPLICATES_FIXTURE: DuplicateReport = {
  entries: ['equal-a.txt', 'equal-b.txt', 'linked.txt', 'alias.txt', 'changed.txt'].map((relative_path) => ({ relative_path, logical_bytes: '16' })),
  groups: [[0, 1]], links: [[2, 3]], links_not_compared: 2,
  skipped: [{ entry: 4, reason: 'changed_during_read' }], bytes_read: '96', partial: true, truncated: false, stop: null,
  candidate_space: { files: 12, bytes: '192' }, entries_compared: 2,
};
export const LAB_ROOT_IDS = LABS.map((l) => l.id);

const RUN_MS = 4000;
const CANCEL_MS = 600;
const QUEUE_MS = 150;
const OBSERVE_MS = 1200;
const COVERS_MS = 1500;

export interface ScanMockOptions {
  now: () => Date;
  fail: FailWith;
  emit: Emit;
  isRevoked: (rootId: string) => boolean;
  /** How long a mock scan "runs" before it finishes. */
  runMs?: number;
  /** Catalogue file node ids the mock coverage report may name, so its Explorer links open. */
  fileNodes?: () => string[];
}

export interface ScanMock {
  handlers: Record<string, (payload: Record<string, unknown>) => unknown>;
  roots(): Root[];
  revoke(rootId: string): void;
  /** A short fake `reclaim` job (R1g): progress, then completion, which calls `onComplete`. One at a time, as the engine. */
  observe(onComplete: () => void): JobResult;
}

export function createScanMock(o: ScanMockOptions): ScanMock {
  const iso = () => o.now().toISOString();
  const jobs: Job[] = [];
  const done = new Map<string, { job: Job; seed: LabSeed }>(); // latest finished scan per root, for the roots view
  let n = 0;
  let generation = 1;

  const later = (ms: number, fn: () => void) => void setTimeout(fn, ms);
  const find = (id: unknown): Job => jobs.find((j) => j.job_id === id) ?? o.fail('not_found', 'no such job');
  const active = (j: Job) => j.state === 'queued' || j.state === 'running' || j.state === 'cancel_requested';
  const set = (j: Job, patch: Partial<Job>) => {
    Object.assign(j, patch);
    j.rev++;
    o.emit('job.state', { job: { ...j } } satisfies JobResult);
  };

  const progressOf = (s: LabSeed, elapsed: number): ScanProgress => ({
    phase: 'done', strategy: 'std_read_dir', examined: s.files + s.dirs + s.unreadable, indexed_files: s.files, indexed_dirs: s.dirs,
    skipped: 0, failed: s.unreadable, logical_bytes: s.bytes.toString(), allocated_bytes: ((s.bytes * 21n) / 20n).toString(), pending_dirs: 0, writer_queue_depth: 0,
    entries_per_second: Math.round((s.files + s.dirs) / Math.max(1, elapsed / 1000)), elapsed_ms: elapsed, limit_hit: false,
  });

  function finish(j: Job, s: LabSeed): void {
    if (j.state !== 'running') return; // cancelled meanwhile
    const started = Date.parse(j.started_at ?? j.created_at);
    const progress = progressOf(s, Math.max(0, o.now().getTime() - started));
    o.emit('scan.progress', { job_id: j.job_id, root_id: s.id, progress });
    set(j, { progress });
    o.emit('tree.invalidated', { root_id: s.id, generation: String(++generation), scope: 'all', node_ids: [] });
    set(j, { state: 'completed', finished_at: iso(), progress, coverage: s.coverage });
    done.set(s.id, { job: j, seed: s });
  }

  function start(p: Record<string, unknown>): JobResult {
    const id = typeof p.root_id === 'string' ? p.root_id : o.fail('invalid_request', 'root_id must be a string');
    if (p.mode !== 'full' && p.mode !== 'refresh') o.fail('invalid_request', 'mode must be full or refresh');
    if (!/^rt_mock_\d+$/.test(id)) o.fail('not_found', 'unknown root');
    if (o.isRevoked(id)) o.fail('permission_denied', "the root's grant is not active");
    const seed = LABS.find((l) => l.id === id);
    if (!seed) o.fail('permission_denied', 'a synthetic session scans registered lab roots only', { reason: 'synthetic_session_requires_lab_root' });
    const running = jobs.find((j) => j.kind === 'scan' && j.root_id === id && active(j));
    if (running) o.fail('busy', 'a scan of this root is already running', { job_id: running.job_id });
    const job: Job = { job_id: `jb_mock_${++n}`, seq: n, rev: 0, kind: 'scan', state: 'queued', root_id: id, created_at: iso(), started_at: null, finished_at: null, progress: null, coverage: null, error: null };
    jobs.unshift(job);
    o.emit('job.state', { job: { ...job } });
    later(QUEUE_MS, () => {
      if (job.state !== 'queued') return;
      set(job, { state: 'running', started_at: iso() });
      const runMs = o.runMs ?? RUN_MS;
      later(runMs / 2, () => {
        if (job.state !== 'running') return;
        const progress: ScanProgress = {
          ...progressOf({ ...seed!, files: Math.floor(seed!.files / 2), dirs: Math.floor(seed!.dirs / 2), bytes: seed!.bytes / 2n, unreadable: 0 },
            Math.max(0, o.now().getTime() - Date.parse(job.started_at!))),
          phase: 'enumerating', pending_dirs: 1,
        };
        o.emit('scan.progress', { job_id: job.job_id, root_id: id, progress });
        set(job, { progress });
      });
      later(runMs, () => finish(job, seed!));
    });
    return { job: { ...job } };
  }

  function cancel(p: Record<string, unknown>): JobResult {
    const job = find(p.job_id);
    if (!active(job)) return { job: { ...job } }; // idempotent: a finished job stays as it ended
    if (job.state !== 'cancel_requested') {
      set(job, { state: 'cancel_requested' });
      later(CANCEL_MS, () => set(job, { state: 'cancelled', finished_at: iso(), coverage: 'cancelled' }));
    }
    return { job: { ...job } };
  }

  function inspect(p: Record<string, unknown>): JobResult {
    const id = String(p.root_id);
    if (o.isRevoked(id)) o.fail('permission_denied', 'grant_not_active');
    if (!LAB_ROOT_IDS.includes(id)) o.fail('permission_denied', 'Synthetic inspection requires a registered lab root.');
    if (jobs.some((j) => j.kind === 'duplicates' && j.root_id === id && active(j))) o.fail('busy', 'inspection active');
    const budget = p.budget as { max_bytes?: string; max_candidates?: number; max_ms?: number; min_bytes?: unknown } | undefined;
    const validBytes = (value: unknown) => typeof value === 'string' && /^(0|[1-9][0-9]{0,19})$/.test(value) && BigInt(value) <= 18446744073709551615n;
    if (!budget || !Number.isInteger(budget.max_candidates) || budget.max_candidates! < 2 || budget.max_candidates! > 200 ||
      !Number.isInteger(budget.max_ms) || budget.max_ms! < 1 || budget.max_ms! > 60000 || !validBytes(budget.max_bytes) || ('min_bytes' in budget && !validBytes(budget.min_bytes))) o.fail('invalid_request', 'duplicate budget');
    const job: Job = { job_id: `jb_mock_${++n}`, seq: n, rev: 0, kind: 'duplicates', state: 'queued', root_id: id, created_at: iso(), started_at: null, finished_at: null, progress: null, coverage: null, error: null };
    jobs.unshift(job);
    o.emit('job.state', { job: { ...job } });
    later(QUEUE_MS, () => {
      if (job.state !== 'queued') return;
      set(job, { state: 'running', started_at: iso() });
      later(1000, () => {
        if (job.state !== 'running') return;
        if (o.isRevoked(id)) { set(job, { state: 'failed', finished_at: iso(), error: { code: 'permission_denied', message: 'cancelled', retryable: false, detail: null } }); return; }
        const byteStop = BigInt(budget!.max_bytes!) < 96n;
        const truncated = budget!.max_candidates! < 5;
        const entries = DUPLICATES_FIXTURE.entries.slice(0, budget!.max_candidates);
        const report: DuplicateReport = { ...DUPLICATES_FIXTURE, entries, groups: byteStop ? [] : [[0, 1]],
          links: entries.length >= 4 ? [[2, 3]] : [], links_not_compared: entries.length >= 4 ? 2 : 0,
          skipped: entries.length === 5 ? DUPLICATES_FIXTURE.skipped : [], bytes_read: byteStop ? '0' : '96',
          entries_compared: byteStop ? 0 : 2,
          truncated, stop: byteStop ? 'byte_budget' : truncated ? 'entry_budget' : null };
        set(job, { state: 'completed', finished_at: iso(), coverage: 'partial', duplicates: report });
      });
    });
    return { job: { ...job } };
  }

  function observe(onComplete: () => void): JobResult {
    const running = jobs.find((j) => j.kind === 'reclaim' && active(j));
    if (running) o.fail('busy', 'a reclaim observation is already running', { job_id: running.job_id });
    const job: Job = { job_id: `jb_mock_${++n}`, seq: n, rev: 0, kind: 'reclaim', state: 'queued', root_id: null, created_at: iso(), started_at: null, finished_at: null, progress: null, coverage: null, error: null, reclaim: { candidates_total: 1, candidates_done: 0, names_checked: 0 } };
    jobs.unshift(job);
    o.emit('job.state', { job: { ...job } });
    later(QUEUE_MS, () => {
      if (job.state !== 'queued') return;
      set(job, { state: 'running', started_at: iso() });
      later(OBSERVE_MS / 2, () => { if (job.state === 'running') set(job, { reclaim: { candidates_total: 1, candidates_done: 0, names_checked: 4200 } }); });
      later(OBSERVE_MS, () => {
        if (job.state !== 'running') return; // cancelled meanwhile: nothing stored, relief stays unknown
        onComplete();
        set(job, { state: 'completed', finished_at: iso(), coverage: 'complete', reclaim: { candidates_total: 1, candidates_done: 1, names_checked: 8400 } });
      });
    });
    return { job: { ...job } };
  }

  /** `coverage.start` (#1106): one at a time, as the engine. From the first lab to the second the report is partial (a spent budget); the other way round it is a finished answer with two files not found. Nothing is read. */
  const reports = new Map<string, CoverageReport>();
  function coverStart(p: Record<string, unknown>): JobResult {
    const known = ['a_root_id', 'b_root_id', 'budget_bytes', 'budget_secs'];
    if (Object.keys(p).some((k) => !known.includes(k))) o.fail('invalid_request', 'Unexpected field.');
    const [a, b] = [p.a_root_id, p.b_root_id];
    if (typeof a !== 'string' || typeof b !== 'string') o.fail('invalid_request', 'a_root_id and b_root_id must be strings');
    if (a === b) o.fail('invalid_request', 'choose two different folders to compare', { reason: 'roots_must_differ' });
    for (const id of [a, b] as string[]) {
      if (o.isRevoked(id)) o.fail('permission_denied', 'grant_not_active');
      if (!LAB_ROOT_IDS.includes(id)) o.fail('permission_denied', 'Synthetic coverage requires registered lab roots.');
    }
    const bytes = p.budget_bytes, secs = p.budget_secs;
    if ((bytes !== undefined && !(typeof bytes === 'string' && /^(0|[1-9][0-9]{0,19})$/.test(bytes) && BigInt(bytes) <= 18446744073709551615n))
      || (secs !== undefined && !(Number.isInteger(secs) && (secs as number) >= 1 && (secs as number) <= 3600))) o.fail('invalid_request', 'coverage budget');
    const running = jobs.find((j) => j.kind === 'coverage' && active(j));
    if (running) o.fail('busy', 'coverage check already running', { job_id: running.job_id });
    const job: Job = { job_id: `jb_mock_${++n}`, seq: n, rev: 0, kind: 'coverage', state: 'queued', root_id: a as string, created_at: iso(), started_at: null, finished_at: null, progress: null, coverage: null, error: null };
    jobs.unshift(job);
    o.emit('job.state', { job: { ...job } });
    later(QUEUE_MS, () => {
      if (job.state !== 'queued') return;
      set(job, { state: 'running', started_at: iso() });
      later(COVERS_MS, () => {
        if (job.state !== 'running') return; // cancelled meanwhile: no result is kept
        if (o.isRevoked(a as string) || o.isRevoked(b as string)) { set(job, { state: 'failed', finished_at: iso(), error: { code: 'permission_denied', message: 'cancelled', retryable: false, detail: null } }); return; }
        const report = structuredClone(coverageFixture) as CoverageReport;
        report.job_id = job.job_id; report.a_root_id = a as string; report.b_root_id = b as string; report.observed_at = iso();
        const files = o.fileNodes?.() ?? [];
        report.not_covered.forEach((item, i) => { item.node_id = i === 0 ? files[0] ?? null : null; }); // the second stays 'not in the catalogue yet'
        if (a === LAB_ROOT_IDS[1]) {
          report.outcome = 'complete'; report.reasons = []; report.unknown = []; report.totals.unknown = { files: 0, bytes: '0', size_unknown: 0 };
        }
        reports.set(job.job_id, report);
        set(job, { state: 'completed', finished_at: iso(), coverage: report.outcome === 'complete' ? 'complete' : 'partial' });
      });
    });
    return { job: { ...job } };
  }

  const scanState = (s: LabSeed): { scan: RootScanState; totals: SubtreeTotals | null } => {
    const last = done.get(s.id);
    const running = jobs.some((j) => j.kind === 'scan' && j.root_id === s.id && active(j));
    if (o.isRevoked(s.id)) return { scan: { state: 'stale', last_job_id: null, generation: null, finished_at: null, coverage: 'stale' }, totals: null };
    const cov: CoverageState = last ? last.seed.coverage : 'partial';
    const totals: SubtreeTotals | null = !last ? null : {
      files: s.files, dirs: s.dirs, logical_bytes: s.bytes.toString(), allocated_bytes: ((s.bytes * 21n) / 20n).toString(), allocation_unknown_files: 0,
      skipped: 0, failed: s.unreadable, complete: s.coverage === 'complete', stream_coverage: 'default_stream_only',
      unique_objects: null, unique_allocated_bytes: null, multi_link_entries: null, // the mock has no native file IDs to count
    };
    if (running) return { scan: { state: 'scanning', last_job_id: null, generation: last ? String(generation) : null, finished_at: last?.job.finished_at ?? null, coverage: 'partial' }, totals };
    if (!last) {
      const cancelled = jobs.some((j) => j.root_id === s.id && j.state === 'cancelled');
      return { scan: { state: cancelled ? 'partial' : 'never_scanned', last_job_id: null, generation: null, finished_at: null, coverage: cancelled ? 'partial' : 'unscanned' }, totals: null };
    }
    return { scan: { state: cov === 'complete' ? 'complete' : 'partial', last_job_id: null, generation: String(generation), finished_at: last.job.finished_at, coverage: cov }, totals };
  };

  return {
    handlers: {
      'duplicates.inspect': inspect,
      'duplicates.cancel': (p) => { if (find(p.job_id).kind !== 'duplicates') o.fail('invalid_request', 'not a duplicates job'); return { job: { ...cancel(p).job, duplicates: undefined } }; },
      'coverage.start': coverStart,
      'coverage.result': (p): CoverageReport => reports.get(String(p.job_id)) ?? o.fail('not_found', 'no coverage result for this job'),
      'scan.start': start,
      'scan.cancel': cancel,
      'jobs.cancel': cancel,
      'jobs.get': (p) => ({ job: { ...find(p.job_id) } }),
      'jobs.list': (p): JobList => {
        const kinds = Array.isArray(p.kinds) ? p.kinds : null;
        const limit = Number.isInteger(p.limit) && (p.limit as number) >= 1 && (p.limit as number) <= 200 ? (p.limit as number) : o.fail('invalid_request', 'limit must be 1..200');
        return { jobs: jobs.filter((j) => !kinds || kinds.includes(j.kind)).slice(0, limit).map((j) => ({ ...j })) };
      },
    },
    observe,
    revoke: (rootId) => {
      for (const [id, report] of reports) if (report.a_root_id === rootId || report.b_root_id === rootId) reports.delete(id);
      for (const job of jobs.filter((j) => j.kind === 'coverage' && active(j))) cancel({ job_id: job.job_id });
      for (const job of jobs.filter((j) => j.root_id === rootId && j.kind === 'duplicates')) {
        if (job.duplicates) set(job, { duplicates: undefined });
        if (active(job)) cancel({ job_id: job.job_id });
      }
    },
    roots: () =>
      LABS.map((s): Root => ({
        root_id: s.id, display_path: { text: s.name, truncated: false }, origin: 'lab_generated', dataset_class: 'synthetic', volume_id: 'vo_g',
        granted_at: '2026-10-01T09:00:00Z', granted_via: 'fixture', grant_state: o.isRevoked(s.id) ? 'revoked' : 'active', ...scanState(s),
      })),
  };
}
