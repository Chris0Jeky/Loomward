import { insightFailure } from '../format/insight';
import { LoomwardError, type Client } from '../transport/client';
import type { InsightFolder } from '../contracts.gen';

/**
 * One on-demand `insight.folder` request. A newer request, or `clear()`, aborts the older one and its
 * reply is dropped, so a slow answer can never describe a different folder than the one on screen.
 */
export class FolderInsight {
  result = $state<InsightFolder | null>(null);
  message = $state('');
  busy = $state(false);
  private abort: AbortController | null = null;

  clear() {
    this.abort?.abort();
    this.abort = null;
    this.result = null;
    this.message = '';
    this.busy = false;
  }

  /** `handle` turns an unexpected error into display text (and lets the session notice a lost connection). */
  async describe(client: Client, rootId: string, nodeId: string, handle: (e: unknown) => string = (e) => (e instanceof Error ? e.message : '')) {
    this.abort?.abort();
    const ctl = this.abort = new AbortController();
    this.result = null;
    this.message = '';
    this.busy = true;
    try {
      const { result } = await client.call('insight.folder', { root_id: rootId, node_id: nodeId }, { signal: ctl.signal });
      if (!ctl.signal.aborted) {
        // a reply about another folder is never shown under this one
        if (result.node_id === nodeId && result.root_id === rootId) this.result = result;
        else this.message = 'The reply was for a different folder and was dropped. Nothing was described.';
      }
    } catch (e) {
      if (!ctl.signal.aborted) {
        const text = handle(e);
        this.message = e instanceof LoomwardError ? insightFailure(e.code, text) : insightFailure(null, text);
      }
    } finally {
      if (!ctl.signal.aborted) this.busy = false;
    }
  }
}
