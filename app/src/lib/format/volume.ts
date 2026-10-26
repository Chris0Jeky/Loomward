import type { Volume } from '../contracts.gen';

export function volumeMarkers(volume: Volume | null | undefined): string[] {
  if (!volume) return [];
  return [
    ...(!volume.online ? ['Offline'] : []),
    ...(volume.device.bus_type === 'usb' ? ['External (USB), may be detached'] : []),
  ];
}
