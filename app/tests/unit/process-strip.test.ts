import { describe, expect, it } from 'vitest';
import type { ProcessRow } from '../../src/lib/contracts.gen';
import { observationAge, processValue, stripRows, stripStatus } from '../../src/views/observatory/processes';

const at = '2026-10-10T12:00:00Z';
const row = (pid: number, cpu: number | null, bytes: string | null): ProcessRow => ({
  pid, process_ref: `pc_${pid}`, name: `synthetic-${pid}`, observed_at: at, started_at: null,
  cpu_fraction: cpu, private_commit_bytes: bytes, private_working_set_bytes: null, working_set_bytes: null,
  io_read_bytes_per_s: null, io_write_bytes_per_s: null, gpu_dedicated_bytes: null, access: 'full', loomward_owned: false,
});

describe('Observatory process strip', () => {
  it('sorts known machine CPU first, with stable PID ties and unknowns after zero', () => {
    const rows = [row(1, null, null), row(4, 0.5, '2'), row(3, 0.5, '1'), row(2, 0, '0')];
    expect(stripRows(rows, 'cpu_desc').map((r) => r.pid)).toEqual([3, 4, 2, 1]);
    expect(rows[0]!.pid).toBe(1);
  });
  it('sorts byte strings exactly, bounds the strip to five, and treats denied readings as unknown', () => {
    const rows = [row(1, 1, '9007199254740992'), row(2, 0, '9007199254740993'),
      { ...row(3, 1, '18446744073709551615'), access: 'denied' as const }, row(4, null, null), row(5, 0, '0'), row(6, 0, '1')];
    expect(stripRows(rows, 'private_desc').map((r) => r.pid)).toEqual([2, 1, 6, 5, 3]);
  });
  it('keeps unknowns distinct from measured zero for each lens', () => {
    expect(processValue(row(1, null, null), 'cpu_desc')).toBe('unknown');
    expect(processValue(row(1, 0, '0'), 'cpu_desc')).toBe('0.0%');
    expect(processValue(row(1, 0, '0'), 'private_desc')).toBe('0 B');
    expect(processValue(row(1, NaN, 'bad'), 'private_desc')).toBe('unknown');
    expect(processValue({ ...row(1, 1, '1'), access: 'denied' }, 'cpu_desc')).toBe('unknown');
  });
  it('ages the row timestamp, never the containing sample, and reports invalid or future clocks honestly', () => {
    const now = Date.parse(at);
    expect(observationAge(at, now + 12_900)).toBe('observed 12 s ago');
    expect(observationAge(at, now + 120_000)).toBe('observed 2 min ago');
    expect(observationAge(at, now + 7_200_000)).toBe('observed 2 h ago');
    expect(observationAge(at, now + 172_800_000)).toBe('observed 2 d ago');
    expect(observationAge('bad', now)).toBe('observation age unknown');
    expect(observationAge(at, now - 1)).toBe('observation age unknown (clock ahead)');
  });
  it('keeps partial coverage distinct from unavailable and includes denied counts', () => {
    expect(stripStatus(null, 'partial_coverage')).toContain('Partial coverage');
    expect(stripStatus(null, 'capability_unavailable')).toContain('Process observation unavailable');
    expect(stripStatus(null, '')).toBe('Waiting for process observations');
    expect(stripStatus({ rows: [], observed_count: 0, denied_count: 0, truncated: false, sample_seq: 1, observed_at: at, note: '' }, '')).toContain('No processes reported');
    expect(stripStatus({ rows: [row(1, 1, '2')], observed_count: 8, denied_count: 2, truncated: true, sample_seq: 1, observed_at: at, note: '' }, '')).toContain('2 could not be read');
  });
});
