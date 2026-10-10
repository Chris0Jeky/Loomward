// The Companion's process table: polling, the sampler lease, and what holds a row still (WCAG 2.2.2).
// Lives outside the component so the call pattern can be tested: which commands a sort change makes,
// and when a new sample is shown or kept waiting.
import type { Client } from '../../lib/transport/client';
import type { OwnBudgets, ProcessList, TelemetrySample } from '../../lib/contracts.gen';
import type { ProcessSort } from '../../lib/derived';
import { readProcesses, TelemetryLease } from './telemetry';

export const POLL_MS = 3000;

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
  private lease: TelemetryLease | null = null;

  constructor(private readonly deps: TableDeps) {}

  private get now(): number { return (this.deps.now ?? Date.now)(); }

  /** Poll until the returned function is called, which also releases the sampler lease. */
  run(): () => void {
    const c = this.deps.client();
    const lease = c ? new TelemetryLease(c, ['system', 'processes', 'engine'], 2000, () => this.now) : null;
    this.lease = lease;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let alive = true;
    const loop = async () => {
      if (typeof document === 'undefined' || !document.hidden) await this.tick();
      if (alive) timer = setTimeout(loop, POLL_MS);
    };
    void loop();
    return () => {
      alive = false;
      clearTimeout(timer);
      this.ticket++;
      lease?.close();
      if (this.lease === lease) this.lease = null;
    };
  }

  private async tick(): Promise<void> {
    const c = this.deps.client();
    if (!c || !this.deps.available()) return;
    const mine = ++this.ticket;
    try {
      await this.lease?.ensure();
      if (mine !== this.ticket) return;
      const [s, p, b] = await Promise.all([
        c.call('telemetry.snapshot', { channels: ['system', 'engine'] }),
        readProcesses(c, this.sort, this.limit),
        this.budgets ? Promise.resolve(null) : c.call('budgets.get', {}),
      ]);
      if (mine !== this.ticket) return;
      this.sample = s.result;
      if (this.deps.holdsFocus()) this.waiting = p.result; else { this.list = p.result; this.waiting = null; }
      if (b) this.budgets = b.result;
      this.error = '';
    } catch (e) {
      if (mine !== this.ticket) return;
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
      const p = await readProcesses(c, this.sort, this.limit);
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
