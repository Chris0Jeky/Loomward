import { parseBytes } from '../../lib/format/bytes';

export type ReserveUnit = 'GiB' | 'B';
export interface ReserveDraft { value: string; unit: ReserveUnit }
const GIB = 1n << 30n;
const MAX_BYTES = (1n << 64n) - 1n;

export function reserveBytes(value: string, unit: ReserveUnit): string | null {
  const amount = parseBytes(value);
  if (amount === null) return null;
  const bytes = amount * (unit === 'GiB' ? GIB : 1n);
  return bytes <= MAX_BYTES ? bytes.toString() : null;
}

export function reserveDraft(bytes: string | null | undefined): ReserveDraft {
  const value = parseBytes(bytes);
  if (value === null) return { value: '', unit: 'GiB' };
  return value % GIB === 0n
    ? { value: (value / GIB).toString(), unit: 'GiB' }
    : { value: value.toString(), unit: 'B' };
}
