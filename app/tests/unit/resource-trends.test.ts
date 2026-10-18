import { expect, it } from 'vitest';
import type { TrendSeries } from '../../src/lib/contracts.gen';
import { segments, trendAge, trendLabel } from '../../src/views/observatory/trends';

const series = (values: (number | string | null)[], times = values.map((_, i) => i * 1000)): TrendSeries => ({
  key: 'system', metric: 'cpu_busy_fraction', unit: 'fraction', max_gap_ms: 2000, unknowns: [],
  points: values.map((value, i) => ({ offset_ms: times[i]!, observed_at: '2026-10-09T12:00:00Z', value })),
} as TrendSeries);

it('draws separate paths around missing samples and elapsed gaps, preserving zero', () => {
  expect(segments(series([0, 0.5, null, 0.75, 1]))).toHaveLength(2);
  expect(segments(series([0, 0.5, 0.75, 1], [0, 1000, 5000, 6000]))).toHaveLength(2);
  expect(segments(series([null, null]))).toEqual([]);
  expect(segments(series([0, 0.5, 1], [0, 0, 1000]))).toHaveLength(2);
  expect(segments(series([0, 1], [1000, 0]))).toEqual([]);
  expect(segments(series([0, 0]))[0]).toContain('L');
});

it('uses elapsed spacing and byte differences before approximate chart conversion', () => {
  const uneven = segments(series([0, 0.5, 1], [0, 1000, 3000]));
  expect(uneven[0]).toContain('L34,');
  const bytes = { ...series(['18446744073709551613', '18446744073709551614', '18446744073709551615']), metric: 'private_commit_bytes', unit: 'bytes' } as TrendSeries;
  expect(segments(bytes)[0]).toBe('M2,38 L50,20 L98,2');
});

it('refuses unit mismatches and malformed values without drawing them as zero', () => {
  expect(segments({ ...series([0.1, 0.2]), unit: 'queue' } as TrendSeries)).toEqual([]);
  expect(segments(series([90, 100]))).toEqual([]);
  expect(segments(series([-0.1, -0.2]))).toEqual([]);
  expect(trendLabel(Infinity, 'queue')).toBe('unknown');
  expect(segments(series(['0.1', '0.2']))).toEqual([]);
  expect(trendLabel(null, 'bytes')).toBe('unknown');
  expect(trendLabel('1048576', 'bytes')).toBe('1.00 MiB');
  expect(trendLabel(0.95, 'fraction')).toBe('95.0%');
  expect(trendLabel(3.5, 'queue')).toBe('3.5 queued');
  expect(trendLabel(4096, 'bytes_per_s')).toBe('4.00 KiB/s');
  expect(trendLabel('4096', 'queue')).toBe('unknown');
});

it('states the age of a series whose last point trails the sample, and stays silent when fresh or when ages are unreadable', () => {
  const at = (iso: string) => ({ ...series([0.5]), points: [{ offset_ms: 0, observed_at: iso, value: 0.5 }] }) as TrendSeries;
  const sample = '2026-10-09T12:00:15Z';
  expect(trendAge(at('2026-10-09T12:00:15Z'), sample)).toBeNull();
  expect(trendAge(at('2026-10-09T12:00:14Z'), sample)).toBeNull(); // within one sampler interval
  expect(trendAge(at('2026-10-09T12:00:03Z'), sample)).toBe('Last point observed 12 s before this sample');
  expect(trendAge(at('2026-10-09T11:58:00Z'), sample)).toBe('Last point observed 2 min before this sample');
  expect(trendAge(at('not-a-time'), sample)).toBe('The age of the last point is unknown');
  expect(trendAge(at('2026-10-09T12:00:03Z'), 'not-a-time')).toBeNull();
  expect(trendAge({ ...series([]), points: [] } as TrendSeries, sample)).toBeNull();
});
