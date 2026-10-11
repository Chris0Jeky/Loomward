import { describe, expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import type { ErrorCode, RequestEnvelope, TelemetryChannel, TelemetryTrends, TrendCandidate, TrendSeries } from '../../src/lib/contracts.gen';
import { ObservatoryFeed } from '../../src/views/observatory/feed';
import { TelemetryLease } from '../../src/views/companion/telemetry';

function setup() {
  let time = Date.parse('2026-10-10T12:00:00Z');
  let error = '';
  const now = () => time;
  const inner = createMockTransport({ now: () => new Date(now()) });
  const calls: RequestEnvelope[] = [];
  const client = new Client({ ...inner, call: async (req, signal) => {
    calls.push(req);
    if (req.command === 'processes.list' && error) return { protocol: 'loomward/3', request_id: req.request_id, ok: false, error: { code: error as 'partial_coverage', message: 'synthetic coverage fault', retryable: true, detail: null } };
    return inner.call(req, signal);
  } });
  const feed = new ObservatoryFeed(client, true, now);
  const count = (cmd: string) => calls.filter((c) => c.command === cmd).length;
  return { feed, client, calls, count, advance: (ms: number) => { time += ms; }, fail: (code: string) => { error = code; } };
}

describe('Observatory shared telemetry feed', () => {
  it('uses one 1 Hz lease for gauges and strip, reads five rows only every 15 s, and renews the same ID', async () => {
    const { feed, calls, count, advance } = setup();
    await feed.read('cpu_desc');
    expect(calls[0]!.payload).toEqual({ subscription_id: null, channels: ['system', 'gpu', 'disks', 'processes'], interval_ms: 1000 });
    expect(calls.find(c => c.command === 'telemetry.snapshot')!.payload?.channels).toEqual(['system', 'gpu', 'disks', 'processes']);
    expect(calls.find((c) => c.command === 'processes.list')!.payload).toEqual({ sort: 'cpu_desc', limit: 5 });
    for (let i = 0; i < 14; i++) { advance(1000); expect((await feed.read('cpu_desc')).processes).toBeNull(); }
    expect(count('processes.list')).toBe(1);
    advance(1000);
    expect((await feed.read('cpu_desc')).processes?.list?.rows).toHaveLength(5);
    advance(5000);
    await feed.read('cpu_desc');
    const subscriptions = calls.filter((c) => c.command === 'telemetry.subscribe');
    expect(subscriptions).toHaveLength(2);
    expect(subscriptions[1]!.payload?.subscription_id).toBe('sub_mock_1');
    expect(count('processes.list')).toBe(2);
    feed.close();
    expect(count('telemetry.unsubscribe')).toBe(1);
    await expect(feed.read('cpu_desc')).rejects.toThrow('Observatory closed');
    expect(count('processes.list')).toBe(2);
  });

  it('a lens switch reuses the lease, while paused reads cannot start another', async () => {
    const { feed, count } = setup();
    await feed.read('cpu_desc');
    expect((await feed.read('private_desc')).processes?.list?.rows[0]!.name).toBe('vm-host.exe');
    expect(count('telemetry.subscribe')).toBe(1);
    feed.close();
    await expect(feed.read('private_desc')).rejects.toThrow();
    expect(count('telemetry.subscribe')).toBe(1);
  });

  it.each(['partial_coverage', 'capability_unavailable'])('keeps %s visible without replacing it with rows or failing the gauges', async (code) => {
    const { feed, fail } = setup();
    fail(code);
    const sample = await feed.read('cpu_desc');
    expect(sample.result.system).not.toBeNull();
    expect(sample.processes).toMatchObject({ list: null, dataset: null, error: code });
    feed.close();
  });

  it('omits process collection when the session has no process observation capability', async () => {
    const { client, calls, count } = setup();
    const feed = new ObservatoryFeed(client, false);
    expect((await feed.read('cpu_desc')).processes).toBeNull();
    expect(calls[0]!.payload?.channels).toEqual(['system', 'gpu', 'disks']);
    expect(calls.find(c => c.command === 'telemetry.snapshot')!.payload?.channels).toEqual(['system', 'gpu', 'disks']);
    expect(count('processes.list')).toBe(0);
    feed.close();
  });

  it('releases the old client lease and sends the next poll only to the replacement client', async () => {
    const old = setup(), next = setup();
    await old.feed.read('cpu_desc');
    const feed = old.feed.forSession(next.client, true, 0);
    expect(feed).not.toBe(old.feed);
    expect(old.calls.at(-1)).toMatchObject({ command: 'telemetry.unsubscribe', payload: { subscription_id: 'sub_mock_1' } });
    const oldCalls = old.calls.length;
    await feed.read('cpu_desc');
    expect(old.calls).toHaveLength(oldCalls);
    expect(next.calls.map((c) => c.command)).toEqual(['telemetry.subscribe', 'telemetry.snapshot', 'processes.list']);
    feed.close();
  });

  it('reuses an unchanged session, but releases and recreates the feed on epoch and capability changes', async () => {
    const { feed, client, calls, count } = setup();
    await feed.read('cpu_desc');
    expect(feed.forSession(client, true, 0)).toBe(feed);
    const restarted = feed.forSession(client, true, 1);
    expect(restarted).not.toBe(feed);
    expect(count('telemetry.unsubscribe')).toBe(1);
    expect((await restarted.read('cpu_desc')).processes?.list).not.toBeNull();
    const disabled = restarted.forSession(client, false, 1);
    expect(disabled).not.toBe(restarted);
    expect(count('telemetry.unsubscribe')).toBe(2);
    expect((await disabled.read('cpu_desc')).processes).toBeNull();
    expect(calls.filter((c) => c.command === 'telemetry.subscribe').at(-1)!.payload?.channels).toEqual(['system', 'gpu', 'disks']);
    const enabled = disabled.forSession(client, true, 1);
    expect(count('telemetry.unsubscribe')).toBe(3);
    expect((await enabled.read('cpu_desc')).processes?.list).not.toBeNull();
    expect(count('processes.list')).toBe(3);
    enabled.close();
  });

  it.each(['internal_error', 'not_found'] satisfies ErrorCode[])('retries after a %s renewal failure even if the wall clock moves back', async (code) => {
    let time = 1000, fail = false;
    const inner = createMockTransport();
    const calls: RequestEnvelope[] = [];
    const client = new Client({ ...inner, call: async (req, signal) => {
      calls.push(req);
      if (req.command === 'telemetry.subscribe' && fail) {
        fail = false;
        time = 5000; // Clock correction while a renewal is in flight.
        return { protocol: 'loomward/3', request_id: req.request_id, ok: false, error: { code, message: 'synthetic renewal failure', retryable: true, detail: null } };
      }
      return inner.call(req, signal);
    } });
    const lease = new TelemetryLease(client, ['system'], 1000, () => time);
    await lease.ensure();
    time += 20000;
    fail = true;
    await expect(lease.ensure()).rejects.toMatchObject({ code });
    await lease.ensure();
    const subscriptions = calls.filter((c) => c.command === 'telemetry.subscribe');
    expect(subscriptions).toHaveLength(3);
    expect(subscriptions[2]!.payload?.subscription_id).toBe(code === 'not_found' ? null : 'sub_mock_1');
    await lease.ensure();
    expect(calls.filter((c) => c.command === 'telemetry.subscribe')).toHaveLength(3);
    lease.close();
  });

  it('releases a subscription that finishes after the view has closed', async () => {
    const { client, count } = setup();
    const lease = new TelemetryLease(client, ['system', 'processes'], 1000);
    const pending = lease.ensure();
    lease.close();
    await pending;
    expect(count('telemetry.subscribe')).toBe(1);
    expect(count('telemetry.unsubscribe')).toBe(1);
    await lease.ensure();
    expect(count('telemetry.subscribe')).toBe(1);
  });

  it('recovers an expired lease on the next poll without keeping two live leases', async () => {
    const { feed, advance, count } = setup();
    await feed.read('cpu_desc');
    advance(65000);
    await expect(feed.read('cpu_desc')).rejects.toMatchObject({ code: 'not_found' });
    expect((await feed.read('cpu_desc')).processes?.list).not.toBeNull();
    expect(count('telemetry.subscribe')).toBe(3); // create, refused expired renewal, fresh create
    feed.close();
    expect(count('telemetry.unsubscribe')).toBe(1);
  });

  it('keeps mock row, list and explanation timestamps at enumeration time, even as snapshots advance or sampling stops', async () => {
    const { feed, client, advance } = setup();
    const first = (await feed.read('cpu_desc')).processes!.list!;
    expect(first.rows.every((r) => r.cpu_fraction === null)).toBe(true);
    advance(12000);
    const newer = await feed.read('cpu_desc');
    expect(newer.result.observed_at).not.toBe(first.observed_at);
    const retained = (await client.call('processes.list', { sort: 'cpu_desc', limit: 5 })).result;
    expect(retained.observed_at).toBe(first.rows[0]!.observed_at);
    expect(retained.rows[0]!.observed_at).toBe(first.rows[0]!.observed_at);
    advance(3000);
    const next = (await feed.read('cpu_desc')).processes!.list!;
    expect(next.rows.some((r) => r.cpu_fraction !== null)).toBe(true);
    expect(next.observed_at).not.toBe(first.observed_at);
    feed.close();
    advance(16000);
    const paused = (await client.call('processes.list', { sort: 'private_desc', limit: 5 })).result;
    expect(paused.observed_at).toBe(next.observed_at);
    const explanation = (await client.call('processes.explain', { process_ref: paused.rows[0]!.process_ref })).result;
    expect(explanation.observed_at).toBe(paused.rows[0]!.observed_at);
  });

  describe('process trends ride only the due poll (#377)', () => {
    const PROCESS_NOTE = 'Shared resident memory is not measured uniquely; process working sets overlap.';
    const PROCESS_METRICS = ['private_commit_bytes', 'private_working_set_bytes', 'working_set_bytes', 'cpu_fraction', 'io_read_bytes_per_s', 'io_write_bytes_per_s', 'gpu_dedicated_bytes'] as const;
    const unitOf = (m: string) => (m === 'cpu_fraction' ? 'fraction' : m.endsWith('_per_s') ? 'bytes_per_s' : 'bytes');
    const make = (key: string, metric: string, unit: string, at: string) => ({ key, metric, unit, max_gap_ms: 30000, points: Array.from({ length: 8 }, (_, i) => ({ offset_ms: i * 1000, observed_at: at, value: unit === 'bytes' ? String(1_000_000 + i) : 0.5 })), unknowns: [PROCESS_NOTE] }) as unknown as TrendSeries;
    const candidate = (kind: TrendCandidate['kind'], series_index: number): TrendCandidate => ({ kind, series_index, unit: 'fraction', start_at: 'a', end_at: 'b', window_ms: 7000, sample_count: 8, start_value: 0.5, end_value: 0.5, slope_per_s: '0', facts: [], unknowns: [] });
    const channelOf = (s: TrendSeries) => (s.key === 'system' ? 'system' : s.key.startsWith('disk_') ? 'disks' : 'processes');

    /** An engine stand-in: the report it builds for the requested channels, in its key order (disks, processes, system). */
    function engine(processes: number) {
      const all = [
        make('disk_0:C:', 'disk_queue_length', 'queue', 'machine-at'),
        ...Array.from({ length: processes }, (_, p) => PROCESS_METRICS.map((m) => make(`pc_${String(p).padStart(5, '0')}`, m, unitOf(m), 'processes-at'))).flat(),
        make('system', 'cpu_busy_fraction', 'fraction', 'machine-at'),
      ];
      const memo = new Map<string, TelemetryTrends>();
      return (channels: TelemetryChannel[]): TelemetryTrends => {
        const id = [...channels].sort().join();
        if (!memo.has(id)) {
          const series = all.filter((s) => channels.includes(channelOf(s)));
          const at = (key: string) => series.findIndex((s) => s.key === key);
          const candidates = [candidate('disk_queue', at('disk_0:C:')), candidate('growth', at('pc_00000')), candidate('cpu_saturation', at('system'))].filter((c) => c.series_index >= 0);
          memo.set(id, { series, candidates, candidates_truncated: false, unknowns: channels.includes('processes') ? ['Process coverage is truncated; unobserved processes and shared memory remain unknown.'] : [] });
        }
        return memo.get(id)!;
      };
    }

    function trendSetup(reports: (channels: TelemetryChannel[]) => TelemetryTrends) {
      let time = Date.parse('2026-10-10T12:00:00Z');
      let refuse = false;
      const inner = createMockTransport({ now: () => new Date(time) });
      const asked: TelemetryChannel[][] = [];
      const leased: TelemetryChannel[][] = [];
      let bytes = 0;
      const sizes = new Map<TelemetryTrends, number>();
      const client = new Client({ ...inner, call: async (req, signal) => {
        const res = await inner.call(req, signal);
        if (req.command === 'telemetry.subscribe') leased.push((req.payload as { channels: TelemetryChannel[] }).channels);
        if (req.command !== 'telemetry.snapshot') return res;
        if (refuse) { refuse = false; return { protocol: 'loomward/3', request_id: req.request_id, ok: false, error: { code: 'internal_error', message: 'synthetic refusal', retryable: true, detail: null } }; }
        const channels = (req.payload as { channels: TelemetryChannel[] }).channels;
        asked.push(channels);
        const trends = reports(channels);
        if (!sizes.has(trends)) sizes.set(trends, JSON.stringify(trends).length);
        bytes += sizes.get(trends)!;
        return { ...res, result: { ...(res as { result: object }).result, trends } } as typeof res;
      } });
      return { client, asked, leased, now: () => time, advance: (ms: number) => { time += ms; }, refuse: () => { refuse = true; }, bytes: () => bytes };
    }
    const hasProcessSeries = (r: { result: { trends: TelemetryTrends } }) => r.result.trends.series.some((s) => s.key.startsWith('pc_'));

    it('asks for processes only when the process read is due, and the lease keeps the sampler collecting', async () => {
      const t = trendSetup(engine(2));
      const feed = new ObservatoryFeed(t.client, true, t.now);
      for (let i = 0; i < 31; i++) { await feed.read('cpu_desc'); t.advance(1000); }
      expect(t.asked.flatMap((c, i) => (c.includes('processes') ? [i] : []))).toEqual([0, 15, 30]);
      expect(t.leased.length).toBeGreaterThan(1); // created, then renewed
      for (const channels of t.leased) expect(channels).toEqual(['system', 'gpu', 'disks', 'processes']);
      feed.close();
    });

    it('carries the last process series and candidates forward on the polls in between, with their own timestamps', async () => {
      const t = trendSetup(engine(2));
      const feed = new ObservatoryFeed(t.client, true, t.now);
      const due = (await feed.read('cpu_desc')).result.trends;
      t.advance(1000);
      const carried = (await feed.read('cpu_desc')).result.trends;
      expect(t.asked[1]).not.toContain('processes');
      const names = (r: TelemetryTrends) => r.series.map((s) => `${s.key}/${s.metric}`).sort();
      expect(names(carried)).toEqual(names(due));
      expect(carried.series.filter((s) => s.key.startsWith('pc_')).every((s) => s.points.every((p) => p.observed_at === 'processes-at'))).toBe(true);
      expect(carried.series.filter((s) => s.key === 'system').every((s) => s.points.every((p) => p.observed_at === 'machine-at'))).toBe(true);
      // every candidate still points at the series it named, wherever the carried series landed
      const named = (r: TelemetryTrends) => r.candidates.map((c) => `${c.kind}:${r.series[c.series_index]!.key}`).sort();
      expect(named(due)).toEqual(['cpu_saturation:system', 'disk_queue:disk_0:C:', 'growth:pc_00000']);
      expect(named(carried)).toEqual(named(due));
      expect(carried.unknowns).toEqual(due.unknowns);
      expect(carried.candidates_truncated).toBe(false);
      feed.close();
    });

    it('never exceeds the engine candidate cap when carried candidates join fresh ones', async () => {
      // The due report held 2 machine + 62 process candidates; a later machine-only poll finds 4 machine ones.
      const procs = [make('system', 'cpu_busy_fraction', 'fraction', 'x'), make('pc_00000', 'cpu_fraction', 'fraction', 'x')];
      const machine = (n: number) => Array.from({ length: n }, () => candidate('cpu_saturation', 0));
      const t = trendSetup((channels) => channels.includes('processes')
        ? { series: procs, candidates: [...machine(2), ...Array.from({ length: 62 }, () => candidate('growth', 1))], candidates_truncated: false, unknowns: [] }
        : { series: procs.slice(0, 1), candidates: machine(4), candidates_truncated: false, unknowns: [] });
      const feed = new ObservatoryFeed(t.client, true, t.now);
      await feed.read('cpu_desc');
      t.advance(1000);
      const next = (await feed.read('cpu_desc')).result.trends;
      expect(next.candidates).toHaveLength(64);
      expect(next.candidates.filter((c) => c.kind === 'cpu_saturation')).toHaveLength(4);
      expect(next.candidates_truncated).toBe(true);
      feed.close();
    });

    it('a refused snapshot drops the carried process trends, and the next good poll asks for them again', async () => {
      const t = trendSetup(engine(2));
      const feed = new ObservatoryFeed(t.client, true, t.now);
      await feed.read('cpu_desc');
      expect((feed as unknown as { processTrends: unknown }).processTrends).not.toBeNull();
      t.advance(1000);
      t.refuse();
      await expect(feed.read('cpu_desc')).rejects.toMatchObject({ code: 'internal_error' });
      expect((feed as unknown as { processTrends: unknown }).processTrends).toBeNull();
      t.advance(1000);
      const next = await feed.read('cpu_desc');
      expect(t.asked.at(-1)).toContain('processes');
      expect(hasProcessSeries(next)).toBe(true);
      feed.close();
    });

    it('keeps a report-level unknown that only a non-due poll carries, beside the carried process unknowns', async () => {
      const MACHINE = 'Machine-level gap that is not about processes.';
      const base = engine(2);
      const t = trendSetup((ch) => { const r = base(ch); return ch.includes('processes') ? r : { ...r, unknowns: [MACHINE] }; }); // the machine gap shows on non-due polls only
      const feed = new ObservatoryFeed(t.client, true, t.now);
      await feed.read('cpu_desc');
      t.advance(1000);
      const carried = await feed.read('cpu_desc');
      expect(t.asked.at(-1)).not.toContain('processes');
      expect(carried.result.trends.unknowns.filter((u) => u === MACHINE)).toHaveLength(1);
      expect(carried.result.trends.unknowns).toContain('Process coverage is truncated; unobserved processes and shared memory remain unknown.');
      feed.close();
    });

    it('a replacement session starts without the old session process trends', async () => {
      const old = trendSetup(engine(2)), next = trendSetup(engine(0));
      const feed = new ObservatoryFeed(old.client, true, old.now);
      await feed.read('cpu_desc');
      const replacement = feed.forSession(next.client, true, 1);
      next.advance(1000);
      const read = await replacement.read('cpu_desc');
      expect(next.asked[0]).toContain('processes');
      expect(hasProcessSeries(read)).toBe(false);
      replacement.close();
    });

    it('leaves the report alone when the session has no process observation', async () => {
      const reports = engine(2);
      const t = trendSetup(reports);
      const feed = new ObservatoryFeed(t.client, false, t.now);
      const read = await feed.read('cpu_desc');
      expect(t.asked[0]).toEqual(['system', 'gpu', 'disks']);
      expect(read.result.trends).toBe(reports(['system', 'gpu', 'disks']));
      expect(hasProcessSeries(read)).toBe(false);
      feed.close();
    });

    it('cuts the trend bytes requested per minute at the synthetic worst case (1,024 processes, 7,168 process series)', async () => {
      const reports = engine(1024);
      const t = trendSetup(reports);
      const feed = new ObservatoryFeed(t.client, true, t.now);
      for (let i = 0; i < 60; i++) { await feed.read('cpu_desc'); t.advance(1000); }
      const withProcesses = t.asked.filter((c) => c.includes('processes')).length;
      // Before #377 every one of the 60 polls asked for the process channel too.
      const before = 60 * JSON.stringify(reports(['system', 'gpu', 'disks', 'processes'])).length;
      console.info(`trend bytes per minute: before ${before}, after ${t.bytes()} (${withProcesses} of 60 polls with processes)`);
      expect(withProcesses).toBe(4);
      expect(t.bytes()).toBeLessThan(before * 0.08);
      feed.close();
    });
  });
});
