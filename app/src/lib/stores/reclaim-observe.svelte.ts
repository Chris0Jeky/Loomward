import type { EventEnvelope, Job, JobResult } from '../contracts.gen';
import { isActive } from '../scan/model';
import { LoomwardError } from '../transport/client';
import { observeRefusal } from '../../views/reclaim/model';
import { session } from './session.svelte';

/** The one `reclaim` job (R1g): the engine runs one at a time and the job names no root or volume, so the store holds a single job. */
export class ReclaimObserve {
  job = $state<Job | null>(null);
  pending = $state(false);
  problem = $state('');
  private epoch: string | null = null;

  get active(): boolean { return this.job !== null && isActive(this.job.state); }

  apply(job: Job): void {
    if (job.kind !== 'reclaim') return;
    const old = this.job;
    if (old && (job.seq < old.seq || (job.job_id === old.job_id && job.rev < old.rev))) return;
    this.job = job;
  }
  onEvent(event: EventEnvelope): void {
    if (this.epoch !== null && this.epoch !== event.epoch) this.job = null; // engine restarted: the old job is not known to still run
    this.epoch = event.epoch;
    if (event.event === 'job.state') this.apply((event.data as unknown as JobResult).job);
  }
  /** Picks up a job still running from earlier (another page, a reload). */
  async sync(): Promise<void> {
    const c = session.client;
    if (!c) return;
    try {
      const { result } = await c.call('jobs.list', { kinds: ['reclaim'], limit: 5 });
      if (c !== session.client) return;
      for (const job of result.jobs) if (isActive(job.state)) this.apply(job); // an old, finished run is not news
    } catch (e) { this.problem = session.handle(e); }
  }
  async refresh(): Promise<void> {
    const c = session.client, job = this.job;
    if (!c || !job) return;
    try {
      const { result } = await c.call('jobs.get', { job_id: job.job_id });
      if (c === session.client) this.apply(result.job);
    } catch (e) { this.problem = session.handle(e); }
  }
  async start(volumeId: string): Promise<void> {
    const c = session.client;
    if (!c || this.pending || this.active) return;
    this.pending = true; this.problem = '';
    try {
      const { result } = await c.call('reclaim.observe', { volume_id: volumeId });
      if (c === session.client) this.apply(result.job);
    } catch (e) {
      session.handle(e);
      this.problem = observeRefusal(e);
      // Busy names the running job: follow it instead of leaving the owner blind.
      if (e instanceof LoomwardError && e.code === 'busy' && typeof e.detail?.job_id === 'string') {
        try { const { result } = await c.call('jobs.get', { job_id: e.detail.job_id }); this.apply(result.job); } catch { /* the refusal text stands */ }
      }
    } finally { this.pending = false; }
  }
  async cancel(): Promise<void> {
    const c = session.client, job = this.job;
    if (!c || !job || this.pending || !isActive(job.state)) return;
    this.pending = true; this.problem = '';
    try {
      const { result } = await c.call('jobs.cancel', { job_id: job.job_id });
      if (c === session.client) this.apply(result.job);
    } catch (e) { this.problem = session.handle(e); }
    finally { this.pending = false; }
  }
}
