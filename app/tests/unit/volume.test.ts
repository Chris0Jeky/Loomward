import { describe, expect, it } from 'vitest';
import type { StorageCriticalWarning, Volume } from '../../src/lib/contracts.gen';
import { diskHealthLine, hasDiskHealthWarning, volumeMarkers } from '../../src/lib/format/volume';
import { volumes } from '../../src/lib/stores/volumes.svelte';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';

describe('volume markers', () => {
  it('marks USB even when it is offline and does not guess for unknown buses', async () => {
    const { result } = await new Client(createMockTransport()).call('volumes.list', {});
    const usb = result.volumes.find((v) => v.device.bus_type === 'usb')!;
    expect(volumeMarkers(usb)).toEqual(['External (USB), may be detached']);
    expect(volumeMarkers({ ...usb, online: false })).toEqual(['Offline', 'External (USB), may be detached']);
    const unknown: Volume = { ...usb, device: { ...usb.device, bus_type: 'unknown' } };
    expect(volumeMarkers(unknown)).toEqual([]);
    expect(volumeMarkers({ ...unknown, online: false })).toEqual(['Offline']);
    expect(volumeMarkers(null)).toEqual([]);
  });
});

describe('disk health line (LW-137)', () => {
  const load = async () => (await new Client(createMockTransport()).call('volumes.list', {})).result.volumes;
  const NEVER_HEALTHY = /(healthy|good|ok|fine|passed?)/i;

  it('reads unknown, never healthy, for hidden, unsupported and unobserved health', async () => {
    const volumes = await load();
    const usb = volumes.find((v) => v.device.bus_type === 'usb' && v.storage_path)!;
    expect(usb.storage_path!.health.state).toBe('unsupported_or_hidden');
    expect(diskHealthLine(usb)).toBe('Disk health: unknown (not reported through this USB bridge)');
    const { storage_path: _unobserved, ...bare } = usb;
    expect(diskHealthLine(bare as Volume)).toBe('Disk health: unknown (not observed)');
    const noReason: Volume = { ...usb, storage_path: { ...usb.storage_path!, health: { state: 'unsupported_or_hidden', reason: null, counters: null } } };
    expect(diskHealthLine(noReason)).toBe('Disk health: unknown (not reported)');
    // Even a malformed counters_present without counters is unknown, not healthy.
    const empty: Volume = { ...usb, storage_path: { ...usb.storage_path!, health: { state: 'counters_present', reason: null, counters: null } } };
    expect(diskHealthLine(empty)).toMatch(/^Disk health: unknown/);
    for (const v of [usb, bare as Volume, noReason, empty]) expect(diskHealthLine(v)).not.toMatch(NEVER_HEALTHY);
  });

  it('shows reported counters as evidence, omits unreported ones and gives no verdict', async () => {
    const reported = (await load()).find((v) => v.storage_path?.health.state === 'counters_present')!;
    const line = diskHealthLine(reported);
    expect(line).toBe('Disk health: counters reported, not a verdict (34 °C, 3% wear, 100% spare (threshold 10%), 0 read errors, 0 write errors, 0 media errors, 3 unsafe shutdowns, 1200 h powered on)');
    expect(line).not.toMatch(NEVER_HEALTHY);
    const sparse: Volume = { ...reported, storage_path: { ...reported.storage_path!, health: { state: 'counters_present', reason: null, counters: { ...reported.storage_path!.health.counters!, temperature_celsius: null, wear_percent: null, read_errors_total: null, write_errors_total: null, power_on_hours: null, available_spare_percent: null, available_spare_threshold_percent: null, media_errors: null, unsafe_shutdowns: null } } } };
    expect(diskHealthLine(sparse)).toBe('Disk health: counters reported, not a verdict');
    expect(hasDiskHealthWarning(reported)).toBe(false);
  });

  const withWarning = (volume: Volume, warning: Partial<StorageCriticalWarning> | null): Volume => ({
    ...volume,
    storage_path: {
      ...volume.storage_path!,
      health: {
        state: 'counters_present', reason: null,
        counters: {
          ...volume.storage_path!.health.counters!,
          critical_warning: warning && { available_spare_low: false, temperature: false, reliability_degraded: false, read_only: false, volatile_backup_failed: false, other_bits: 0, ...warning },
        },
      },
    },
  });

  it('leads with a latched critical warning, as the drive\'s report and never a verdict', async () => {
    const reported = (await load()).find((v) => v.storage_path?.health.state === 'counters_present')!;
    const warned = withWarning(reported, { available_spare_low: true });
    const line = diskHealthLine(warned);
    expect(line).toBe('Disk health: the drive reports a critical warning (available spare below threshold) — counters reported, not a verdict (34 °C, 3% wear, 100% spare (threshold 10%), 0 read errors, 0 write errors, 0 media errors, 3 unsafe shutdowns, 1200 h powered on)');
    expect(line).not.toMatch(NEVER_HEALTHY);
    expect(hasDiskHealthWarning(warned)).toBe(true);
    const many = diskHealthLine(withWarning(reported, { temperature: true, reliability_degraded: true, read_only: true, volatile_backup_failed: true, other_bits: 0xa0 }));
    expect(many).toContain('the drive reports a critical warning (temperature outside its threshold, reliability degraded, media placed in read-only mode, volatile memory backup failed, other warning bits set (0xA0))');
    expect(many).toContain('counters reported, not a verdict');
  });

  it('a clear or unreported warning byte neither warns nor reads as healthy', async () => {
    const reported = (await load()).find((v) => v.storage_path?.health.state === 'counters_present')!;
    for (const v of [reported, withWarning(reported, null)]) {
      expect(hasDiskHealthWarning(v)).toBe(false);
      expect(diskHealthLine(v)).toMatch(/^Disk health: counters reported, not a verdict/);
      expect(diskHealthLine(v)).not.toMatch(/critical warning|healthy/i);
    }
    // A warning on a hidden or unobserved drive cannot appear: there are no counters to carry it.
    const hidden = (await load()).find((v) => v.storage_path?.health.state === 'unsupported_or_hidden')!;
    expect(hasDiskHealthWarning(hidden)).toBe(false);
    expect(hasDiskHealthWarning({ ...reported, storage_path: undefined })).toBe(false);
  });

  it('an offline mock volume is not queried and so reads unknown', async () => {
    const offline = (await load()).find((v) => !v.online)!;
    expect(offline.storage_path).toBeUndefined();
    expect(diskHealthLine(offline)).toBe('Disk health: unknown (not observed)');
  });
});

describe('volumes.changed keeps observed storage paths', () => {
  it('merges the previous storage_path by volume_id until the next list, and never invents one', async () => {
    const listed = (await new Client(createMockTransport()).call('volumes.list', {})).result.volumes;
    const observed = listed.find((v) => v.storage_path)!;
    const stripped = listed.map(({ storage_path: _dropped, ...rest }) => rest as Volume);
    volumes.items = listed;
    volumes.changed(stripped);
    expect(volumes.items!.find((v) => v.volume_id === observed.volume_id)!.storage_path).toEqual(observed.storage_path);
    // A volume that was never observed stays unobserved, and a row's own storage_path wins.
    const never = listed.find((v) => !v.storage_path)!;
    expect(volumes.items!.find((v) => v.volume_id === never.volume_id)!.storage_path).toBeUndefined();
    const fresh = { ...observed.storage_path!, bus_type: 'sata' };
    volumes.changed(stripped.map((v) => (v.volume_id === observed.volume_id ? { ...v, storage_path: fresh } : v)));
    expect(volumes.items!.find((v) => v.volume_id === observed.volume_id)!.storage_path!.bus_type).toBe('sata');
    volumes.items = null;
  });
});
