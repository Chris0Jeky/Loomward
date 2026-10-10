import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { Poller } from '../../src/views/observatory/poller';
import type { TelemetrySample } from '../../src/lib/contracts.gen';

const now = () => new Date('2026-10-01T00:00:00Z');

describe('telemetry poller', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('survives a mock that throws once: reports the failure, backs off, then resumes', async () => {
    const client = new Client(createMockTransport({ now }));
    let throwNext = false;
    const events: string[] = [];
    const p = new Poller<TelemetrySample>({
      call: async () => {
        if (throwNext) { throwNext = false; throw new Error('mock telemetry failed once'); }
        return (await client.call('telemetry.snapshot', { channels: ['system', 'gpu', 'disks'] })).result;
      },
      onSample: (s) => events.push(`sample ${s.sample_seq}`),
      onError: (_e, n) => events.push(`error ${n}`),
      intervalMs: 1000,
      backoffMs: [2000, 4000],
    });
    p.start();
    await vi.advanceTimersByTimeAsync(0);
    expect(events).toEqual(['sample 1']);
    throwNext = true;
    await vi.advanceTimersByTimeAsync(1000);
    expect(events).toEqual(['sample 1', 'error 1']);
    await vi.advanceTimersByTimeAsync(1000); // still backing off (2 s)
    expect(events).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(1000);
    expect(events).toEqual(['sample 1', 'error 1', 'sample 2']);
    expect(p.consecutiveFailures).toBe(0);
    p.stop();
  });

  it('never overlaps calls, and kick() polls at once after a failure', async () => {
    let inFlight = 0, maxInFlight = 0, calls = 0;
    let release: (() => void) | null = null;
    const p = new Poller<number>({
      call: () => new Promise<number>((resolve, reject) => {
        calls++; inFlight++; maxInFlight = Math.max(maxInFlight, inFlight);
        release = () => { inFlight--; if (calls === 1) reject(new Error('down')); else resolve(calls); };
      }),
      onSample: () => {}, onError: () => {}, intervalMs: 1000, backoffMs: [30000],
    });
    p.start();
    p.kick(); p.kick(); // a busy poller ignores kicks while a call is in flight
    await vi.advanceTimersByTimeAsync(5000);
    expect(calls).toBe(1);
    release!();
    await vi.advanceTimersByTimeAsync(0);
    expect(p.consecutiveFailures).toBe(1);
    p.kick(); // e.g. the session epoch changed: no 30 s wait
    await vi.advanceTimersByTimeAsync(0);
    expect(calls).toBe(2);
    expect(maxInFlight).toBe(1);
    release!();
    p.stop();
  });
});
