import type { TrendSeries, TrendValue, TrendUnit } from '../../lib/contracts.gen';
import { parseBytes, formatBytes } from '../../lib/format/bytes';

function reading(v: TrendValue | null, unit: TrendUnit): bigint | number | null {
  if (unit === 'bytes') return typeof v === 'string' ? parseBytes(v) : null;
  return typeof v === 'number' && Number.isFinite(v) && v >= 0 && (unit !== 'fraction' || v <= 1) ? v : null;
}

export function segments(series: TrendSeries): string[] {
  const unit = series.metric.endsWith('fraction') ? 'fraction' : series.metric === 'disk_queue_length' ? 'queue' : series.metric.endsWith('_per_s') ? 'bytes_per_s' : 'bytes';
  if (unit !== series.unit) return [];
  const values = series.points.map(p => reading(p.value, unit));
  const known = values.filter((v): v is bigint | number => v !== null);
  if (!known.length) return [];
  const low = known.reduce((a, b) => a < b ? a : b), high = known.reduce((a, b) => a > b ? a : b);
  // Subtract bytes as integers before display conversion, so adjacent u64 values remain distinct.
  const delta = (v: bigint | number, base: bigint | number) => typeof v === 'bigint' && typeof base === 'bigint' ? Number(v - base) : Number(v) - Number(base);
  const range = delta(high, low);
  const start = series.points[0]!.offset_ms, duration = series.points.at(-1)!.offset_ms - start;
  if (duration < 0) return [];
  const out: string[] = [];
  let path = '';
  for (let i = 0; i < values.length; i++) {
    const v = values[i], p = series.points[i]!;
    const dt = i ? p.offset_ms - series.points[i - 1]!.offset_ms : 0;
    if (i && (dt <= 0 || dt > series.max_gap_ms) && path) { out.push(path); path = ''; }
    if (v === null || v === undefined) { if (path) out.push(path); path = ''; continue; }
    const x = duration ? 2 + 96 * (p.offset_ms - start) / duration : 50;
    const y = range ? 38 - 36 * delta(v, low) / range : 20;
    path += `${path ? ' L' : 'M'}${+x.toFixed(3)},${+y.toFixed(3)}`;
  }
  if (path) out.push(path);
  return out;
}

export function trendLabel(v: TrendValue | null, unit: TrendUnit): string {
  const n = reading(v, unit);
  if (n === null) return 'unknown';
  if (unit === 'bytes') return formatBytes(String(n));
  if (unit === 'fraction') return `${(Number(n) * 100).toFixed(1)}%`;
  if (unit === 'queue') return `${n} queued`;
  return `${formatBytes(String(Math.round(Number(n))))}/s`;
}

/**
 * States the age of a series' last point when it trails the sample it is shown beside (the feed carries the process series
 * between its 15 s polls, and the engine samples them no faster than every 15 s), so an older line never passes for a
 * live one. This is information, not a fault: within one sampler interval (plus slack) it stays silent.
 */
export function trendAge(series: TrendSeries, sampleAt: string): string | null {
  const last = series.points.at(-1);
  const sample = Date.parse(sampleAt);
  if (!last || !Number.isFinite(sample)) return null;
  const lag = sample - Date.parse(last.observed_at);
  if (!Number.isFinite(lag)) return 'The age of the last point is unknown';
  if (lag < 2000) return null;
  const s = Math.floor(lag / 1000);
  return `Last point observed ${s < 60 ? `${s} s` : s < 3600 ? `${Math.floor(s / 60)} min` : `${Math.floor(s / 3600)} h`} before this sample`;
}
