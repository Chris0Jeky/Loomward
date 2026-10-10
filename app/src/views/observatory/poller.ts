/**
 * Telemetry polling with one call in flight at a time, backoff after a failure, and resume on
 * success or on demand (`kick`, e.g. when the session epoch changes). Timers are injectable for tests.
 */
export interface PollerOptions<T> {
  call: () => Promise<T>;
  onSample: (sample: T) => void;
  onError: (error: unknown, attempt: number) => void;
  intervalMs: number;
  /** Waits after the 1st, 2nd, ... consecutive failure; the last value repeats. */
  backoffMs: number[];
  isHidden?: () => boolean;
  setTimer?: (f: () => void, ms: number) => unknown;
  clearTimer?: (t: unknown) => void;
}

export class Poller<T> {
  private timer: unknown = null;
  private inFlight = false;
  private failures = 0;
  private stopped = true;
  private readonly set: (f: () => void, ms: number) => unknown;
  private readonly clear: (t: unknown) => void;

  constructor(private readonly o: PollerOptions<T>) {
    this.set = o.setTimer ?? ((f, ms) => setTimeout(f, ms));
    this.clear = o.clearTimer ?? ((t) => clearTimeout(t as ReturnType<typeof setTimeout>));
  }

  get busy(): boolean { return this.inFlight; }
  get consecutiveFailures(): number { return this.failures; }

  start(): void { this.stopped = false; void this.tick(); }
  stop(): void { this.stopped = true; if (this.timer !== null) this.clear(this.timer); this.timer = null; }
  /** Poll now and forget the backoff (a reconnect or a fresh session). */
  kick(): void { if (this.stopped) return; this.failures = 0; if (this.timer !== null) this.clear(this.timer); this.timer = null; void this.tick(); }

  private schedule(ms: number): void {
    if (this.stopped) return;
    if (this.timer !== null) this.clear(this.timer);
    this.timer = this.set(() => { this.timer = null; void this.tick(); }, ms);
  }

  private async tick(): Promise<void> {
    if (this.stopped || this.inFlight) return;
    if (this.o.isHidden?.()) return this.schedule(this.o.intervalMs);
    this.inFlight = true;
    try {
      const s = await this.o.call();
      if (this.stopped) return;
      this.failures = 0;
      this.o.onSample(s);
      this.schedule(this.o.intervalMs);
    } catch (e) {
      if (this.stopped) return;
      this.failures++;
      this.o.onError(e, this.failures);
      const b = this.o.backoffMs;
      this.schedule(b[Math.min(this.failures - 1, b.length - 1)] ?? this.o.intervalMs);
    } finally {
      this.inFlight = false;
    }
  }
}
