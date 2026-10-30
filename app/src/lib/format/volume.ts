import type { StorageCriticalWarning, Volume } from '../contracts.gen';

export function volumeMarkers(volume: Volume | null | undefined): string[] {
  if (!volume) return [];
  return [
    ...(!volume.online ? ['Offline'] : []),
    ...(volume.device.bus_type === 'usb' ? ['External (USB), may be detached'] : []),
  ];
}

/** The warnings the drive itself latched in its NVMe Critical Warning byte, in plain words. */
function criticalWarnings(w: StorageCriticalWarning | null): string[] {
  if (!w) return [];
  return [
    w.available_spare_low ? 'available spare below threshold' : null,
    w.temperature ? 'temperature outside its threshold' : null,
    w.reliability_degraded ? 'reliability degraded' : null,
    w.read_only ? 'media placed in read-only mode' : null,
    w.volatile_backup_failed ? 'volatile memory backup failed' : null,
    w.other_bits !== 0 ? `other warning bits set (0x${w.other_bits.toString(16).toUpperCase()})` : null,
  ].filter((part) => part !== null);
}

/**
 * The storage-path health last observed for a volume, only while it is online: an offline volume is
 * not queried, so whatever it last held (a kept row, a stale warning) is not current and reads not observed.
 */
function currentHealth(volume: Volume) {
  return volume.online ? volume.storage_path?.health : undefined;
}

/** True when the volume's drive reported a critical warning. A clear or unreported byte is never a reading of health. */
export function hasDiskHealthWarning(volume: Volume): boolean {
  const health = currentHealth(volume);
  return health?.state === 'counters_present' && criticalWarnings(health.counters?.critical_warning ?? null).length > 0;
}

/**
 * The one "Disk health" line for a volume (LW-137). Only reported counters are shown, and as
 * evidence rather than a verdict: nothing here says healthy. A missing storage path, an unsupported
 * or hidden result (SMART behind a USB bridge) and any unreported counter all read as unknown. A
 * critical warning the drive latched leads the line, worded as the drive's report.
 */
export function diskHealthLine(volume: Volume): string {
  const health = currentHealth(volume);
  if (!health) return 'Disk health: unknown (not observed)';
  if (health.state !== 'counters_present' || !health.counters) {
    return `Disk health: unknown (${health.reason ?? 'not reported'})`;
  }
  const c = health.counters;
  const parts = [
    c.temperature_celsius !== null ? `${c.temperature_celsius} °C` : null,
    c.wear_percent !== null ? `${c.wear_percent}% wear` : null,
    c.available_spare_percent !== null
      ? `${c.available_spare_percent}% spare${c.available_spare_threshold_percent !== null ? ` (threshold ${c.available_spare_threshold_percent}%)` : ''}`
      : null,
    c.read_errors_total !== null ? `${c.read_errors_total} read errors` : null,
    c.write_errors_total !== null ? `${c.write_errors_total} write errors` : null,
    c.media_errors !== null ? `${c.media_errors} media errors` : null,
    c.unsafe_shutdowns !== null ? `${c.unsafe_shutdowns} unsafe shutdowns` : null,
    c.power_on_hours !== null ? `${c.power_on_hours} h powered on` : null,
  ].filter((part) => part !== null);
  const warnings = criticalWarnings(c.critical_warning);
  const lead = warnings.length ? `the drive reports a critical warning (${warnings.join(', ')}) — counters reported, not a verdict` : 'counters reported, not a verdict';
  return `Disk health: ${lead}${parts.length ? ` (${parts.join(', ')})` : ''}`;
}
