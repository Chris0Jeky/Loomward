import { describe, expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import type { RequestEnvelope } from '../../src/lib/contracts.gen';
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
    expect(count('processes.list')).toBe(0);
    feed.close();
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
});
