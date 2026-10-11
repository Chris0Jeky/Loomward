import type { EventEnvelope, Job, JobResult } from '../contracts.gen';
import { isActive } from '../scan/model';
import { LoomwardError } from '../transport/client';
import { session } from './session.svelte';

/** Per-view state, ordered by the engine's submission/revision counters. */
export class Duplicates {
  book = $state<Record<string, Job>>({});
  pending = $state(false);
  problem = $state('');
  gated = $state(false);
  private epoch: string | null = null;

  apply(job: Job): void {
    if (job.kind !== 'duplicates' || !job.root_id) return;
    const old = this.book[job.root_id];
    if (old && (job.seq < old.seq || (job.job_id === old.job_id && job.rev <= old.rev))) return;
    this.book = { ...this.book, [job.root_id]: job };
  }
  onEvent(event: EventEnvelope): void {
    if (this.epoch !== null && this.epoch !== event.epoch) this.book = {};
    this.epoch = event.epoch;
    if (event.event === 'job.state') this.apply((event.data as unknown as JobResult).job);
  }
  async sync(): Promise<void> {
    const c = session.client;
    if (!c) return;
    const { result } = await c.call('jobs.list', { kinds: ['duplicates'], limit: 50 });
    for (const job of result.jobs) this.apply(job);
    for (const job of Object.values(this.book)) if (isActive(job.state)) await this.refresh(job);
  }
  async refresh(job: Job): Promise<void> {
    if (!session.client) return;
    try { this.apply((await session.client.call('jobs.get', { job_id: job.job_id })).result.job); }
    catch (e) { this.problem = session.handle(e); }
  }
  async start(rootId: string): Promise<void> {
    if (!session.client || this.pending) return;
    this.pending = true; this.problem = ''; this.gated = false;
    try {
      this.apply((await session.client.call('duplicates.inspect', { root_id: rootId, budget: { max_candidates: 200, max_bytes: '67108864', max_ms: 10000 } })).result.job);
    } catch (e) {
      if (e instanceof LoomwardError && e.detail?.reason === 'content_read_gated') this.gated = true;
      else {
        this.problem = session.handle(e);
        if (e instanceof LoomwardError && e.code === 'busy' && typeof e.detail?.job_id === 'string') {
          this.apply((await session.client.call('jobs.get', { job_id: e.detail.job_id })).result.job);
        }
      }
    } finally { this.pending = false; }
  }
  async cancel(rootId: string): Promise<void> {
    const job = this.book[rootId];
    if (!session.client || !job || this.pending || !isActive(job.state)) return;
    this.pending = true; this.problem = '';
    try { this.apply((await session.client.call('duplicates.cancel', { job_id: job.job_id })).result.job); }
    catch (e) { this.problem = session.handle(e); }
    finally { this.pending = false; }
  }
}
