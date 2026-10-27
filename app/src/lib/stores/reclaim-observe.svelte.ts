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
  /** The job this page started, if any. The job names no volume, so any other job (adopted after busy or a reload) may cover another one. */
  ownJobId = $state<string | null>(null);
  private epoch: string | null = null;
  /** Bumped when the engine restarts: an answer requested before that belongs to the previous lifetime and is dropped (as the Duplicates store does). */
  private generation = 0;
  private sessionStartedAt: string | null = null;

  /** The HTTP hello can show a restart before the stream's hello arrives. */
  private alignEpoch(): void {
    const startedAt = session.info?.session_started_at;
    if (startedAt && this.sessionStartedAt !== startedAt) {
      if (this.sessionStartedAt !== null) { this.job = null; this.generation++; this.epoch = null; }
      this.sessionStartedAt = startedAt;
    }
  }
  /** Captured at dispatch: false once the engine lifetime or the client it asked has gone. */
  private capture(): () => boolean {
    this.alignEpoch();
    const generation = this.generation, client = session.client;
    return () => { this.alignEpoch(); return generation === this.generation && client === session.client; };
  }

  get active(): boolean { return this.job !== null && isActive(this.job.state); }

  apply(job: Job): void {
    if (job.kind !== 'reclaim') return;
    const old = this.job;
    if (old && (job.seq < old.seq || (job.job_id === old.job_id && job.rev < old.rev))) return;
    this.job = job;
  }
  onEvent(event: EventEnvelope): void {
    this.alignEpoch();
    if (this.epoch !== null && this.epoch !== event.epoch) { this.job = null; this.generation++; } // engine restarted: the old job is not known to still run
    this.epoch = event.epoch;
    if (event.event === 'job.state') this.apply((event.data as unknown as JobResult).job);
  }
  /** Picks up a job still running from earlier (another page, a reload). */
  async sync(): Promise<void> {
    const c = session.client;
    if (!c) return;
    const current = this.capture();
    try {
      const { result } = await c.call('jobs.list', { kinds: ['reclaim'], limit: 5 });
      if (!current()) return;
      for (const job of result.jobs) if (isActive(job.state)) this.apply(job); // an old, finished run is not news
      this.problem = ''; // the call worked: an earlier fault is no longer current
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
  async start(volumeId: string): Promise<void> {
    const c = session.client;
    if (!c || this.pending || this.active) return;
    this.pending = true; this.problem = '';
    const current = this.capture();
    try {
      const { result } = await c.call('reclaim.observe', { volume_id: volumeId });
      if (current()) { this.apply(result.job); this.ownJobId = result.job.job_id; }
    } catch (e) {
      session.handle(e);
      if (!current()) return;
      this.problem = observeRefusal(e);
      // Busy names the running job: follow it instead of leaving the owner blind.
      if (e instanceof LoomwardError && e.code === 'busy' && typeof e.detail?.job_id === 'string') {
        try {
          const { result } = await c.call('jobs.get', { job_id: e.detail.job_id });
          if (current()) { this.apply(result.job); this.problem = ''; } // the running job's status now says what the refusal meant
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
