import type { LearningDeferredRequest, LearningQueue, LearningQueueMode } from '../../lib/contracts.gen';
import { LoomwardError, type Client } from '../../lib/transport/client';

export class ReviewQueue {
  page = $state<LearningQueue | null>(null);
  mode = $state<LearningQueueMode>('review');
  loading = $state(false);
  saving = $state(false);
  supported = $state(true);
  problem = $state('');
  notice = $state('');
  pending = $state<{ payload: LearningDeferredRequest; expectedStateRev: string } | null>(null);
  private revision: string | null = null;
  private generation = 0;

  clear(): void {
    this.generation++;
    this.page = null;
    this.revision = null;
    this.pending = null;
    this.loading = this.saving = false;
  }

  async load(client: Client | null, handle: (e: unknown) => string, cursor: string | null = null, mode = this.mode): Promise<boolean> {
    if (this.saving) return false;
    const mine = ++this.generation;
    this.page = null;
    this.revision = null;
    this.problem = '';
    this.mode = mode;
    if (!client) return false;
    this.loading = true;
    let restarted = false;
    const read = () => client.call('learning.queue', { root_id: null, limit: 20, cursor, ...(this.mode === 'deferred' ? { mode: 'deferred' as const } : {}) });
    try {
      let response;
      try { response = await read(); }
      catch (e) {
        if (mine !== this.generation) return false;
        if (e instanceof LoomwardError && e.code === 'stale_generation') {
          cursor = null;
          restarted = true;
          this.notice = 'Review changed or expired. Showing the first page.';
        } else if (mode === 'deferred' && e instanceof LoomwardError && ['invalid_request', 'unknown_command', 'capability_unavailable'].includes(e.code)) {
          this.supported = false;
          this.mode = 'review';
          cursor = null;
          this.notice = 'Deferred review is unavailable in this service. Ordinary review remains available.';
        } else throw e;
        response = await read();
      }
      if (mine !== this.generation) return false;
      this.page = response.result;
      this.revision = response.meta.state_rev;
    } catch (e) {
      if (mine === this.generation) this.problem = handle(e);
    } finally {
      if (mine === this.generation) this.loading = false;
    }
    return restarted;
  }

  async choose(client: Client | null, handle: (e: unknown) => string, node?: string, deferred?: boolean): Promise<void> {
    if (!client || this.saving || this.loading || !this.supported) return;
    if (!this.pending) {
      if (!node || deferred === undefined || this.revision === null) return;
      this.pending = { payload: { node_id: node, deferred, client_event_id: `ce_${crypto.randomUUID().replaceAll('-', '')}` }, expectedStateRev: this.revision };
    }
    const mine = ++this.generation;
    this.saving = true;
    this.problem = this.notice = '';
    this.page = null;
    try {
      const response = await client.call('learning.set_deferred', this.pending.payload, { expectedStateRev: this.pending.expectedStateRev });
      if (mine !== this.generation) return;
      this.pending = null;
      this.notice = response.result.deferred ? 'Deferred until you choose Bring back. No label recorded.' : 'Returned to review. No label recorded.';
    } catch (e) {
      if (mine !== this.generation) return;
      if (e instanceof LoomwardError && ['invalid_request', 'stale_generation', 'not_found', 'permission_denied', 'capability_unavailable', 'unknown_command'].includes(e.code)) this.pending = null;
      if (e instanceof LoomwardError && ['unknown_command', 'capability_unavailable'].includes(e.code)) this.supported = false;
      this.problem = handle(e);
    } finally {
      if (mine === this.generation) this.saving = false;
    }
    if (mine !== this.generation || this.pending) return;
    const problem = this.problem;
    await this.load(client, handle);
    if (problem && !this.problem) this.problem = problem;
  }
}
