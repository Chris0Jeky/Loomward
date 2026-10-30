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
    // volumes.changed rows never carry storage_path (LW-137): keep what the last volumes.list observed.
    const before = this.items;
    this.items = items.map((v) => {
      const kept = before?.find((o) => o.volume_id === v.volume_id)?.storage_path;
      return v.storage_path || !kept ? v : { ...v, storage_path: kept };
    });
    this.error = '';
  }
}

export const volumes = new Volumes();
