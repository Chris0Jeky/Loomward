import type { MeaningThread } from '../contracts.gen';
import type { Client } from '../transport/client';

/** Meaning a person set (a label, a mixture of labels): a student suggestion never replaces it.
 *  A system aggregate or a teacher label is not human, so the projection still overlays it. */
export const isHumanMeaning = (m: Pick<MeaningThread, 'state' | 'source'> | undefined) =>
  (m?.state === 'labelled' || m?.state === 'mixed') && m.source === 'human';

const UNKNOWN: MeaningThread = { state: 'unknown', label: null, share: null, source: null, collection_ids: [] };

/** The slice with the student projection laid over each node, except where the slice already carries a human meaning. */
export function overlayMeaning<S extends { nodes: { node_id: string; threads: { meaning: MeaningThread } }[] }>(slice: S, items: Record<string, MeaningThread>): S {
  return { ...slice, nodes: slice.nodes.map((n) => isHumanMeaning(n.threads.meaning) ? n : { ...n, threads: { ...n.threads, meaning: items[n.node_id] ?? UNKNOWN } }) };
}

/** One visible projection. Aborting also invalidates transports that finish after cancellation. */
export class Meaning {
  items = $state<Record<string, MeaningThread>>({});
  message = $state('');
  busy = $state(false);
  /** A load attempt has finished (reply, failure or budget): a missing node is then unknown, not unasked. */
  loaded = $state(false);
  private abort: AbortController | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;

  /** A consumer needs no request of its own while one is in flight or after it returned this node. */
  covers(nodeId: string) { return this.busy || (this.loaded && nodeId in this.items); }

  clear() {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    this.abort?.abort();
    this.abort = null;
    this.loaded = false;
    this.items = {};
    this.message = '';
    this.busy = false;
  }

  /** Coalesces a burst of changes (a window drag) into one request for the last list. */
  loadSoon(client: Client, ids: string[], ms = 200) {
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(() => { this.timer = null; void this.load(client, ids); }, ms);
  }

  /** The previous items stay until the replacement arrives: no flash of pending, no relayout. */
  async load(client: Client, ids: string[]) {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    if (!ids.length) return this.clear();
    this.abort?.abort();
    const ctl = this.abort = new AbortController();
    this.message = '';
    this.loaded = false;
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
      if (!ctl.signal.aborted) {
        this.items = {};
        this.message = e instanceof Error ? e.message : 'Meaning unavailable.';
      }
    } finally {
      if (!ctl.signal.aborted) { this.busy = false; this.loaded = true; }
    }
  }
}
