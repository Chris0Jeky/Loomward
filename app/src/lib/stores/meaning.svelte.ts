import type { MeaningThread } from '../contracts.gen';
import type { Client } from '../transport/client';

/** One visible projection. Aborting also invalidates transports that finish after cancellation. */
export class Meaning {
  items = $state<Record<string, MeaningThread>>({});
  message = $state('');
  busy = $state(false);
  private abort: AbortController | null = null;

  clear() {
    this.abort?.abort();
    this.abort = null;
    this.items = {};
    this.message = '';
    this.busy = false;
  }

  async load(client: Client, ids: string[]) {
    this.clear();
    if (!ids.length) return;
    const ctl = this.abort = new AbortController();
    this.busy = true;
    const visible = [...new Set(ids)];
    try {
      // The service work cap also keeps current sealed IDs inside HTTP's 64 KiB ingress limit.
      const { result } = await client.call('threads.meaning', { node_ids: visible.slice(0, 1000) }, { signal: ctl.signal });
      if (ctl.signal.aborted) return;
      this.items = Object.fromEntries(result.items.map(i => [i.node_id, i.meaning]));
      this.message = result.budget_hit || visible.length > 1000 ? 'Meaning work budget reached; unreturned nodes remain unknown.'
        : result.model_id === null ? 'No fitted model yet. The cloth has no meaning weft.' : '';
    } catch (e) {
      if (!ctl.signal.aborted) this.message = e instanceof Error ? e.message : 'Meaning unavailable.';
    } finally {
      if (!ctl.signal.aborted) this.busy = false;
    }
  }
}
