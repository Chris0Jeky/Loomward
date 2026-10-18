import type { EventEnvelope, Job, JobResult } from '../contracts.gen';
import { isActive } from '../scan/model';
import { LoomwardError } from '../transport/client';
import { session } from './session.svelte';

/** Per-view state, ordered by the engine's submission/revision counters. */
export class Duplicates {
  book = $state<Record<string, Job>>({});
  pending = $state(false);
  problem = $state('');
  private epoch: string | null = null;
  /** Bumped when the engine restarts: an answer requested before that belongs to the previous lifetime and is dropped (as Explorer's generation does). */
  private generation = 0;

  apply(job: Job): void {
    if (job.kind !== 'duplicates' || !job.root_id) return;
    const old = this.book[job.root_id];
    // An equal revision can restore evidence withheld while resynchronising.
    if (old && (job.seq < old.seq || (job.job_id === old.job_id && job.rev < old.rev))) return;
    this.book = { ...this.book, [job.root_id]: job };
  }
  onEvent(event: EventEnvelope): void {
    if (this.epoch !== null && this.epoch !== event.epoch) { this.book = {}; this.generation++; }
    this.epoch = event.epoch;
    if (event.event === 'job.state') this.apply((event.data as unknown as JobResult).job);
  }
  async sync(): Promise<void> {
    const c = session.client;
    if (!c) return;
    const generation = this.generation;
    this.book = Object.fromEntries(Object.entries(this.book).map(([root, job]) => [root, { ...job, duplicates: undefined }]));
    const { result } = await c.call('jobs.list', { kinds: ['duplicates'], limit: 50 });
    if (generation !== this.generation) return;
    for (const job of result.jobs) this.apply(job);
    for (const job of Object.values(this.book)) {
      if (generation !== this.generation) return;
      if (isActive(job.state) || !result.jobs.some(listed => listed.job_id === job.job_id)) await this.refresh(job);
    }
  }
  async refresh(job: Job): Promise<void> {
    if (!session.client) return;
    const generation = this.generation;
    try {
      const { result } = await session.client.call('jobs.get', { job_id: job.job_id });
      if (generation === this.generation) this.apply(result.job);
    } catch (e) {
      const message = session.handle(e); // a transport fault is still a fault; the problem text only for this lifetime
      if (generation === this.generation) this.problem = message;
    }
  }
  async start(rootId: string): Promise<void> {
    if (!session.client || this.pending) return;
    this.pending = true; this.problem = '';
    const generation = this.generation; // answers from a previous engine lifetime are dropped, as in sync()
    const current = () => generation === this.generation;
    try {
      const { result } = await session.client.call('duplicates.inspect', { root_id: rootId, budget: { max_candidates: 200, max_bytes: '67108864', max_ms: 10000 } });
      if (current()) this.apply(result.job);
    } catch (e) {
      const message = session.handle(e);
      if (current()) this.problem = message;
      if (current() && e instanceof LoomwardError && e.code === 'busy' && typeof e.detail?.job_id === 'string') {
        try {
          const { result } = await session.client.call('jobs.get', { job_id: e.detail.job_id });
          if (current()) this.apply(result.job);
        } catch (follow) {
          const followMessage = session.handle(follow);
          if (current()) this.problem = followMessage;
        }
      }
    } finally { this.pending = false; }
  }
  async cancel(rootId: string): Promise<void> {
    const job = this.book[rootId];
    if (!session.client || !job || this.pending || !isActive(job.state)) return;
    this.pending = true; this.problem = '';
    const generation = this.generation;
    try {
      const { result } = await session.client.call('duplicates.cancel', { job_id: job.job_id });
      if (generation === this.generation) this.apply(result.job);
    } catch (e) {
      const message = session.handle(e);
      if (generation === this.generation) this.problem = message;
    } finally { this.pending = false; }
  }
}
