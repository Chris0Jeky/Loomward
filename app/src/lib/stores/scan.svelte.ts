import type { EventEnvelope, Job, JobResult, ScanProgressEvent } from '../contracts.gen';
import { applyJob, applyProgress, announcement, isActive, lostJob, refusalText, viewOf, type Book, type ScanView } from '../scan/model';
import { LoomwardError } from '../transport/client';
import { session } from './session.svelte';

/** What the Grants & health view announces: the root, and the sentence for it (no name; the view adds it). */
export interface Change { rootId: string; text: string; n: number }

/**
 * The latest scan job per granted root, kept current by `job.state` and `scan.progress` events and
 * rebuilt from `jobs.list` whenever a view syncs (mount, reconnect, resync). The service decides every
 * outcome; this store only holds and words it. The tree itself refreshes through `session.epoch`, which
 * `tree.invalidated` and `roots.changed` already bump.
 */
class Scans {
  book = $state<Book>({});
  /** Plain-language reason the last start or cancel for a root was refused, until the next try. */
  problems = $state<Record<string, string>>({});
  /** Roots with a request in flight: a double click sends one. */
  pending = $state<Record<string, boolean>>({});
  /** Bumped whenever a scan reaches a terminal state, so views re-read the roots (totals, coverage). */
  settled = $state(0);
  change = $state<Change | null>(null);
  private n = 0;

  view(rootId: string, nowMs: number): ScanView | null {
    const job = this.book[rootId];
    return job ? viewOf(job, nowMs) : null;
  }

  get anyActive(): boolean {
    return Object.values(this.book).some((j) => isActive(j.state));
  }

  onEvent(ev: EventEnvelope): void {
    if (ev.event === 'job.state') this.apply((ev.data as unknown as JobResult).job);
    else if (ev.event === 'scan.progress') {
      const d = ev.data as unknown as ScanProgressEvent;
      this.commit(applyProgress(this.book, d.root_id, d.job_id, d.progress));
    }
  }

  apply(job: Job): void {
    this.commit(applyJob(this.book, job));
  }

  /**
   * Brings the book up to date with the service's own list (after a mount, reconnect or lagged stream). The list is a
   * bounded page, so a root it leaves out keeps its last known finished scan; a scan that was running and is left out
   * is asked for by id, and when the service no longer knows it, it is shown as lost rather than as still scanning.
   */
  async sync(): Promise<void> {
    const c = session.client;
    if (!c) return;
    const { result } = await c.call('jobs.list', { kinds: ['scan'], limit: 50 });
    this.commit(result.jobs.reduce(applyJob, this.book));
    const listed = new Set(result.jobs.map((j) => j.job_id));
    for (const job of Object.values(this.book)) {
      if (!isActive(job.state) || listed.has(job.job_id)) continue;
      try {
        this.apply((await c.call('jobs.get', { job_id: job.job_id })).result.job);
      } catch (e) {
        if (!(e instanceof LoomwardError)) throw e;
        if (e.code === 'not_found') this.apply(lostJob(job, new Date().toISOString()));
      }
    }
  }

  async start(rootId: string, mode: 'full' | 'refresh'): Promise<void> {
    const c = session.client;
    if (!c || this.pending[rootId]) return;
    this.pending = { ...this.pending, [rootId]: true };
    this.setProblem(rootId, null);
    try {
      this.apply((await c.call('scan.start', { root_id: rootId, mode })).result.job);
    } catch (e) {
      if (!(e instanceof LoomwardError)) throw e; // a transport fault belongs to the shell
      const busyJob = e.code === 'busy' && typeof e.detail?.job_id === 'string' ? e.detail.job_id : null;
      if (busyJob) await this.adopt(busyJob);
      this.setProblem(rootId, refusalText(e.code, e.detail, e.message));
    } finally {
      this.pending = { ...this.pending, [rootId]: false };
    }
  }

  async cancel(rootId: string): Promise<void> {
    const c = session.client;
    const job = this.book[rootId];
    if (!c || !job || this.pending[rootId]) return;
    this.pending = { ...this.pending, [rootId]: true };
    this.setProblem(rootId, null);
    try {
      this.apply((await c.call('scan.cancel', { job_id: job.job_id })).result.job);
    } catch (e) {
      if (!(e instanceof LoomwardError)) throw e;
      this.setProblem(rootId, `The scan could not be cancelled: ${e.message}`);
    } finally {
      this.pending = { ...this.pending, [rootId]: false };
    }
  }

  /** A start answered `busy` with the running job's id: show that job instead of pretending none exists. */
  private async adopt(jobId: string): Promise<void> {
    try {
      this.apply((await session.client!.call('jobs.get', { job_id: jobId })).result.job);
    } catch (e) {
      if (!(e instanceof LoomwardError)) throw e;
    }
  }

  private setProblem(rootId: string, text: string | null): void {
    const next = { ...this.problems };
    if (text === null) delete next[rootId];
    else next[rootId] = text;
    this.problems = next;
    if (text !== null) this.change = { rootId, text, n: ++this.n };
  }

  /**
   * Installs the next book. A root whose scan changed state is announced, except a finished scan met for the
   * first time (history on page load); one that went from running to finished settles, so views re-read the roots.
   */
  private commit(next: Book): void {
    if (next === this.book) return;
    const before = this.book;
    this.book = next;
    for (const [rootId, job] of Object.entries(next)) {
      const was = before[rootId] ? viewOf(before[rootId], 0) : null;
      const now = viewOf(job, 0);
      if (was?.activity === now.activity && before[rootId]?.job_id === job.job_id) continue;
      if (was && !now.active && (was.active || before[rootId]?.job_id !== job.job_id)) this.settled++;
      if (was || now.active) this.change = { rootId, text: announcement(now), n: ++this.n };
    }
  }
}

export const scans = new Scans();
session.listen((ev) => scans.onEvent(ev));
