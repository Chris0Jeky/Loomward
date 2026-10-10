// The scan-state model: what the app knows about each root's latest scan job, built only from what the
// service sends (`job.state` events, `scan.progress` events, `jobs.list`). Pure functions, no DOM.
//
// Honesty rules (AGENTS.md invariant 4): `scan.progress` currently arrives only at the end of a job, so a
// running scan has no counts and no percentage, and none is invented. A scan is "complete" only when the
// service says its coverage is complete; anything else says partial.
import type { ErrorCode, Job, JobState, ScanProgress } from '../contracts.gen';
import { formatBytes } from '../format/bytes';
import { formatCount } from '../format/time';

/** The latest scan job per root id. */
export type Book = Readonly<Record<string, Job>>;

export type Activity = 'queued' | 'running' | 'cancelling' | 'completed' | 'partial' | 'failed' | 'cancelled';

const RANK: Record<JobState, number> = { queued: 0, running: 1, cancel_requested: 2, completed: 3, failed: 3, cancelled: 3 };
const TERMINAL = 3;

export const isActive = (s: JobState): boolean => RANK[s] < TERMINAL;

/**
 * Applies a job snapshot (event, command result or list row). A state never moves backwards and a terminal
 * state is final, so a replayed or late event cannot undo a newer one (semantics.md section 7); an older job
 * never replaces a newer one for the same root.
 */
export function applyJob(book: Book, job: Job): Book {
  const root = job.root_id;
  if (job.kind !== 'scan' || !root) return book;
  const cur = book[root];
  if (cur) {
    if (cur.job_id === job.job_id) {
      if (RANK[job.state] < RANK[cur.state] || (RANK[cur.state] === TERMINAL && job.state !== cur.state)) return book;
      return { ...book, [root]: { ...job, progress: job.progress ?? cur.progress } };
    }
    if (Date.parse(job.created_at) < Date.parse(cur.created_at)) return book;
  }
  return { ...book, [root]: job };
}

/** A `scan.progress` event: attached to the root's current job when it is that job, else dropped (jobs.list will bring the job). */
export function applyProgress(book: Book, rootId: string, jobId: string, progress: ScanProgress): Book {
  const cur = book[rootId];
  return cur && cur.job_id === jobId ? { ...book, [rootId]: { ...cur, progress } } : book;
}

/** Rebuilds the book from a `jobs.list` answer (any order). */
export const fromList = (jobs: readonly Job[]): Book => jobs.reduce<Book>(applyJob, {});

export interface ScanView {
  activity: Activity;
  /** Queued, running or cancelling: the control is Cancel and the elapsed time is ticking. */
  active: boolean;
  headline: string;
  /** Counts, or what is not known. Empty while a scan runs: no count exists until it ends. */
  detail: string;
  /** Wall time so far (active) or the job's own figure (finished); null when unknown. */
  elapsedMs: number | null;
}

const ms = (iso: string | null): number | null => {
  const t = iso === null ? NaN : Date.parse(iso);
  return Number.isNaN(t) ? null : t;
};

function countsText(p: ScanProgress | null): string {
  if (!p) return 'Counts were not reported.';
  const unread = p.failed > 0 ? `, ${formatCount(p.failed)} could not be read` : '';
  return `${formatCount(p.indexed_files)} files, ${formatCount(p.indexed_dirs)} folders, ${formatBytes(p.logical_bytes)} logical; ${formatCount(p.skipped)} skipped${unread}.`;
}

export function viewOf(job: Job, nowMs: number): ScanView {
  const started = ms(job.started_at) ?? ms(job.created_at);
  const live = started === null ? null : Math.max(0, nowMs - started);
  const finished = ms(job.finished_at);
  const final = job.progress?.elapsed_ms ?? (started !== null && finished !== null ? Math.max(0, finished - started) : null);
  switch (job.state) {
    case 'queued':
      return { activity: 'queued', active: true, headline: 'Queued', detail: '', elapsedMs: live };
    case 'running':
      return { activity: 'running', active: true, headline: 'Scanning', detail: '', elapsedMs: live };
    case 'cancel_requested':
      return { activity: 'cancelling', active: true, headline: 'Cancelling', detail: '', elapsedMs: live };
    case 'cancelled':
      return { activity: 'cancelled', active: false, headline: 'Cancelled', detail: 'The scan stopped early, so this root is not fully covered.', elapsedMs: final };
    case 'failed':
      return { activity: 'failed', active: false, headline: 'Failed', detail: job.error?.message ?? 'The service gave no reason.', elapsedMs: final };
    case 'completed': {
      const p = job.progress;
      const stopped = p?.limit_hit === true;
      const gaps = (p?.failed ?? 0) > 0;
      if (job.coverage === 'complete' && !stopped && !gaps) {
        return { activity: 'completed', active: false, headline: 'Finished, complete coverage', detail: countsText(p), elapsedMs: final };
      }
      const why = stopped ? 'Stopped at its entry budget' : job.coverage === null ? 'Finished, coverage not reported' : 'Finished, partial coverage';
      return { activity: 'partial', active: false, headline: why, detail: `${countsText(p)} Totals are lower bounds, not the whole root.`, elapsedMs: final };
    }
  }
}

/** The sentence a screen reader hears when a root's scan changes state (without the root's name). */
export const announcement = (v: ScanView): string => `${v.headline}${v.detail ? `. ${v.detail}` : ''}`;

const reasonOf = (detail: Record<string, unknown> | null): string | null =>
  typeof detail?.reason === 'string' ? detail.reason : null;

/** Plain-language refusal for a failed `scan.start`. The service decides; this only words it. */
export function refusalText(code: ErrorCode, detail: Record<string, unknown> | null, message: string): string {
  switch (reasonOf(detail)) {
    case 'personal_scan_gated_184':
      return 'Real-disk scans arrive after the safety gates (issue 184). Until then this personal session does not scan, and nothing was read.';
    case 'synthetic_session_requires_lab_root':
      return 'This synthetic session scans registered lab roots only, and this root is not one. Nothing was read.';
  }
  switch (code) {
    case 'busy': return 'A scan of this root is already running.';
    case 'permission_denied': return "This root's grant is not active, so Loomward may not scan it.";
    case 'not_found': return 'The service no longer knows this root. Reload this page and try again.';
    default: return message || 'The service refused the scan without giving a reason.';
  }
}
