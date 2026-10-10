// The Companion table's call pattern and its hold-under-focus rule (WCAG 2.2.2; issue #169).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { ProcessTable, POLL_MS } from '../../src/views/companion/table.svelte';

const now = () => new Date('2026-10-01T00:00:00Z');

function setup(holds = { value: false }) {
  const inner = createMockTransport({ now });
  const calls: string[] = [];
  const client = new Client({ ...inner, mode: inner.mode, subscribe: inner.subscribe, call: (req, signal) => { calls.push(req.command); return inner.call(req, signal); } });
  const table = new ProcessTable({ client: () => client, available: () => true, handle: (e) => String(e), holdsFocus: () => holds.value });
  const count = (cmd: string) => calls.filter((c) => c === cmd).length;
  return { table, calls, count, holds };
}

describe('Companion process table', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('polls with a lease while running, and a sort change polls once more under the same lease', async () => {
    const { table, calls, count } = setup();
    const stop = table.run();
    await vi.advanceTimersByTimeAsync(0);
    expect(count('telemetry.subscribe')).toBe(1);
    expect(count('processes.list')).toBe(1);
    table.sort = 'name_asc';
    await table.reorder();
    expect(count('processes.list')).toBe(2);
    expect(count('telemetry.subscribe')).toBe(1); // renewed only when the lease is due
    expect(table.list?.rows[0]?.name.localeCompare(table.list!.rows[1]!.name)).toBeLessThan(0);
    stop();
    await vi.advanceTimersByTimeAsync(0);
    expect(calls.filter((c) => c === 'telemetry.unsubscribe')).toHaveLength(1);
  });

  it('a sort or limit change while paused asks for the rows only: no lease, no snapshot, no polling', async () => {
    const { table, calls, count } = setup();
    const stop = table.run();
    await vi.advanceTimersByTimeAsync(0);
    table.paused = true;
    stop(); // what the view's effect does when paused flips
    await vi.advanceTimersByTimeAsync(0);
    const held = table.list!.sample_seq;
    calls.length = 0;

    table.sort = 'cpu_desc';
    await table.reorder();
    table.limit = 10;
    await table.reorder();
    expect(calls).toEqual(['processes.list', 'processes.list']); // not telemetry.subscribe: nothing would release that lease
    expect(table.list!.rows).toHaveLength(10);
    expect(table.list!.sample_seq).toBe(held); // still the sample the pause was taken on

    await vi.advanceTimersByTimeAsync(POLL_MS * 4);
    expect(count('processes.list')).toBe(2); // and nothing polls on its own
  });

  it('keeps a new sample waiting while focus is in the table, and shows it once focus has left, event or not', async () => {
    const holds = { value: true };
    const { table } = setup(holds);
    const stop = table.run();
    await vi.advanceTimersByTimeAsync(0);
    expect(table.list).toBeNull(); // the very first sample has nothing to hold still, but focus is asked at each sample
    expect(table.waiting).not.toBeNull();
    const first = table.waiting!.sample_seq;

    // The table unmounts or hides while focused: no focusout is ever delivered. The next sample must not stay held.
    holds.value = false;
    await vi.advanceTimersByTimeAsync(POLL_MS);
    expect(table.waiting).toBeNull();
    expect(table.list!.sample_seq).toBeGreaterThan(first);
    stop();
  });

  it('release() shows the kept-back sample at once', async () => {
    const holds = { value: true };
    const { table } = setup(holds);
    const stop = table.run();
    await vi.advanceTimersByTimeAsync(0);
    const waiting = table.waiting;
    table.release();
    expect(table.list).toEqual(waiting);
    expect(table.waiting).toBeNull();
    stop();
  });
});
