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

/** How long ago a stored observation was made, in the largest whole unit. Unparsable or future stays unknown; never overstates freshness. */
export function observationAge(at: string, now: number): string {
  const elapsed = now - Date.parse(at);
  if (Number.isNaN(elapsed)) return 'at an unknown time';
  if (elapsed < 0) return 'at an unknown time (clock ahead)'; // never overstate freshness
  const seconds = Math.floor(elapsed / 1000);
  if (seconds < 60) return 'less than a minute ago';
  const unit = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'} ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return unit(minutes, 'minute');
  const hours = Math.floor(minutes / 60);
  return hours < 48 ? unit(hours, 'hour') : unit(Math.floor(hours / 24), 'day');
}
