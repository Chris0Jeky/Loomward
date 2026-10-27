import type { GroupKind, ReclaimPlan, Volume } from '../../lib/contracts.gen';
import { formatBytes, parseBytes } from '../../lib/format/bytes';

export const GROUP_HEADINGS: Record<GroupKind, string> = {
  tool_owned_cleanup: 'Tool-owned cleanup',
  relocate: 'Relocate',
  archive_candidates: 'Archive candidates',
  informational: 'Informational',
};

/** Prefer the least known free space; an unknown observation never becomes zero. */
export function defaultVolume(volumes: Volume[]): string {
  let selected = volumes[0];
  for (const volume of volumes) {
    const free = parseBytes(volume.free_bytes);
    const current = parseBytes(selected?.free_bytes);
    if (free !== null && (current === null || free < current)) selected = volume;
  }
  return selected?.volume_id ?? '';
}

export function unaccountedText(report: ReclaimPlan): string {
  return report.unaccounted_bytes === null
    ? 'Unaccounted: unknown — coverage or volume measurements are partial or unknown.'
    : `Unaccounted: ${formatBytes(report.unaccounted_bytes)} of volume used is outside the covered roots.`;
}
