// Wording and shaping for the "Is this folder already in another?" panel (#1106). Pure functions, no DOM.
//
// Honesty rules (AGENTS.md invariants 4 and 9): a run with anything unknown is partial and never reads as a clean
// verdict in either direction; unknown sizes are "unknown", never zero; and nothing here says a file is safe to
// delete: covered means an equal-size, equal-content file was in B when read.
import type { CoverageReport, CoverageStop, Job } from '../../lib/contracts.gen';
import { formatBytes, parseBytes } from '../../lib/format/bytes';
import { LoomwardError } from '../../lib/transport/client';

export const COVERS_EXPLAINER =
  'Reads file contents on this machine to see whether every file in the first folder also exists, with identical contents, somewhere in the second, whatever the names and paths. It changes nothing.';

export const COVERS_CAVEAT =
  'Covered by content when read: an observation, not a receipt. It does not check dates, attributes or permissions, it does not make the second folder a backup (it may share a failing disk), and it is not permission to delete anything. Either folder can change afterwards.';

export const BYTE_CHOICES = [
  { label: '1 GiB', value: '1073741824' },
  { label: '8 GiB', value: '8589934592' },
  { label: '64 GiB', value: '68719476736' },
] as const;

export const SECOND_CHOICES = [
  { label: '1 minute', value: 60 },
  { label: '5 minutes', value: 300 },
  { label: '30 minutes', value: 1800 },
] as const;

const STOP_WORDS: Record<CoverageStop, string> = {
  budget: 'the read budget ran out',
  refused: 'some files or folders could not be read or proven',
  changed_during_read: 'a file changed while it was being read',
};

const REASON_WORDS: Record<string, string> = {
  byte_budget: 'byte budget spent',
  time_budget: 'time budget spent',
  entry_budget: 'too many entries for one check',
  changed_during_compare: 'changed while being read',
  changed_during_read: 'changed while being read',
  recheck_incomplete: 'could not be re-verified',
  absence_unproven: 'the other folder was not fully listed',
  sharing_violation: 'in use by another program',
  access_denied: 'access denied',
  reparse_point: 'a link or junction (not followed)',
  placeholder: 'a cloud placeholder (never downloaded)',
  root_not_observed: 'the folder itself could not be read',
};

export function reasonText(reason: string): string {
  return REASON_WORDS[reason] ?? reason.replace(/_/g, ' ');
}

/** Navigation only: the Explorer route that shows a file's catalogue row, or null when the catalogue has none. */
export function explorerLink(nodeId: string | null): string | null {
  return nodeId ? `#/explorer?node=${encodeURIComponent(nodeId)}` : null;
}

const n = (v: number) => v.toLocaleString('en-US');

/** Bytes of one verdict. A lower bound is said so; an unknown total is "unknown", never zero. */
export function bytesText(bytes: string, sizeUnknown: number): string {
  if (parseBytes(bytes) === null) return 'unknown';
  return sizeUnknown > 0 ? `at least ${formatBytes(bytes)} (${n(sizeUnknown)} ${sizeUnknown === 1 ? 'file' : 'files'} of unknown size)` : formatBytes(bytes);
}

export interface Headline {
  tone: 'complete' | 'partial';
  heading: string;
  detail: string;
}

export function headline(report: CoverageReport): Headline {
  const { covered, not_covered, unknown } = report.totals;
  const files = (count: number) => `${n(count)} ${count === 1 ? 'file' : 'files'}`;
  if (report.outcome === 'partial' || unknown.files > 0) {
    const why = report.reasons.map((r) => STOP_WORDS[r] ?? r).join('; ');
    return {
      tone: 'partial',
      heading: 'Partial result: this is not a complete answer.',
      detail: `${files(unknown.files)} could not be judged${why ? ` (${why})` : ''}. Of the rest, ${files(covered.files)} ${covered.files === 1 ? 'was' : 'were'} covered by content and ${files(not_covered.files)} ${not_covered.files === 1 ? 'was' : 'were'} not found in the second folder.`,
    };
  }
  if (not_covered.files > 0) {
    return {
      tone: 'complete',
      heading: `${files(not_covered.files)} of the first folder ${not_covered.files === 1 ? 'was' : 'were'} not found in the second.`,
      detail: `${bytesText(not_covered.bytes, not_covered.size_unknown)} without an equal-content file there. ${files(covered.files)} ${covered.files === 1 ? 'was' : 'were'} covered by content.`,
    };
  }
  return covered.files === 0
    ? { tone: 'complete', heading: 'The first folder held no files when it was read.', detail: 'Nothing was compared.' }
    : {
        tone: 'complete',
        heading: 'Every file of the first folder was covered by the second when read.',
        detail: `${files(covered.files)} (${bytesText(covered.bytes, covered.size_unknown)}) each had a file of equal size and content in the second folder.`,
      };
}

export interface TotalRow { label: string; files: string; bytes: string }

export function totalRows(report: CoverageReport): TotalRow[] {
  const row = (label: string, t: CoverageReport['totals']['covered']): TotalRow => ({ label, files: n(t.files), bytes: bytesText(t.bytes, t.size_unknown) });
  return [
    row('Covered by content', report.totals.covered),
    row('Not found in the second folder', report.totals.not_covered),
    row('Unknown', report.totals.unknown),
  ];
}

/** What a job without a report says. `now` is the clock, so the elapsed time is testable. */
export function statusText(job: Job | null, report: CoverageReport | null, pending: boolean, now: number, budgetSecs: number): string {
  if (!job) return pending ? 'Sending request…' : '';
  const elapsed = job.started_at ? Math.max(0, Math.round((now - Date.parse(job.started_at)) / 1000)) : 0;
  switch (job.state) {
    case 'queued': return 'Check queued.';
    case 'running': return `Reading both folders · ${n(elapsed)} s so far, at most ${n(budgetSecs)} s. The service reports no finer progress.`;
    case 'cancel_requested': return 'Cancelling. No verdict will be produced. The read may still be finishing in the background.';
    case 'cancelled': return 'Cancelled. No verdict was produced and nothing was changed. The read may still be finishing in the background; a new check starts once it has ended.';
    case 'failed': return `Failed${job.error ? `: ${job.error.message}` : ''}. No verdict was produced.`;
    case 'completed':
      return report ? '' : 'Finished, but its report is no longer available: it is kept for ten minutes and ends if either folder\'s grant ends. Run the check again.';
  }
}

/** Plain words for a refused or failed start. The service's own text is the fallback. */
export function coversRefusal(e: unknown): string {
  if (!(e instanceof LoomwardError)) return e instanceof Error ? e.message : 'unexpected failure';
  if (e.code === 'busy' && e.detail?.reason === 'read_still_finishing') return 'The previous check is still finishing its read in the background (cancelling does not stop a read at once). Try again in a moment.';
  if (e.code === 'busy') return 'Another coverage check is already running. Wait for it to end, or cancel it.';
  if (e.code === 'invalid_request' && e.detail?.reason === 'roots_must_differ') return 'Choose two different folders.';
  if (e.code === 'invalid_request' && e.detail?.reason === 'roots_overlap') return 'One folder lies inside the other. Choose two separate folders.';
  if (e.code === 'permission_denied') return `A folder cannot be read now: ${e.message}. Nothing was read.`;
  if (e.code === 'not_found') return 'One of these folders is no longer listed. Reload the list.';
  return e.message;
}
