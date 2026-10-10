import type { NullableBytes } from '../contracts.gen';

const UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB', 'EiB'];

/** Wire byte string -> BigInt, or null when unknown or malformed. */
export function parseBytes(v: NullableBytes | undefined): bigint | null {
  return typeof v === 'string' && /^(0|[1-9][0-9]{0,19})$/.test(v) ? BigInt(v) : null;
}

/** Lossy number for layout only (ADR-V3-04). Never use it to display an exact value. */
export function bytesToNumber(v: NullableBytes | undefined): number | null {
  const b = parseBytes(v);
  return b === null ? null : Number(b);
}

/** Display string. Unknown stays "unknown": it is never rendered as zero (invariant 4). */
export function formatBytes(v: NullableBytes | undefined): string {
  const b = parseBytes(v);
  if (b === null) return 'unknown';
  if (b < 1024n) return `${b} B`;
  let i = 0;
  let n = b;
  while (n >= 1024n && i < UNITS.length - 1) {
    n /= 1024n;
    i++;
  }
  const scaled = Number(b) / 1024 ** i;
  return `${scaled >= 100 ? scaled.toFixed(0) : scaled >= 10 ? scaled.toFixed(1) : scaled.toFixed(2)} ${UNITS[i]}`;
}
