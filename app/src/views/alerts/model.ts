import type { AlertItem, AlertSettings, AlertStatus } from '../../lib/contracts.gen';
import { formatBytes, parseBytes } from '../../lib/format/bytes';

const GIB = 1024n ** 3n;

/** What each status means, in words. Only `alert` is an active alert; the others say why none was raised. */
export const STATUS: Record<AlertStatus, { label: string; tone: 'bad' | 'ok' | 'unk' | 'warn'; why: string }> = {
  alert: { label: 'Alert', tone: 'bad', why: 'The measurement crossed the threshold.' },
  ok: { label: 'Within threshold', tone: 'ok', why: 'Measured, and under the threshold.' },
  unknown: { label: 'Not observed', tone: 'unk', why: 'Capacity or free space was not observed, so no alert is raised and none is implied.' },
  insufficient_history: { label: 'Not enough history', tone: 'unk', why: 'Fewer than two complete scans inside the window.' },
  incomplete_coverage: { label: 'Partial coverage', tone: 'warn', why: 'A scan in the window did not cover the whole root, so its size is not comparable.' },
};

/** The measurement in words: unknown stays unknown, and is never shown as 0. */
export function measured(i: AlertItem): string {
  if (i.measured_bytes === null) return 'unknown';
  if (i.kind === 'fast_growth') return `+${formatBytes(i.measured_bytes)}`;
  return `${formatBytes(i.measured_bytes)} free${i.capacity_bytes ? ` of ${formatBytes(i.capacity_bytes)}` : ''}`;
}

export function threshold(i: AlertItem): string {
  if (i.threshold_bytes === null) return 'none applied';
  return i.kind === 'low_space' ? `below ${formatBytes(i.threshold_bytes)}` : `${formatBytes(i.threshold_bytes)} or more`;
}

export const COVERAGE_WORDS: Record<string, string> = { complete: 'Complete', partial: 'Partial', unknown: 'Unknown' };

/** Bytes -> a GiB number for the form, exact to 2 decimals. */
export function toGib(bytes: string): string {
  const b = parseBytes(bytes);
  if (b === null) return '';
  const hundredths = (b * 100n + GIB / 2n) / GIB;
  const whole = hundredths / 100n;
  const frac = hundredths % 100n;
  return frac === 0n ? `${whole}` : `${whole}.${frac.toString().padStart(2, '0').replace(/0$/, '')}`;
}

/** A GiB number from the form -> a wire byte string, or null when it is not a non-negative number with at most 2 decimals. */
export function fromGib(text: string): string | null {
  const m = /^(\d{1,9})(?:\.(\d{1,2}))?$/.exec(text.trim());
  if (!m) return null;
  const hundredths = BigInt(m[1]!) * 100n + BigInt((m[2] ?? '').padEnd(2, '0') || '0');
  return String((hundredths * GIB) / 100n);
}

export interface SettingsForm {
  notifications: boolean;
  percent: string;
  freeGib: string;
  days: string;
  growthGib: string;
}

export function formFrom(s: AlertSettings): SettingsForm {
  return {
    notifications: s.notifications_enabled, percent: String(s.low_space_free_percent), freeGib: toGib(s.low_space_free_bytes),
    days: String(s.growth_window_days), growthGib: toGib(s.growth_bytes),
  };
}

/** The whole record the service replaces, or the first thing wrong with the form. */
export function settingsFrom(f: SettingsForm): { settings: AlertSettings } | { error: string; field: keyof SettingsForm } {
  const int = (t: string, min: number, max: number) => (/^\d{1,3}$/.test(t.trim()) && +t >= min && +t <= max ? +t : null);
  const percent = int(f.percent, 1, 50);
  if (percent === null) return { error: 'Use a whole percentage from 1 to 50.', field: 'percent' };
  const free = fromGib(f.freeGib);
  if (free === null) return { error: 'Use a size in GiB, such as 10 or 2.5 (0 for percentage only).', field: 'freeGib' };
  const days = int(f.days, 1, 90);
  if (days === null) return { error: 'Use a whole number of days from 1 to 90.', field: 'days' };
  const growth = fromGib(f.growthGib);
  if (growth === null) return { error: 'Use a size in GiB, such as 5 or 0.5.', field: 'growthGib' };
  return {
    settings: {
      notifications_enabled: f.notifications, low_space_free_percent: percent, low_space_free_bytes: free,
      growth_window_days: days, growth_bytes: growth,
    },
  };
}
