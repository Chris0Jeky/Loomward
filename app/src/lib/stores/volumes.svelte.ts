import type { Volume } from '../contracts.gen';
import { session } from './session.svelte';

class Volumes {
  items = $state<Volume[] | null>(null);
  error = $state('');
  private ticket = 0;

  async refresh(): Promise<void> {
    const c = session.client;
    const mine = ++this.ticket;
    this.items = null;
    this.error = '';
    if (!c) return;
    try {
      const { result } = await c.call('volumes.list', {});
      if (mine === this.ticket) this.items = result.volumes;
    } catch (e) {
      if (mine === this.ticket) this.error = session.handle(e);
    }
  }

  changed(items: Volume[]): void {
    this.ticket++;
    this.items = items;
    this.error = '';
  }
}

export const volumes = new Volumes();
