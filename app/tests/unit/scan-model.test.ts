import { describe, expect, it } from 'vitest';
import type { ErrorCode, Job, JobState, ScanProgress } from '../../src/lib/contracts.gen';
import { announcement, applyJob, applyProgress, fromList, isActive, refusalText, viewOf, type Book } from '../../src/lib/scan/model';

const T0 = Date.parse('2026-10-10T10:00:00Z');
const progress = (over: Partial<ScanProgress> = {}): ScanProgress => ({
  phase: 'done', strategy: 'std_read_dir', examined: 100, indexed_files: 80, indexed_dirs: 20, skipped: 0, failed: 0, logical_bytes: '2048', allocated_bytes: '4096',
  pending_dirs: 0, writer_queue_depth: 0, entries_per_second: 100, elapsed_ms: 3200, limit_hit: false, ...over,
});
const job = (state: JobState, over: Partial<Job> = {}): Job => ({
  job_id: 'jb_1', kind: 'scan', state, root_id: 'rt_a', created_at: '2026-10-10T10:00:00Z',
  started_at: state === 'queued' ? null : '2026-10-10T10:00:01Z', finished_at: null, progress: null, coverage: null, error: null, ...over,
});
const done = (over: Partial<Job> = {}) => job('completed', { finished_at: '2026-10-10T10:00:05Z', coverage: 'complete', progress: progress(), ...over });

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
    const second = applyJob(first, job('queued', { job_id: 'jb_2', created_at: '2026-10-10T11:00:00Z' }));
    expect(second.rt_a?.job_id).toBe('jb_2');
    expect(applyJob(second, done({ job_id: 'jb_0', created_at: '2026-10-10T09:00:00Z' })).rt_a?.job_id).toBe('jb_2');
  });

  it('ignores jobs that are not scans or have no root', () => {
    expect(applyJob({}, job('running', { kind: 'refit' }))).toEqual({});
    expect(applyJob({}, job('running', { root_id: null }))).toEqual({});
  });

  it('keeps the progress already seen when a later snapshot carries none', () => {
    let b = applyJob({}, job('running'));
    b = applyProgress(b, 'rt_a', 'jb_1', progress());
    b = applyJob(b, done({ progress: null }));
    expect(b.rt_a?.progress?.indexed_files).toBe(80);
  });

  it('drops progress for a job the book does not hold', () => {
    const b = applyJob({}, job('running'));
    expect(applyProgress(b, 'rt_a', 'jb_other', progress())).toBe(b);
    expect(applyProgress(b, 'rt_x', 'jb_1', progress())).toBe(b);
  });

  it('rebuilds from a jobs.list in any order', () => {
    const old = done({ job_id: 'jb_0', created_at: '2026-10-10T09:00:00Z' });
    const fresh = job('running', { job_id: 'jb_2', created_at: '2026-10-10T11:00:00Z' });
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

  it('never invents counts from a progress that arrived early', () => {
    // the engine sends none until the end; if one ever arrives mid-job it is not turned into a running figure
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

  it('announces the headline and the counts together', () => {
    expect(announcement(viewOf(done({ coverage: 'partial' }), T0))).toMatch(/^Finished, partial coverage\. 80 files/);
    expect(announcement(viewOf(job('running'), T0))).toBe('Scanning');
  });
});

describe('refusals in plain language', () => {
  const words = (code: ErrorCode, reason: string | null, message = 'service text') => refusalText(code, reason ? { reason } : null, message);

  it('explains that real-disk scans wait for the safety gates in a personal session', () => {
    const t = words('capability_unavailable', 'personal_scan_gated_184');
    expect(t).toContain('Real-disk scans arrive after the safety gates');
    expect(t).not.toContain('service text');
  });

  it('explains a synthetic session\'s lab-root rule', () => {
    expect(words('permission_denied', 'synthetic_session_requires_lab_root')).toContain('registered lab roots only');
  });

  it('words busy, a revoked grant and an unknown root', () => {
    expect(words('busy', null)).toContain('already running');
    expect(words('permission_denied', null)).toContain('grant is not active');
    expect(words('not_found', null)).toContain('no longer knows this root');
  });

  it('falls back to the service\'s own message, never to silence', () => {
    expect(words('internal_error', null, 'disk went away')).toBe('disk went away');
    expect(words('internal_error', null, '')).toContain('without giving a reason');
  });
});
