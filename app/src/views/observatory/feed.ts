import { LoomwardError, type Client } from '../../lib/transport/client';
import type { DatasetClass, ProcessList } from '../../lib/contracts.gen';
import { readProcesses, TelemetryLease } from '../companion/telemetry';
import { PROCESS_LIMIT, PROCESS_REFRESH_MS, type ProcessLens } from './processes';

export interface ProcessReading { list: ProcessList | null; dataset: DatasetClass | null; error: string; note: string; lens: ProcessLens }

/** Gauges and strip share one lease and one poller; process reads follow the enumeration cadence. */
export class ObservatoryFeed {
  private readonly lease: TelemetryLease;
  private closed = false;
  private dueAt = 0;
  private sort: ProcessLens | null = null;

  constructor(private readonly client: Client, private readonly processesAvailable: boolean, private readonly now = Date.now, private readonly epoch = 0) {
    this.lease = new TelemetryLease(client, processesAvailable ? ['system', 'gpu', 'disks', 'processes'] : ['system', 'gpu', 'disks'], 1000, now);
  }

  forSession(client: Client, processesAvailable: boolean, epoch: number): ObservatoryFeed {
    if (!this.closed && this.client === client && this.processesAvailable === processesAvailable && this.epoch === epoch) return this;
    this.close();
    return new ObservatoryFeed(client, processesAvailable, this.now, epoch);
  }

  async read(lens: ProcessLens) {
    await this.lease.ensure();
    if (this.closed) throw new Error('Observatory closed');
    const telemetry = await this.client.call('telemetry.snapshot', { channels: ['system', 'gpu', 'disks'] });
    let processes: ProcessReading | null = null;
    if (!this.closed && this.processesAvailable && (this.now() >= this.dueAt || this.sort !== lens)) {
      this.dueAt = this.now() + PROCESS_REFRESH_MS;
      this.sort = lens;
      try {
        const { result, meta } = await readProcesses(this.client, lens, PROCESS_LIMIT);
        processes = { list: result, dataset: meta.dataset_class, error: '', note: '', lens };
      } catch (e) {
        if (!(e instanceof LoomwardError)) throw e;
        processes = { list: null, dataset: null, error: e.code, note: e.message, lens };
      }
    }
    return { ...telemetry, processes };
  }

  close(): void { this.closed = true; this.lease.close(); }
}
