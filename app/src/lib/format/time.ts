/** RFC 3339 -> short UTC display; null or unparsable stays "unknown". */
export function formatTime(v: string | null | undefined): string {
  if (!v) return 'unknown';
  const t = Date.parse(v);
  return Number.isNaN(t) ? 'unknown' : new Date(t).toISOString().slice(0, 16).replace('T', ' ') + ' UTC';
}

/** Whole count with grouping; null is "unknown", not 0. */
export function formatCount(v: number | null | undefined): string {
  return typeof v === 'number' ? v.toLocaleString('en-GB') : 'unknown';
}

/** Elapsed time for a scan: whole seconds, then minutes; null or negative is "unknown", not 0. */
export function formatDuration(v: number | null | undefined): string {
  if (typeof v !== 'number' || !Number.isFinite(v) || v < 0) return 'unknown';
  const s = Math.floor(v / 1000);
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  return m < 60 ? `${m} min ${s % 60} s` : `${Math.floor(m / 60)} h ${m % 60} min`;
}
