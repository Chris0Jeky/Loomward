import { describe, expect, it } from 'vitest';
import type { Volume } from '../../src/lib/contracts.gen';
import { volumeMarkers } from '../../src/lib/format/volume';
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
