import type { Volume } from '../contracts.gen';

export function volumeMarkers(volume: Volume | null | undefined): string[] {
  if (!volume) return [];
  return [
    ...(!volume.online ? ['Offline'] : []),
    ...(volume.device.bus_type === 'usb' ? ['External (USB), may be detached'] : []),
  ];
}

/**
 * The one "Disk health" line for a volume (LW-137). Only reported counters are shown, and as
 * evidence rather than a verdict: nothing here says healthy. A missing storage path, an unsupported
 * or hidden result (SMART behind a USB bridge) and any unreported counter all read as unknown.
 */
export function diskHealthLine(volume: Volume): string {
  const health = volume.storage_path?.health;
  if (!health) return 'Disk health: unknown (not observed)';
  if (health.state !== 'counters_present' || !health.counters) {
    return `Disk health: unknown (${health.reason ?? 'not reported'})`;
  }
  const c = health.counters;
  const parts = [
    c.temperature_celsius !== null ? `${c.temperature_celsius} °C` : null,
    c.wear_percent !== null ? `${c.wear_percent}% wear` : null,
    c.read_errors_total !== null ? `${c.read_errors_total} read errors` : null,
    c.write_errors_total !== null ? `${c.write_errors_total} write errors` : null,
    c.power_on_hours !== null ? `${c.power_on_hours} h powered on` : null,
  ].filter((part) => part !== null);
  return `Disk health: counters reported, not a verdict${parts.length ? ` (${parts.join(', ')})` : ''}`;
}
