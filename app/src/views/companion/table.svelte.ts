// The Companion's process table: polling, the sampler lease, and what holds a row still (WCAG 2.2.2).
// Lives outside the component so the call pattern can be tested: which commands a sort change makes,
// and when a new sample is shown or kept waiting.
import { LoomwardError, type Client } from '../../lib/transport/client';
import type { OwnBudgets, ProcessList, TelemetrySample } from '../../lib/contracts.gen';
import type { ProcessSort } from '../../lib/derived';

export const POLL_MS = 3000;
export const LEASE_RENEW_MS = 20000;

export interface TableDeps {
  client(): Client | null;
  available(): boolean;
  /** Session errors pass through the shell (a transport fault flips it to "unavailable"). */
  handle(e: unknown): string;
  /** Is the reader's focus inside the table right now? Asked at each sample, never remembered. */
  holdsFocus(): boolean;
  now?(): number;
}

export class ProcessTable {
  sort = $state<ProcessSort>('private_desc');
  limit = $state(20);
  list = $state<ProcessList | null>(null);
  /** A newer sample that is kept back while the reader works inside the table. */
  waiting = $state<ProcessList | null>(null);
  sample = $state<TelemetrySample | null>(null);
  budgets = $state<OwnBudgets | null>(null);
  error = $state('');
  paused = $state(false);

  private ticket = 0;
  private sub: string | null = null;
  private renewAt = 0;

  constructor(private readonly deps: TableDeps) {}

  private get now(): number { return (this.deps.now ?? Date.now)(); }

  /** Poll until the returned function is called, which also releases the sampler lease. */
  run(): () => void {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let alive = true;
    const loop = async () => {
      if (typeof document === 'undefined' || !document.hidden) await this.tick();
      if (alive) timer = setTimeout(loop, POLL_MS);
    };
    void loop();
    const c = this.deps.client();
    return () => {
      alive = false;
      clearTimeout(timer);
      this.ticket++;
      if (c && this.sub !== null) void c.call('telemetry.unsubscribe', { subscription_id: this.sub }).catch(() => {});
      this.sub = null;
    };
  }

  private async tick(): Promise<void> {
    const c = this.deps.client();
    if (!c || !this.deps.available()) return;
    const mine = ++this.ticket;
    try {
      // The sampler runs only while a 60 s lease is live: take one, renew it well inside the window.
      if (this.sub === null || this.now >= this.renewAt) {
        const lease = await c.call('telemetry.subscribe', { subscription_id: this.sub, channels: ['system', 'processes', 'engine'], interval_ms: 2000 });
        this.sub = lease.result.subscription_id;
        this.renewAt = this.now + LEASE_RENEW_MS;
      }
      const [s, p, b] = await Promise.all([
        c.call('telemetry.snapshot', { channels: ['system', 'engine'] }),
        c.call('processes.list', { sort: this.sort, limit: this.limit }),
        this.budgets ? Promise.resolve(null) : c.call('budgets.get', {}),
      ]);
      if (mine !== this.ticket) return;
      this.sample = s.result;
      if (this.deps.holdsFocus()) this.waiting = p.result; else { this.list = p.result; this.waiting = null; }
      if (b) this.budgets = b.result;
      this.error = '';
    } catch (e) {
      if (mine !== this.ticket) return;
      if (e instanceof LoomwardError && e.code === 'not_found') { this.sub = null; this.renewAt = 0; }
      this.error = this.deps.handle(e);
    }
  }

  /**
   * The sort or the row limit changed. Running, that is one more poll. Paused, the sampler lease has been
   * released and must stay released: ask only for the rows under the new order. The engine answers from
   * its last stored sample, so the table still shows the sample it was paused on.
   */
  async reorder(): Promise<void> {
    if (!this.paused) return this.tick();
    const c = this.deps.client();
    if (!c || !this.deps.available()) return;
    const mine = ++this.ticket;
    try {
      const p = await c.call('processes.list', { sort: this.sort, limit: this.limit });
      if (mine !== this.ticket) return;
      this.list = p.result;
      this.waiting = null;
      this.error = '';
    } catch (e) {
      if (mine !== this.ticket) return;
      this.error = this.deps.handle(e);
    }
  }

  /** Focus left the table: show the sample that was kept back. */
  release(): void {
    if (this.waiting) { this.list = this.waiting; this.waiting = null; }
  }
}
