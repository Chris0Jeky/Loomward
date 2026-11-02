import type {
  BackupEvidence, BackupImportRefusal, BackupRefusal, BackupTopology, BackupTopologyBasis, RequiredVersion,
} from '../../lib/contracts.gen';

/** Every evidence refusal in plain words. Unknown stays visible; nothing here grants anything. */
export const REFUSAL_WORDS: Record<BackupRefusal, string> = {
  snapshot_missing: 'The snapshot named for this repository is not in the imported list.',
  version_not_covered: 'No snapshot manifest or restore shows the exact file versions you need.',
  target_not_independent: 'The backup is not shown to live on a separate device from the source.',
  credentials_unavailable: 'Nothing shows the repository password or key still works.',
  integrity_not_checked: 'No clean integrity check that read all the data.',
  restore_not_succeeded: 'No restore drill has been observed.',
  partial_restore: 'No complete restore of every needed file.',
  unsupported_metadata: 'Permissions and timestamps were not compared after a restore.',
  stale_observation: 'An observation is older than the freshness window.',
  invalid_observation_time: 'An observation is dated in the future, so its age is unknown.',
};

export const IMPORT_REFUSAL_WORDS: Record<BackupImportRefusal, string> = {
  synthetic_session: 'A synthetic session cannot import a personal receipt.',
  not_a_regular_file: 'That pick is not a regular local file (network, device, link or cloud placeholder), so it was not opened.',
  unreadable: 'That file could not be read.',
  too_large: 'That file is larger than 256 KiB. Export only the one snapshot: restic snapshots <id> --json.',
  invalid_json: 'That file is not valid JSON.',
  invalid_receipt: 'That file is not a restic snapshots --json list.',
  invalid_source: 'A file name, version or label in the form was refused.',
  repository_limit: 'Loomward keeps evidence for at most 64 repositories. Forget one first.',
};

/** Which observation a refusal explains; the rest are recovery conditions that belong to none. */
const GROUP: Record<BackupRefusal, 'snapshot' | 'integrity' | 'restore' | 'conditions'> = {
  snapshot_missing: 'snapshot',
  integrity_not_checked: 'integrity',
  restore_not_succeeded: 'restore', partial_restore: 'restore', version_not_covered: 'restore', unsupported_metadata: 'restore',
  target_not_independent: 'conditions', credentials_unavailable: 'conditions', stale_observation: 'conditions', invalid_observation_time: 'conditions',
};

export interface Observation {
  name: string;
  /** Plain status: never a bare tick. */
  status: string;
  good: boolean;
  why: string[];
}

export function observations(e: BackupEvidence): Observation[] {
  const why = (g: string) => e.refusals.filter((r) => GROUP[r] === g).map((r) => REFUSAL_WORDS[r]);
  return [
    { name: 'Snapshot taken', good: e.snapshot_taken, why: why('snapshot'),
      status: e.snapshot_taken ? `Yes${e.snapshot_time ? `, provider time ${e.snapshot_time}` : ''}` : 'Not found' },
    { name: 'Integrity checked', good: e.integrity_checked && e.integrity_read_all_data, why: why('integrity'),
      status: !e.integrity_checked ? 'No clean check imported'
        : e.integrity_read_all_data ? 'Yes, all data read' : 'Structure only, or full data read not shown' },
    { name: 'Restore drill', good: e.restore_drill_succeeded, why: why('restore'),
      status: e.restore_drill_succeeded ? 'Succeeded for the listed versions' : 'Not run or not observed' },
  ];
}

export function conditions(e: BackupEvidence): string[] {
  return e.refusals.filter((r) => GROUP[r] === 'conditions').map((r) => REFUSAL_WORDS[r]);
}

const TOPOLOGY: Record<BackupTopology, string> = {
  independent_target: 'Separate device',
  same_volume: 'Same volume as the source',
  same_physical_device: 'Same physical disk as the source',
  unknown: 'Unknown',
};
const BASIS: Record<BackupTopologyBasis, string> = {
  same_volume_identity: 'Both sides are the same volume by its identity.',
  different_volumes_device_unknown: 'Different volumes; Loomward cannot yet tell whether they share a physical disk.',
  undeclared: 'The source or backup volume was not declared.',
};
export function topology(e: BackupEvidence): { label: string; basis: string } {
  return { label: TOPOLOGY[e.topology], basis: BASIS[e.topology_basis] };
}

/** Seconds as a short age; null (a future or missing observation) stays "unknown", never 0. */
export function formatAge(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds) || seconds < 0) return 'unknown';
  if (seconds < 3600) return `${Math.floor(seconds / 60)} min`;
  if (seconds < 172_800) return `${Math.floor(seconds / 3600)} h`;
  return `${Math.floor(seconds / 86_400)} days`;
}

export function coverage(e: BackupEvidence): string {
  return `${e.restore_coverage.length} of ${e.required_versions.length} needed versions covered by a restore`;
}

/** A needed name is relative: no root, no drive letter and no `..` segment, whichever separator it uses. */
const relativeName = (name: string): boolean => !/^([\\/]|[A-Za-z]:)/.test(name) && !name.split(/[\\/]/).includes('..');

/**
 * One needed file per line: `relative/name sha256` (the hash may carry a `sha256:` prefix).
 * Returns the lines it refused, by number, instead of guessing.
 */
export function parseRequired(text: string): { required: RequiredVersion[]; errors: string[] } {
  const required: RequiredVersion[] = [];
  const errors: string[] = [];
  text.split(/\r?\n/).forEach((raw, i) => {
    const line = raw.trim();
    if (!line) return;
    const m = /^(.+?)\s+(?:sha256:)?([0-9a-fA-F]{64})$/.exec(line);
    if (!m || !relativeName(m[1]!) || [...m[1]!].length > 1024) {
      errors.push(`Line ${i + 1}: expected a relative name, a space and a 64-digit SHA-256.`);
      return;
    }
    required.push({ relative_name: m[1]!, version: `sha256:${m[2]!.toLowerCase()}` });
  });
  if (!required.length && !errors.length) errors.push('List at least one file you need back.');
  if (required.length > 64) errors.push('At most 64 files per repository.');
  return { required, errors };
}

/** The line editor must preserve stored names and versions exactly, including whitespace. */
export function formatRequired(required: readonly RequiredVersion[]): string | null {
  const text = required.map((f) => `${f.relative_name} ${f.version}`).join('\n');
  const parsed = parseRequired(text);
  return !parsed.errors.length && parsed.required.length === required.length
    && parsed.required.every((f, i) => f.relative_name === required[i]!.relative_name && f.version === required[i]!.version)
    ? text : null;
}

export const LABEL = /^[A-Za-z0-9._-]{1,64}$/;
export const SNAPSHOT = /^[0-9a-f]{64}$/;
