import type { ProcessList, ProcessRow } from '../../lib/contracts.gen';
import { formatBytes, parseBytes } from '../../lib/format/bytes';

export type ProcessLens = 'cpu_desc' | 'private_desc';
export const PROCESS_LIMIT = 5;
export const PROCESS_REFRESH_MS = 15000;

function reading(row: ProcessRow, lens: ProcessLens): number | bigint | null {
  if (row.access === 'denied') return null;
  if (lens === 'private_desc') return parseBytes(row.private_commit_bytes);
  return row.cpu_fraction !== null && Number.isFinite(row.cpu_fraction) && row.cpu_fraction >= 0 && row.cpu_fraction <= 1 ? row.cpu_fraction : null;
}

export function stripRows(rows: ProcessRow[], lens: ProcessLens): ProcessRow[] {
  return [...rows].sort((a, b) => {
    const x = reading(a, lens), y = reading(b, lens);
    if (x === null || y === null) return x === y ? a.pid - b.pid : x === null ? 1 : -1;
    return x === y ? a.pid - b.pid : x > y ? -1 : 1;
  }).slice(0, PROCESS_LIMIT);
}

export function processValue(row: ProcessRow, lens: ProcessLens): string {
  const value = reading(row, lens);
  return value === null ? 'unknown' : lens === 'private_desc' ? formatBytes(row.private_commit_bytes) : `${(Number(value) * 100).toFixed(1)}%`;
}

export function observationAge(at: string, now: number): string {
  const elapsed = now - Date.parse(at);
  if (!Number.isFinite(elapsed)) return 'observation age unknown';
  if (elapsed < 0) return 'observation age unknown (clock ahead)';
  const seconds = Math.floor(elapsed / 1000);
  const [value, unit] = seconds < 60 ? [seconds, 's'] : seconds < 3600 ? [Math.floor(seconds / 60), 'min'] : seconds < 86400 ? [Math.floor(seconds / 3600), 'h'] : [Math.floor(seconds / 86400), 'd'];
  return `observed ${value} ${unit} ago`;
}

export function stripStatus(list: ProcessList | null, error: string): string {
  if (error === 'partial_coverage') return 'Partial coverage: processes were not sampled under the telemetry lease.';
  if (error) return 'Process observation unavailable; readings are unknown.';
  if (!list) return 'Waiting for process observations';
  if (!list.rows.length) return 'No processes reported in this observation.';
  return `Top ${Math.min(list.rows.length, PROCESS_LIMIT)} of ${list.observed_count} observed · ${list.denied_count} could not be read (unknown, not zero)${list.truncated ? ' · remaining processes not listed' : ''}`;
}
