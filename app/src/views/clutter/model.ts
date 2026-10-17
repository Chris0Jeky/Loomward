import type { ClutterTopic, DeveloperClutterCandidate, DeveloperClutterKind, DeveloperClutterProvider, DeveloperClutterReport } from '../../lib/contracts.gen';
import { formatBytes } from '../../lib/format/bytes';

export const KIND_WORDS: Record<DeveloperClutterKind, string> = {
  build_output: 'Build output',
  dependency_install: 'Installed dependencies',
  environment: 'Environment',
  bytecode_cache: 'Bytecode cache',
  build_cache: 'Build cache',
};

export const PROVIDER_WORDS: Record<DeveloperClutterProvider, string> = {
  cargo: 'Cargo (Rust)', npm: 'npm (Node)', python: 'Python', dotnet: '.NET', gradle: 'Gradle',
};

/** The parts of this surface no provider supplies yet: shown as not assessed, never as a value. */
export const TOPIC_WORDS: Record<ClutterTopic, string> = {
  regeneration_cost: 'Rebuild cost',
  model_host_lifecycle: 'Model files and hosts',
  stale_worktrees: 'Stale worktrees',
};

export const COVERAGE_WORDS: Record<string, string> = {
  complete: 'Complete', partial: 'Partial', stale: 'Stale', unscanned: 'Not scanned', denied: 'Denied', excluded: 'Excluded', cancelled: 'Cancelled', unknown: 'Unknown',
};

/** Observed logical size. Unknown stays the word "unknown": it is never rendered as 0 (invariant 4). */
export function sizeText(c: DeveloperClutterCandidate): string {
  return formatBytes(c.logical_bytes);
}

/** On-disk size, said only when it was observed; otherwise why there is none. */
export function allocatedText(c: DeveloperClutterCandidate): string {
  return c.allocated_bytes === null ? 'on-disk size unknown' : `${formatBytes(c.allocated_bytes)} on disk`;
}

export type Outcome = 'budget' | 'list' | 'none' | 'none-partial';

/**
 * What the report lets the page say. A report that hit its budget lists nothing and says nothing about the
 * folder; an empty list from a partial scan is "none found in what was scanned", not a clean result.
 */
export function outcome(r: DeveloperClutterReport): Outcome {
  if (r.budget_hit) return 'budget';
  if (r.candidates.length) return 'list';
  return r.partial ? 'none-partial' : 'none';
}

export function countText(r: DeveloperClutterReport): string {
  const n = r.candidates.length;
  return `${n}${r.truncated ? '+' : ''} ${n === 1 && !r.truncated ? 'folder' : 'folders'}`;
}
