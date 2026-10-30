import { describe, expect, it } from 'vitest';
import type { Volume } from '../../src/lib/contracts.gen';
import { diskHealthLine, volumeMarkers } from '../../src/lib/format/volume';
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
    expect(line).toBe('Disk health: counters reported, not a verdict (34 °C, 3% wear, 0 read errors, 0 write errors, 1200 h powered on)');
    expect(line).not.toMatch(NEVER_HEALTHY);
    const sparse: Volume = { ...reported, storage_path: { ...reported.storage_path!, health: { state: 'counters_present', reason: null, counters: { ...reported.storage_path!.health.counters!, temperature_celsius: null, wear_percent: null, read_errors_total: null, write_errors_total: null, power_on_hours: null } } } };
    expect(diskHealthLine(sparse)).toBe('Disk health: counters reported, not a verdict');
  });

  it('an offline mock volume is not queried and so reads unknown', async () => {
    const offline = (await load()).find((v) => !v.online)!;
    expect(offline.storage_path).toBeUndefined();
    expect(diskHealthLine(offline)).toBe('Disk health: unknown (not observed)');
  });
});
