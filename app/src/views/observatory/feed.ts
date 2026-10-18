import { LoomwardError, type Client } from '../../lib/transport/client';
import type { DatasetClass, ProcessList, TelemetryChannel, TelemetryTrends, TrendCandidate, TrendMetric, TrendSeries } from '../../lib/contracts.gen';
import { readProcesses, TelemetryLease } from '../companion/telemetry';
import { PROCESS_LIMIT, PROCESS_REFRESH_MS, type ProcessLens } from './processes';

export interface ProcessReading { list: ProcessList | null; dataset: DatasetClass | null; error: string; note: string; lens: ProcessLens }

const BASE_CHANNELS: TelemetryChannel[] = ['system', 'gpu', 'disks'];
const CANDIDATE_CAP = 64; // the engine's report cap
/** The series the engine files under the `processes` channel (`TrendMetric::channel`). */
const PROCESS_METRICS = new Set<TrendMetric>(['private_commit_bytes', 'private_working_set_bytes', 'working_set_bytes', 'cpu_fraction', 'io_read_bytes_per_s', 'io_write_bytes_per_s', 'gpu_dedicated_bytes']);

/**
 * One trend report split into its process part and the rest, candidate indexes following their series. The report-level
 * unknowns go to both parts, so one the engine files outside the `processes` channel is never lost on a non-due poll;
 * `joinTrends` shows each once.
 */
function splitTrends(t: TelemetryTrends) {
  const part = (keep: boolean): TelemetryTrends => {
    const at = new Map<number, number>();
    const series = t.series.filter((s, i) => PROCESS_METRICS.has(s.metric) === keep && (at.set(i, at.size), true));
    const candidates = t.candidates.flatMap((c) => { const n = at.get(c.series_index); return n === undefined ? [] : [{ ...c, series_index: n }]; });
    return { series, candidates, candidates_truncated: t.candidates_truncated, unknowns: t.unknowns };
  };
  return { rest: part(false), processes: part(true) };
}

/** Machine and disk trends first, then the process trends from the last poll that asked for them. */
function joinTrends(rest: TelemetryTrends, p: TelemetryTrends): TelemetryTrends {
  const room = Math.max(0, CANDIDATE_CAP - rest.candidates.length);
  const carried: TrendCandidate[] = p.candidates.map((c) => ({ ...c, series_index: c.series_index + rest.series.length }));
  return {
    series: [...rest.series, ...p.series],
    candidates: [...rest.candidates, ...carried.slice(0, room)],
    candidates_truncated: rest.candidates_truncated || p.candidates_truncated || carried.length > room,
    unknowns: [...new Set([...rest.unknowns, ...p.unknowns])],
  };
}

/** Gauges and strip share one lease and one poller; process reads (and the process trends) follow the enumeration cadence. */
export class ObservatoryFeed {
  private readonly lease: TelemetryLease;
  private closed = false;
  private dueAt = 0;
  private sort: ProcessLens | null = null;
  private processTrends: TelemetryTrends | null = null;

  constructor(private readonly client: Client, private readonly processesAvailable: boolean, private readonly now = Date.now, private readonly epoch = 0) {
    this.lease = new TelemetryLease(client, processesAvailable ? [...BASE_CHANNELS, 'processes'] : BASE_CHANNELS, 1000, now);
  }

  forSession(client: Client, processesAvailable: boolean, epoch: number): ObservatoryFeed {
    if (!this.closed && this.client === client && this.processesAvailable === processesAvailable && this.epoch === epoch) return this;
    this.close();
    return new ObservatoryFeed(client, processesAvailable, this.now, epoch);
  }

  async read(lens: ProcessLens) {
    try { return await this.poll(lens); } catch (e) {
      this.dueAt = 0; // the next good poll asks for the process part again
      this.processTrends = null;
      throw e;
    }
  }

  private async poll(lens: ProcessLens) {
    const due = this.processesAvailable && (this.now() >= this.dueAt || this.sort !== lens);
    await this.lease.ensure();
    if (this.closed) throw new Error('Observatory closed');
    // The lease keeps `processes` so the sampler collects; a snapshot carries the (large) process trends only when due.
    let telemetry = await this.client.call('telemetry.snapshot', { channels: due ? [...BASE_CHANNELS, 'processes'] : BASE_CHANNELS });
    if (this.processesAvailable) {
      const { rest, processes } = splitTrends(telemetry.result.trends);
      if (due) this.processTrends = processes;
      telemetry = { ...telemetry, result: { ...telemetry.result, trends: joinTrends(rest, this.processTrends ?? { series: [], candidates: [], candidates_truncated: false, unknowns: [] }) } };
    }
    let processes: ProcessReading | null = null;
    if (!this.closed && due) {
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
