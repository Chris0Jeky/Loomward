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
