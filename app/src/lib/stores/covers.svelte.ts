import type { CoverageReport, EventEnvelope, Job, JobResult } from '../contracts.gen';
import { isActive } from '../scan/model';
import { LoomwardError } from '../transport/client';
import { coversRefusal } from '../../views/duplicates/covers-model';
import { session } from './session.svelte';

export interface CoversBudget {
  /** An exact decimal byte count; absent means the service default. */
  bytes?: string;
  secs?: number;
}

/** The one `coverage` job (#1106): the engine runs one at a time, so the store holds a single job and the report it produced. */
export class Covers {
  job = $state<Job | null>(null);
  report = $state<CoverageReport | null>(null);
  pending = $state(false);
  problem = $state('');
  private epoch: string | null = null;
  /** Bumped when the engine restarts: an answer requested before that belongs to the previous lifetime and is dropped (as the Duplicates store does). */
  private generation = 0;
  private sessionStartedAt: string | null = null;
  /** The job whose report was asked for, so a finished job is fetched once. */
  private fetched: string | null = null;

  private alignEpoch(): void {
    const startedAt = session.info?.session_started_at;
    if (startedAt && this.sessionStartedAt !== startedAt) {
      if (this.sessionStartedAt !== null) this.forget();
      this.sessionStartedAt = startedAt;
    }
  }
  private forget(): void { this.job = null; this.report = null; this.fetched = null; this.generation++; this.epoch = null; }
  private capture(): () => boolean {
    this.alignEpoch();
    const generation = this.generation, client = session.client;
    return () => { this.alignEpoch(); return generation === this.generation && client === session.client; };
  }

  get active(): boolean { return this.job !== null && isActive(this.job.state); }

  apply(job: Job): void {
    if (job.kind !== 'coverage') return;
    const old = this.job;
    if (old && (job.seq < old.seq || (job.job_id === old.job_id && job.rev < old.rev))) return;
    if (old && old.job_id !== job.job_id) { this.report = null; this.fetched = null; } // a newer run replaces the old report
    this.job = job;
    // Only a run that finished has a report: a cancelled or failed one has none, and says so by its state.
    if (job.state === 'completed' && this.fetched !== job.job_id) void this.load(job);
  }
  onEvent(event: EventEnvelope): void {
    this.alignEpoch();
    if (this.epoch !== null && this.epoch !== event.epoch) this.forget(); // engine restarted: the old job is not known to still run
    this.epoch = event.epoch;
    if (event.event === 'job.state') this.apply((event.data as unknown as JobResult).job);
  }
  /** Picks up the newest run (still going, or finished inside the service's ten minutes) after a reload or another page. */
  async sync(): Promise<void> {
    const c = session.client;
    if (!c) return;
    const current = this.capture();
    try {
      const { result } = await c.call('jobs.list', { kinds: ['coverage'], limit: 5 });
      if (!current()) return;
      const newest = result.jobs[0];
      if (newest && !this.job) this.apply(newest);
      this.problem = '';
    } catch (e) { const message = session.handle(e); if (current()) this.problem = message; }
  }
  async refresh(): Promise<void> {
    const c = session.client, job = this.job;
    if (!c || !job) return;
    const current = this.capture();
    try {
      const { result } = await c.call('jobs.get', { job_id: job.job_id });
      if (current()) { this.apply(result.job); this.problem = ''; }
    } catch (e) { const message = session.handle(e); if (current()) this.problem = message; }
  }
  private async load(job: Job): Promise<void> {
    const c = session.client;
    if (!c) return;
    this.fetched = job.job_id;
    const current = this.capture();
    try {
      const { result } = await c.call('coverage.result', { job_id: job.job_id });
      if (current() && this.job?.job_id === job.job_id) this.report = result;
    } catch (e) {
      // not_found: the report expired or its folder's grant ended. The job's own state stands, with no answer.
      const message = session.handle(e);
      if (current() && !(e instanceof LoomwardError && e.code === 'not_found')) { this.problem = message; this.fetched = null; }
    }
  }
  async start(a: string, b: string, budget: CoversBudget = {}): Promise<void> {
    const c = session.client;
    if (!c || this.pending || this.active) return;
    this.pending = true; this.problem = '';
    const current = this.capture();
    try {
      const { result } = await c.call('coverage.start', {
        a_root_id: a, b_root_id: b,
        ...(budget.bytes ? { budget_bytes: budget.bytes } : {}),
        ...(budget.secs ? { budget_secs: budget.secs } : {}),
      });
      if (current()) { this.report = null; this.fetched = null; this.job = null; this.apply(result.job); }
    } catch (e) {
      session.handle(e);
      if (!current()) return;
      this.problem = coversRefusal(e);
      // Busy names the running check: follow it instead of leaving the owner blind.
      if (e instanceof LoomwardError && e.code === 'busy' && typeof e.detail?.job_id === 'string') {
        try {
          const { result } = await c.call('jobs.get', { job_id: e.detail.job_id });
          if (current()) { this.apply(result.job); this.problem = ''; }
        } catch { /* the refusal text stands */ }
      }
    } finally { this.pending = false; }
  }
  async cancel(): Promise<void> {
    const c = session.client, job = this.job;
    if (!c || !job || this.pending || !isActive(job.state)) return;
    this.pending = true; this.problem = '';
    const current = this.capture();
    try {
      const { result } = await c.call('jobs.cancel', { job_id: job.job_id });
      if (current()) this.apply(result.job);
    } catch (e) { const message = session.handle(e); if (current()) this.problem = message; }
    finally { this.pending = false; }
  }
}
