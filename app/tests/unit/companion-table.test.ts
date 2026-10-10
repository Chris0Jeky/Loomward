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

  it('sorts and limits retained rows while paused without any transport call or polling', async () => {
    const { table, calls, count } = setup();
    const stop = table.run();
    await vi.advanceTimersByTimeAsync(0);
    table.paused = true;
    stop(); // what the view's effect does when paused flips
    await vi.advanceTimersByTimeAsync(0);
    const held = table.list!.sample_seq;
    const retained = table.list!;
    calls.length = 0;

    table.sort = 'name_asc';
    await table.reorder();
    expect(table.list!.rows.map((r) => r.process_ref)).toEqual([...retained.rows].sort((a, b) => a.name.localeCompare(b.name)).map((r) => r.process_ref));
    table.limit = 10;
    await table.reorder();
    expect(calls).toEqual([]);
    expect(table.list!.rows).toHaveLength(10);
    expect(table.list!.sample_seq).toBe(held); // still the sample the pause was taken on
    expect(table.list!.observed_at).toBe(retained.observed_at);
    expect(table.paused).toBe(true);
    table.limit = 100;
    await table.reorder();
    expect(table.list!.rows).toHaveLength(retained.rows.length); // increasing the limit cannot invent unseen rows
    expect(table.list!.truncated).toBe(true);

    // Every paused lens sorts the same retained set, with exact byte strings and unknowns last.
    const base = retained.rows[0]!;
    const rows = [
      { ...base, pid: 30, process_ref: 'pc_c', name: 'Charlie', access: 'full' as const, private_commit_bytes: '9007199254740993', working_set_bytes: '1', cpu_fraction: 0.1, io_read_bytes_per_s: 2, io_write_bytes_per_s: 1, gpu_dedicated_bytes: '2' },
      { ...base, pid: 20, process_ref: 'pc_b', name: 'Bravo', access: 'full' as const, private_commit_bytes: '9007199254740992', working_set_bytes: '3', cpu_fraction: 0.3, io_read_bytes_per_s: 4, io_write_bytes_per_s: null, gpu_dedicated_bytes: '1' },
      { ...base, pid: 10, process_ref: 'pc_a', name: 'Alpha', access: 'limited' as const, private_commit_bytes: null, working_set_bytes: '2', cpu_fraction: 0.3, io_read_bytes_per_s: 2, io_write_bytes_per_s: 2, gpu_dedicated_bytes: '3' },
      { ...base, pid: 40, process_ref: 'pc_d', name: 'Delta', access: 'denied' as const, private_commit_bytes: null, working_set_bytes: null, cpu_fraction: null, io_read_bytes_per_s: null, io_write_bytes_per_s: null, gpu_dedicated_bytes: null },
    ];
    const holds = setup();
    holds.table.paused = true;
    holds.table.list = { ...retained, rows, observed_count: 4, denied_count: 1, truncated: false };
    for (const [sort, expected] of [
      ['private_desc', [30, 20, 10, 40]], ['working_set_desc', [20, 10, 30, 40]],
      ['cpu_desc', [10, 20, 30, 40]], ['io_desc', [10, 30, 20, 40]],
      ['gpu_desc', [10, 30, 20, 40]], ['name_asc', [10, 20, 30, 40]],
    ] as const) {
      holds.table.sort = sort;
      await holds.table.reorder();
      expect(holds.table.list!.rows.map((r) => r.pid)).toEqual(expected);
      expect(holds.table.list!.truncated).toBe(false);
    }
    expect(holds.calls).toEqual([]);

    await vi.advanceTimersByTimeAsync(POLL_MS * 4);
    expect(count('processes.list')).toBe(0);
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
