// The scan-state model: what the app knows about each root's latest scan job, built only from what the
// service sends (`job.state` events, `scan.progress` events, `jobs.list`). Pure functions, no DOM.
//
// Honesty rules (AGENTS.md invariant 4): interim `scan.progress` samples are observations, not totals or
// percentages. The running view stays indeterminate. A scan is "complete" only when the
// service says its coverage is complete; anything else says partial.
import type { ErrorCode, Job, JobState, ScanProgress } from '../contracts.gen';
import { formatBytes, parseBytes } from '../format/bytes';
import { formatCount } from '../format/time';

/** The latest scan job per root id. */
export type Book = Readonly<Record<string, Job>>;

export type Activity = 'queued' | 'running' | 'cancelling' | 'completed' | 'partial' | 'failed' | 'cancelled';

const RANK: Record<JobState, number> = { queued: 0, running: 1, cancel_requested: 2, completed: 3, failed: 3, cancelled: 3 };
const TERMINAL = 3;

export const isActive = (s: JobState): boolean => RANK[s] < TERMINAL;

/**
 * Revisions choose one authoritative snapshot; legacy snapshots without rev preserve known fields.
 */
function merge(cur: Job, job: Job): Job {
  if (cur.rev !== undefined && job.rev !== undefined) return job.rev > cur.rev ? job : cur;
  let progress = job.progress ?? cur.progress;
  if (cur.progress && job.progress) {
    const a = cur.progress, b = job.progress;
    const maxBytes = (x: string | null, y: string | null) => {
      const left = parseBytes(x), right = parseBytes(y);
      return left === null ? y : right === null || left > right ? x : y;
    };
    progress = {
      ...(b.examined >= a.examined ? b : a),
      examined: Math.max(a.examined, b.examined),
      indexed_files: Math.max(a.indexed_files, b.indexed_files),
      indexed_dirs: Math.max(a.indexed_dirs, b.indexed_dirs),
      skipped: Math.max(a.skipped, b.skipped),
      failed: Math.max(a.failed, b.failed),
      logical_bytes: maxBytes(a.logical_bytes, b.logical_bytes)!,
      allocated_bytes: maxBytes(a.allocated_bytes, b.allocated_bytes),
      elapsed_ms: Math.max(a.elapsed_ms, b.elapsed_ms),
      limit_hit: a.limit_hit || b.limit_hit,
      limit_reason: b.limit_reason ?? a.limit_reason,
    };
  }
  return {
    ...job,
    started_at: job.started_at ?? cur.started_at,
    finished_at: job.finished_at ?? cur.finished_at,
    coverage: job.coverage ?? cur.coverage,
    error: job.error ?? cur.error,
    progress,
  };
}

/** Submission order within one engine lifetime, independent of clocks and random IDs. */
function newerJob(job: Job, cur: Job): boolean {
  if (job.seq !== undefined && cur.seq !== undefined) return job.seq > cur.seq;
  const a = Date.parse(job.created_at), b = Date.parse(cur.created_at);
  return Number.isNaN(b) ? !Number.isNaN(a) : a > b; // an unreadable time never outranks a readable one
}

/** Applies snapshots by submission and revision; legacy jobs retain the state and null guards. */
export function applyJob(book: Book, job: Job): Book {
  const root = job.root_id;
  if (job.kind !== 'scan' || !root) return book;
  const cur = book[root];
  if (cur) {
    if (cur.job_id === job.job_id) {
      if ((cur.rev === undefined || job.rev === undefined) &&
        (RANK[job.state] < RANK[cur.state] || (RANK[cur.state] === TERMINAL && job.state !== cur.state))) return book;
      const next = merge(cur, job);
      return next === cur ? book : { ...book, [root]: next };
    }
    if (!newerJob(job, cur)) return book;
  }
  return { ...book, [root]: job };
}

/** Bare progress has no revision; versioned jobs get authoritative progress from job.state. */
export function applyProgress(book: Book, rootId: string, jobId: string, progress: ScanProgress): Book {
  const cur = book[rootId];
  return cur && cur.job_id === jobId && cur.rev === undefined ? { ...book, [rootId]: { ...cur, progress } } : book;
}

/** A running scan the service no longer knows (not in `jobs.list`, `jobs.get` says not found): its end is unknown, never "still scanning". */
export const lostJob = (job: Job, at: string): Job => ({
  ...job, state: 'failed', finished_at: at,
  error: { code: 'not_found', message: 'The service no longer lists this scan, so how it ended is unknown.', retryable: false, detail: null },
});

/** Rebuilds the book from a `jobs.list` answer (any order). */
export const fromList = (jobs: readonly Job[]): Book => jobs.reduce<Book>(applyJob, {});

export interface ScanView {
  activity: Activity;
  /** Queued, running or cancelling: the control is Cancel and the elapsed time is ticking. */
  active: boolean;
  headline: string;
  /** Final counts, or what is not known. Empty while a scan runs; interim samples are not totals. */
  detail: string;
  /** Wall time so far (active) or the job's own figure (finished); null when unknown. */
  elapsedMs: number | null;
}

const ms = (iso: string | null): number | null => {
  const t = iso === null ? NaN : Date.parse(iso);
  return Number.isNaN(t) ? null : t;
};

/** Headline for a scan that stopped at a cap. An absent reason is the entry budget, which names no cause. */
function stopReason(reason: ScanProgress['limit_reason']): string {
  if (reason === 'depth_limit') return 'Stopped at its directory depth limit';
  if (reason === 'directory_limit') return 'Stopped at its directory limit';
  return 'Stopped at its entry budget';
}

function countsText(p: ScanProgress | null): string {
  if (!p) return 'Counts were not reported.';
  const unread = p.failed > 0 ? `, ${formatCount(p.failed)} could not be read` : '';
  return `${formatCount(p.indexed_files)} files, ${formatCount(p.indexed_dirs)} folders, ${formatBytes(p.logical_bytes)} logical; ${formatCount(p.skipped)} skipped${unread}.`;
}

export function viewOf(job: Job, nowMs: number): ScanView {
  const started = ms(job.started_at);
  const liveStarted = started ?? ms(job.created_at);
  const live = liveStarted === null ? null : Math.max(0, nowMs - liveStarted);
  const finished = ms(job.finished_at);
  const final = started !== null && finished !== null ? Math.max(0, finished - started) : job.progress?.elapsed_ms ?? null;
  switch (job.state) {
    case 'queued':
      return { activity: 'queued', active: true, headline: 'Queued', detail: '', elapsedMs: live };
    case 'running':
      return { activity: 'running', active: true, headline: 'Scanning', detail: '', elapsedMs: live };
    case 'cancel_requested':
      return { activity: 'cancelling', active: true, headline: 'Cancelling', detail: '', elapsedMs: live };
    case 'cancelled':
      return { activity: 'cancelled', active: false, headline: 'Cancelled', detail: 'It stopped before it finished, so it was not published as a complete scan.', elapsedMs: final };
    case 'failed':
      return { activity: 'failed', active: false, headline: 'Failed', detail: job.error?.message ?? 'The service gave no reason.', elapsedMs: final };
    case 'completed': {
      const p = job.progress;
      const stopped = p?.limit_hit === true;
      const gaps = (p?.failed ?? 0) > 0;
      if (job.coverage === 'complete' && !stopped && !gaps) {
        return { activity: 'completed', active: false, headline: 'Finished, complete coverage', detail: countsText(p), elapsedMs: final };
      }
      const why = stopped ? stopReason(p?.limit_reason) : job.coverage === null ? 'Finished, coverage not reported' : 'Finished, partial coverage';
      return { activity: 'partial', active: false, headline: why, detail: `${countsText(p)} Totals are lower bounds, not the whole root.`, elapsedMs: final };
    }
  }
}

/** The sentence a screen reader hears when a root's scan changes state (without the root's name). */
export const announcement = (v: ScanView): string => `${v.headline}${v.detail ? `. ${v.detail}` : ''}`;

const reasonOf = (detail: Record<string, unknown> | null): string | null =>
  typeof detail?.reason === 'string' ? detail.reason : null;

/** The notice for a start that met a running scan; it is only true while that scan runs. */
export const ALREADY_RUNNING = 'A scan of this root is already running.';

/** Plain-language refusal for a failed `scan.start`. The service decides; this only words it. */
export function refusalText(code: ErrorCode, detail: Record<string, unknown> | null, message: string): string {
  switch (reasonOf(detail)) {
    case 'personal_session_requires_owner_grant':
      return 'This personal session scans only folders you granted, and this root is not one. Nothing was read.';
    case 'synthetic_session_requires_lab_root':
      return 'This synthetic session scans registered lab roots only, and this root is not one. Nothing was read.';
  }
  switch (code) {
    // `busy` is also the service's answer for a root that is starting or being revoked, a busy writer and too many
    // pending mutations: only a named running job (`job_id`) means this root is already being scanned.
    case 'busy':
      return typeof detail?.job_id === 'string' ? ALREADY_RUNNING : `The service is busy and did not start the scan${message ? `: ${message}` : ''}. Try again shortly.`;
    case 'permission_denied': return "This root's grant is not active, so Loomward may not scan it.";
    case 'not_found': return 'The service no longer knows this root. Reload this page and try again.';
    default: return message || 'The service refused the scan without giving a reason.';
  }
}
