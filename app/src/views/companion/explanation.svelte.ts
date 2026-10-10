import { LoomwardError, type Client } from '../../lib/transport/client';
import type { ProcessExplanation, ProcessList, ProcessRow } from '../../lib/contracts.gen';

export class Explanation {
  answer = $state<{ row: ProcessRow | null; result: ProcessExplanation } | null>(null);
  error = $state('');
  pending = $state<string | null>(null);
  private ticket = 0;

  constructor(private readonly client: () => Client | null, private readonly handle: (e: unknown) => string) {}

  async open(ref: string, row: ProcessRow | null = null, retry = true): Promise<void> {
    const c = this.client();
    if (!c) return;
    const mine = ++this.ticket;
    this.error = '';
    this.answer = null;
    this.pending = null;
    try {
      const { result } = await c.call('processes.explain', { process_ref: ref });
      if (mine === this.ticket) this.answer = { row, result };
    } catch (e) {
      if (mine !== this.ticket) return;
      // Last-lease teardown clears the native sample before Companion's sampler is ready.
      if (retry && !row && e instanceof LoomwardError && e.code === 'partial_coverage') this.pending = ref;
      else this.error = this.handle(e);
    }
  }

  async resume(sample: ProcessList | null): Promise<void> {
    if (sample && this.pending) await this.open(this.pending, null, false);
  }

  close(): void { this.ticket++; this.pending = null; }
}
