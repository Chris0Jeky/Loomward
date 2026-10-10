import { LoomwardError, type Client } from '../../lib/transport/client';
import type { TelemetryChannel, TelemetryInterval } from '../../lib/contracts.gen';
import type { ProcessSort } from '../../lib/derived';

export const LEASE_RENEW_MS = 20000;
export const readProcesses = (client: Client, sort: ProcessSort, limit: number) => client.call('processes.list', { sort, limit });

/** One view's sampler lease, shared by its snapshot and process reads. */
export class TelemetryLease {
  private id: string | null = null;
  private renewAt = 0;
  private closed = false;
  private pending: Promise<void> | null = null;

  constructor(private readonly client: Client, private readonly channels: TelemetryChannel[],
    private readonly interval: TelemetryInterval, private readonly now = Date.now) {}

  async ensure(): Promise<void> {
    if (this.closed) return;
    if (this.pending) return this.pending;
    if (this.id !== null && this.now() < this.renewAt) return;
    this.pending = this.renew();
    try { await this.pending; } finally { this.pending = null; }
  }

  private async renew(): Promise<void> {
    try {
      const { result } = await this.client.call('telemetry.subscribe', { subscription_id: this.id, channels: this.channels, interval_ms: this.interval });
      // A view can leave while subscribe is in flight: release that late lease too.
      if (this.closed) this.release(result.subscription_id);
      else { this.id = result.subscription_id; this.renewAt = this.now() + LEASE_RENEW_MS; }
    } catch (e) {
      this.renewAt = 0;
      if (e instanceof LoomwardError && e.code === 'not_found') this.id = null;
      throw e;
    }
  }

  private release(id: string): void { void this.client.call('telemetry.unsubscribe', { subscription_id: id }).catch(() => {}); }

  close(): void {
    this.closed = true;
    if (this.id !== null) this.release(this.id);
    this.id = null;
  }
}
