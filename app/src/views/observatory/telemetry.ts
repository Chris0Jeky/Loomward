// Pure helpers for the Observatory instruments (lane L10): unknowns stay null, never a made-up number.

/** A byte count sent as a decimal string, or null when it is missing or not one. */
export const byteCount = (s: string | null | undefined): number | null => (typeof s === 'string' && /^\d+$/.test(s) ? Number(s) : null);

/** VRAM in GiB against the adapter's own total. No total, or a total of 0 (no scale), is unknown (null), not a value. */
export function vramGiB(used: string | null | undefined, total: string | null | undefined): { value: number | null; totalGiB: number | null } {
  const u = byteCount(used), t = byteCount(total);
  const totalGiB = t !== null && t > 0 ? t / 2 ** 30 : null;
  return { value: u !== null && totalGiB !== null ? u / 2 ** 30 : null, totalGiB };
}

/** One unique key per disk: a repeated label gets " (2)", " (3)", ... so a keyed list and the sparkline map never collide. */
export function uniqueLabels(labels: string[]): string[] {
  const seen = new Set<string>();
  return labels.map((l) => {
    let out = l;
    for (let n = 2; seen.has(out); n++) out = `${l} (${n})`;
    seen.add(out);
    return out;
  });
}
