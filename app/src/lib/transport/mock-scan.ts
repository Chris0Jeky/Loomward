import type {
  DuplicateReport, CoverageState, ErrorBody, ErrorCode, EventKey, EventMap, Job, JobList, JobResult, Root, RootScanState, ScanProgress, SubtreeTotals,
} from '../contracts.gen';

// Invented, contract-shaped scan jobs for the mock transport: two synthetic lab roots the mock "service"
// will scan (the other mock roots are fixtures and are refused with the service's real reason). Nothing
// here reads a disk. Like the engine today, a running job carries no progress: `scan.progress` arrives once,
// at the end, followed by `tree.invalidated` and the terminal `job.state`.

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
};
export const LAB_ROOT_IDS = LABS.map((l) => l.id);

const RUN_MS = 4000;
const CANCEL_MS = 600;
const QUEUE_MS = 150;

export interface ScanMockOptions {
  now: () => Date;
  fail: FailWith;
  emit: Emit;
  isRevoked: (rootId: string) => boolean;
  /** How long a mock scan "runs" before it finishes. */
  runMs?: number;
}

export interface ScanMock {
  handlers: Record<string, (payload: Record<string, unknown>) => unknown>;
  roots(): Root[];
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
      later(o.runMs ?? RUN_MS, () => finish(job, seed!));
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
    const budget = p.budget as { max_bytes?: string; max_candidates?: number; max_ms?: number } | undefined;
    if (!budget || !Number.isInteger(budget.max_candidates) || budget.max_candidates! < 2 || budget.max_candidates! > 200 ||
      !Number.isInteger(budget.max_ms) || budget.max_ms! < 1 || budget.max_ms! > 60000 || typeof budget.max_bytes !== 'string' || !/^(0|[1-9][0-9]*)$/.test(budget.max_bytes)) o.fail('invalid_request', 'duplicate budget');
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
          truncated, stop: byteStop ? 'byte_budget' : truncated ? 'entry_budget' : null };
        set(job, { state: 'completed', finished_at: iso(), coverage: 'partial', duplicates: report });
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
      unique_objects: null, unique_allocated_bytes: null, multi_link_entries: 0, // the mock has no native file IDs: unknown, not zero
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
      'duplicates.cancel': (p) => { if (find(p.job_id).kind !== 'duplicates') o.fail('invalid_request', 'not a duplicates job'); return cancel(p); },
      'scan.start': start,
      'scan.cancel': cancel,
      'jobs.get': (p) => ({ job: { ...find(p.job_id) } }),
      'jobs.list': (p): JobList => {
        const kinds = Array.isArray(p.kinds) ? p.kinds : null;
        const limit = Number.isInteger(p.limit) && (p.limit as number) >= 1 && (p.limit as number) <= 200 ? (p.limit as number) : o.fail('invalid_request', 'limit must be 1..200');
        return { jobs: jobs.filter((j) => !kinds || kinds.includes(j.kind)).slice(0, limit).map((j) => ({ ...j })) };
      },
    },
    roots: () =>
      LABS.map((s): Root => ({
        root_id: s.id, display_path: { text: s.name, truncated: false }, origin: 'lab_generated', dataset_class: 'synthetic', volume_id: 'vo_g',
        granted_at: '2026-10-01T09:00:00Z', granted_via: 'fixture', grant_state: o.isRevoked(s.id) ? 'revoked' : 'active', ...scanState(s),
      })),
  };
}
