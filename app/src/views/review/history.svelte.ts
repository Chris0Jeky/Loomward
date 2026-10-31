import type { FeedbackHistory } from '../../lib/contracts.gen';
import { LoomwardError, type Client } from '../../lib/transport/client';

export class HistoryPanel {
  page = $state<FeedbackHistory | null>(null);
  problem = $state('');
  notice = $state('');
  loading = $state(false);
  older = $state(false);
  private generation = 0;

  clear(): void {
    this.generation++;
    this.page = null;
    this.problem = '';
    this.notice = '';
    this.loading = false;
    this.older = false;
  }

  async load(client: Client | null, handle: (error: unknown) => string, cursor: string | null = null): Promise<void> {
    this.clear();
    if (!client) return;
    const mine = this.generation;
    this.loading = true;
    try {
      let response;
      try {
        response = await client.call('feedback.history', { limit: 20, cursor });
      } catch (e) {
        if (mine !== this.generation) return;
        if (!cursor || !(e instanceof LoomwardError) || e.code !== 'stale_generation') throw e;
        cursor = null;
        this.notice = 'History changed or expired. Showing the newest records.';
        response = await client.call('feedback.history', { limit: 20, cursor });
      }
      if (mine !== this.generation) return;
      this.page = response.result;
      this.older = cursor !== null;
    } catch (e) {
      if (mine === this.generation) this.problem = handle(e);
    } finally {
      if (mine === this.generation) this.loading = false;
    }
  }
}
