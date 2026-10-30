import { describe, expect, it } from 'vitest';
import type { ErrorCode, Job, JobState, ScanProgress } from '../../src/lib/contracts.gen';
import { announcement, applyJob, applyProgress, fromList, isActive, refusalText, viewOf, type Book } from '../../src/lib/scan/model';

const LEGACY_REV = undefined as unknown as number; // Older responses omitted rev.
const T0 = Date.parse('2026-10-10T10:00:00Z');
const progress = (over: Partial<ScanProgress> = {}): ScanProgress => ({
  phase: 'done', strategy: 'std_read_dir', examined: 100, indexed_files: 80, indexed_dirs: 20, skipped: 0, failed: 0, logical_bytes: '2048', allocated_bytes: '4096',
  pending_dirs: 0, writer_queue_depth: 0, entries_per_second: 100, elapsed_ms: 3200, limit_hit: false, ...over,
});
const job = (state: JobState, over: Partial<Job> = {}): Job => ({
  job_id: 'jb_1', seq: 1, rev: { queued: 0, running: 1, cancel_requested: 2, completed: 3, failed: 3, cancelled: 3 }[state], kind: 'scan', state, root_id: 'rt_a', created_at: '2026-10-10T10:00:00Z',
  started_at: state === 'queued' ? null : '2026-10-10T10:00:01Z', finished_at: null, progress: null, coverage: null, error: null, ...over,
});
const done = (over: Partial<Job> = {}) => job('completed', { finished_at: '2026-10-10T10:00:05Z', coverage: 'complete', progress: progress(), ...over });

it('names a depth-limited scan and keeps its lower-bound warning', () => {
  const p = { ...progress({ limit_hit: true }), limit_reason: 'depth_limit' as const };
  const view = viewOf(done({ coverage: 'partial', progress: p }), T0);
  expect(view.activity).toBe('partial');
  expect(view.headline).toBe('Stopped at its directory depth limit');
  expect(view.detail).toContain('Totals are lower bounds');
});

it('names a directory-limited scan, and an unnamed cap stays the entry budget', () => {
  const named = { ...progress({ limit_hit: true }), limit_reason: 'directory_limit' as const };
  expect(viewOf(done({ coverage: 'partial', progress: named }), T0).headline).toBe('Stopped at its directory limit');
  const unnamed = progress({ limit_hit: true });
  expect(viewOf(done({ coverage: 'partial', progress: unnamed }), T0).headline).toBe('Stopped at its entry budget');
});

it('retains the depth-limit reason when merging legacy progress', () => {
  const p = { ...progress({ limit_hit: true }), limit_reason: 'depth_limit' as const };
  const book = applyJob({}, done({ rev: LEGACY_REV, coverage: 'partial', progress: p }));
  const merged = applyJob(book, done({ rev: LEGACY_REV, coverage: 'partial', progress: progress() }));
  expect(viewOf(merged.rt_a!, T0).headline).toBe('Stopped at its directory depth limit');
});

describe('applying job snapshots', () => {
  it('follows queued, running, cancel requested, cancelled', () => {
    let b: Book = {};
    for (const s of ['queued', 'running', 'cancel_requested', 'cancelled'] as const) {
      b = applyJob(b, job(s));
      expect(b.rt_a?.state).toBe(s);
    }
    expect(viewOf(b.rt_a!, T0).activity).toBe('cancelled');
  });

  it('never moves a job backwards: a late or replayed event is ignored', () => {
    const b = applyJob(applyJob({}, job('cancel_requested')), job('running'));
    expect(b.rt_a?.state).toBe('cancel_requested');
    expect(applyJob(b, job('queued')).rt_a?.state).toBe('cancel_requested');
  });

  it('keeps a terminal state final, even against a different terminal state', () => {
    const b = applyJob({}, job('cancelled', { finished_at: '2026-10-10T10:00:02Z' }));
    expect(applyJob(b, done()).rt_a?.state).toBe('cancelled');
    expect(applyJob(b, job('running')).rt_a?.state).toBe('cancelled');
  });

  it('lets a newer job replace the root\'s last one, and never an older one', () => {
    const first = applyJob({}, done());
    const second = applyJob(first, job('queued', { job_id: 'jb_2', seq: 2, created_at: '2026-10-10T11:00:00Z' }));
    expect(second.rt_a?.job_id).toBe('jb_2');
    expect(applyJob(second, done({ job_id: 'jb_0', seq: 0, created_at: '2026-10-10T09:00:00Z' })).rt_a?.job_id).toBe('jb_2');
  });

  it('orders submissions by seq despite equal times, clock rollback, state and random IDs', () => {
    const older = done({ job_id: 'jb_z', seq: 4, created_at: '2026-10-10T12:00:00Z' });
    const newer = job('queued', { job_id: 'jb_a', seq: 5 });
    for (const pair of [[older, newer], [newer, older]]) expect(fromList(pair).rt_a).toEqual(newer);
    const equalTime = done({ ...older, created_at: newer.created_at });
    for (const pair of [[equalTime, newer], [newer, equalTime]]) expect(fromList(pair).rt_a).toEqual(newer);
  });

  it('falls back to creation time if either snapshot has no submission sequence', () => {
    const older = done({ job_id: 'jb_old', seq: 20 });
    const newer = job('queued', { job_id: 'jb_new', seq: 21, created_at: '2026-10-10T11:00:00Z' });
    for (const missing of [older, newer]) {
      const legacy = { ...missing, seq: undefined as unknown as number };
      const pair = missing === older ? [legacy, newer] : [older, legacy];
      expect(fromList(pair).rt_a?.job_id).toBe('jb_new');
      expect(fromList([...pair].reverse()).rt_a?.job_id).toBe('jb_new');
    }
  });

  it('keeps the book object for stale and duplicate revisions', () => {
    const current = done({ rev: 5 });
    const book = applyJob({}, current);
    for (const rev of [4, 5]) expect(applyJob(book, { ...current, rev })).toBe(book);
    expect(applyJob(book, { ...current, rev: 6 }).rt_a?.rev).toBe(6);
  });

  it('accepts only higher revisions in either delivery order, without merging stale fields', () => {
    const older = done({ rev: 4, progress: progress({ examined: 120 }), coverage: 'complete' });
    const newer = done({ rev: 5, progress: progress({ examined: 100 }), coverage: null });
    for (const pair of [[older, newer], [newer, older]]) expect(fromList(pair).rt_a).toEqual(newer);
    const book = applyJob({}, newer);
    expect(applyJob(book, { ...older, rev: 5 }).rt_a).toEqual(newer);
    expect(applyJob(book, job('failed', { rev: 3 })).rt_a).toEqual(newer);
    expect(applyProgress(book, 'rt_a', 'jb_1', progress({ examined: 120 }))).toBe(book);
    expect(applyJob(applyJob({}, job('running', { rev: 10 })), done({ rev: 9 })).rt_a?.state).toBe('running');
  });

  it('ignores jobs that are not scans or have no root', () => {
    expect(applyJob({}, job('running', { kind: 'refit' }))).toEqual({});
    expect(applyJob({}, job('running', { root_id: null }))).toEqual({});
  });

  it('never erases known coverage with a replayed same-state snapshot that carries none', () => {
    const finished = applyJob({}, done({ rev: LEGACY_REV }));
    const replay = applyJob(finished, job('completed', { rev: LEGACY_REV, finished_at: '2026-10-10T10:00:05Z', coverage: null, progress: null }));
    expect(replay.rt_a?.coverage).toBe('complete');
    expect(viewOf(replay.rt_a!, T0).activity).toBe('completed');
    expect(viewOf(replay.rt_a!, T0).headline).toBe('Finished, complete coverage');
  });

  it('never erases a known error, start or finish time with a null from a same-state snapshot', () => {
    const error = { code: 'internal_error', message: 'catalogue writer stopped', retryable: false, detail: null } as const;
    const failed = applyJob({}, job('failed', { rev: LEGACY_REV, error, finished_at: '2026-10-10T10:00:04Z' }));
    const replay = applyJob(failed, job('failed', { rev: LEGACY_REV, error: null, started_at: null, finished_at: null }));
    expect(replay.rt_a).toMatchObject({ error, started_at: '2026-10-10T10:00:01Z', finished_at: '2026-10-10T10:00:04Z' });
    expect(viewOf(replay.rt_a!, T0).detail).toBe('catalogue writer stopped');
  });

  it('keeps the newer counts when an older snapshot of the same job arrives late', () => {
    const newer = applyJob({}, done({ rev: LEGACY_REV, progress: progress({ examined: 100, indexed_files: 80 }) }));
    const stale = applyJob(newer, done({ rev: LEGACY_REV, progress: progress({ examined: 40, indexed_files: 30 }) }));
    expect(stale.rt_a?.progress?.indexed_files).toBe(80);
  });

  it('merges every monotonic progress counter independently in either arrival order', () => {
    const a = done({ rev: LEGACY_REV, progress: progress({ examined: 100, indexed_files: 80, indexed_dirs: 20, skipped: 8, failed: 4,
      logical_bytes: '9007199254740993', allocated_bytes: '9007199254740995', elapsed_ms: 4000, limit_hit: true }) });
    const b = done({ rev: LEGACY_REV, progress: progress({ examined: 120, indexed_files: 70, indexed_dirs: 10, skipped: 2, failed: 1,
      logical_bytes: '9007199254740992', allocated_bytes: null, elapsed_ms: 3000, pending_dirs: 2, writer_queue_depth: 3 }) });
    for (const [first, second] of [[a, b], [b, a]]) {
      expect(applyJob(applyJob({}, first!), second!).rt_a?.progress).toMatchObject({
        examined: 120, indexed_files: 80, indexed_dirs: 20, skipped: 8, failed: 4,
        logical_bytes: '9007199254740993', allocated_bytes: '9007199254740995', elapsed_ms: 4000, limit_hit: true,
        pending_dirs: 2, writer_queue_depth: 3,
      });
    }
  });

  it('keeps the progress already seen when a later snapshot carries none', () => {
    let b = applyJob({}, job('running', { rev: LEGACY_REV }));
    b = applyProgress(b, 'rt_a', 'jb_1', progress());
    b = applyJob(b, done({ rev: LEGACY_REV, progress: null }));
    expect(b.rt_a?.progress?.indexed_files).toBe(80);
  });

  it('drops progress for a job the book does not hold', () => {
    const b = applyJob({}, job('running'));
    expect(applyProgress(b, 'rt_a', 'jb_other', progress())).toBe(b);
    expect(applyProgress(b, 'rt_x', 'jb_1', progress())).toBe(b);
  });

  it('never lets an unreadable created_at outrank a readable one when seq is missing', () => {
    const readable = job('queued', { job_id: 'jb_ok', seq: undefined, created_at: '2026-10-10T10:00:00Z' });
    const unreadable = job('queued', { job_id: 'jb_bad', seq: undefined, created_at: 'not a time' });
    expect(applyJob(applyJob({}, unreadable), readable).rt_a?.job_id).toBe('jb_ok');
    expect(applyJob(applyJob({}, readable), unreadable).rt_a?.job_id).toBe('jb_ok');
  });

  it('rebuilds from a jobs.list in any order', () => {
    const old = done({ job_id: 'jb_0', seq: 0, created_at: '2026-10-10T09:00:00Z' });
    const fresh = job('running', { job_id: 'jb_2', seq: 2, created_at: '2026-10-10T11:00:00Z' });
    expect(fromList([fresh, old]).rt_a?.job_id).toBe('jb_2');
    expect(fromList([old, fresh]).rt_a?.job_id).toBe('jb_2');
    expect(isActive('running') && isActive('queued') && isActive('cancel_requested') && !isActive('completed')).toBe(true);
  });
});

describe('what the view says', () => {
  it('shows a running scan as indeterminate: elapsed time, no counts, no percentage', () => {
    const v = viewOf(job('running', { progress: null }), T0 + 12_000);
    expect(v).toMatchObject({ activity: 'running', active: true, headline: 'Scanning', detail: '', elapsedMs: 11_000 });
  });

  it('keeps the running view indeterminate when an interim sample arrives', () => {
    expect(viewOf(job('running', { progress: progress() }), T0 + 2000).detail).toBe('');
  });

  it('clamps a clock that is behind the service to zero seconds', () => {
    expect(viewOf(job('running'), T0 - 60_000).elapsedMs).toBe(0);
  });

  it('measures a queued job from its creation', () => {
    expect(viewOf(job('queued'), T0 + 500)).toMatchObject({ activity: 'queued', active: true, elapsedMs: 500 });
  });

  it('says cancelling while the cancel is acknowledged but not finished', () => {
    expect(viewOf(job('cancel_requested'), T0 + 3000)).toMatchObject({ activity: 'cancelling', active: true, headline: 'Cancelling' });
  });

  it('reports complete coverage only when the service does, with the final counts', () => {
    const v = viewOf(done(), T0 + 60_000);
    expect(v).toMatchObject({ activity: 'completed', active: false, headline: 'Finished, complete coverage', elapsedMs: 3200 });
    expect(v.detail).toContain('80 files, 20 folders, 2.00 KiB logical');
  });

  it('says partial when the service says partial', () => {
    const v = viewOf(done({ coverage: 'partial', progress: progress({ failed: 4 }) }), T0);
    expect(v.activity).toBe('partial');
    expect(v.headline).toBe('Finished, partial coverage');
    expect(v.detail).toContain('4 could not be read');
    expect(v.detail).toContain('lower bounds');
  });

  it('never says complete when entries could not be read, whatever the coverage label', () => {
    expect(viewOf(done({ coverage: 'complete', progress: progress({ failed: 1 }) }), T0).activity).toBe('partial');
  });

  it('says stopped at its budget when the limit was hit', () => {
    const v = viewOf(done({ coverage: 'complete', progress: progress({ limit_hit: true }) }), T0);
    expect(v).toMatchObject({ activity: 'partial', headline: 'Stopped at its entry budget' });
  });

  it('keeps an unknown coverage unknown, and unknown counts unknown', () => {
    const v = viewOf(done({ coverage: null, progress: null }), T0);
    expect(v.activity).toBe('partial');
    expect(v.headline).toBe('Finished, coverage not reported');
    expect(v.detail).toContain('Counts were not reported');
    expect(v.elapsedMs).toBe(4000); // from started_at and finished_at, since no progress carries it
  });

  it('shows the service\'s own error for a failed job', () => {
    const v = viewOf(job('failed', { error: { code: 'internal_error', message: 'catalogue writer stopped', retryable: false, detail: null } }), T0);
    expect(v).toMatchObject({ activity: 'failed', headline: 'Failed', detail: 'catalogue writer stopped' });
  });

  it('says a cancelled root is not fully covered', () => {
    expect(viewOf(job('cancelled', { coverage: 'cancelled' }), T0).detail).toContain('not published as a complete scan');
  });

  it('keeps the exact engine duration of a completed scan, not whole-second timestamps (#839)', () => {
    const v = viewOf(done({ started_at: '2026-10-10T10:00:01Z', finished_at: '2026-10-10T10:00:02Z', progress: progress({ elapsed_ms: 400 }) }), T0);
    expect(v.elapsedMs).toBe(400);
  });

  it.each(['cancelled', 'failed', 'completed'] as const)('uses the full timestamp duration for a %s job with an old sample', (state) => {
    const v = viewOf(job(state, {
      started_at: '2026-10-10T10:00:00Z', finished_at: '2026-10-10T10:10:00Z',
      progress: progress({ phase: 'enumerating', elapsed_ms: 120_000 }),
    }), T0 + 900_000);
    expect(v.elapsedMs).toBe(600_000);
  });

  it.each([
    { started_at: null, finished_at: '2026-10-10T10:10:00Z' },
    { started_at: '2026-10-10T10:00:00Z', finished_at: null },
    { started_at: 'unreadable', finished_at: '2026-10-10T10:10:00Z' },
  ])('falls back to the sample when terminal timestamps are incomplete: %j', (times) => {
    expect(viewOf(job('cancelled', { ...times, progress: progress({ elapsed_ms: 120_000 }) }), T0).elapsedMs).toBe(120_000);
  });

  it('announces the headline and the counts together', () => {
    expect(announcement(viewOf(done({ coverage: 'partial' }), T0))).toMatch(/^Finished, partial coverage\. 80 files/);
    expect(announcement(viewOf(job('running'), T0))).toBe('Scanning');
  });
});

describe('refusals in plain language', () => {
  const words = (code: ErrorCode, reason: string | null, message = 'service text') => refusalText(code, reason ? { reason } : null, message);

  it("explains a personal session's granted-folder rule", () => {
    const t = words('permission_denied', 'personal_session_requires_owner_grant');
    expect(t).toContain('only folders you granted');
    expect(t).not.toContain('service text');
  });

  it('explains a synthetic session\'s lab-root rule', () => {
    expect(words('permission_denied', 'synthetic_session_requires_lab_root')).toContain('registered lab roots only');
  });

  it('says a root is already scanning only when the service names the running job', () => {
    const withJob = refusalText('busy', { job_id: 'jb_9' }, 'root already scanning');
    expect(withJob).toContain('already running');
    // the service also answers busy for other reasons: a root starting or revoking, a writer busy, too many mutations pending
    for (const m of ['root already starting or revoking', 'root writer busy', 'too many mutations are pending; retry shortly']) {
      const t = refusalText('busy', null, m);
      expect(t).not.toContain('already running');
      expect(t).toContain(m);
    }
  });

  it('words a revoked grant and an unknown root', () => {
    expect(words('permission_denied', null)).toContain('grant is not active');
    expect(words('not_found', null)).toContain('no longer knows this root');
  });

  it('falls back to the service\'s own message, never to silence', () => {
    expect(words('internal_error', null, 'disk went away')).toBe('disk went away');
    expect(words('internal_error', null, '')).toContain('without giving a reason');
  });
});
