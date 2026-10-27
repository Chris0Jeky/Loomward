import type { GroupKind, Job, ReclaimObserveProgress, ReclaimPlan, Volume } from '../../lib/contracts.gen';
import { formatBytes, parseBytes } from '../../lib/format/bytes';
import { LoomwardError } from '../../lib/transport/client';

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

export const OBSERVE_EXPLAINER =
  'Observing reads only metadata (link counts) of the folders you granted, to measure what removing each candidate would really free. It can take minutes and changes nothing.';

/** Plain words for a refused or failed observe request. The service's own text is the fallback. */
export function observeRefusal(e: unknown): string {
  if (!(e instanceof LoomwardError)) return e instanceof Error ? e.message : 'unexpected failure';
  if (e.code === 'busy') return 'Another observation is already running. Wait for it to end, or cancel it.';
  if (e.code === 'permission_denied' && e.detail?.reason === 'elevated_token') {
    return 'Loomward is running with an administrator token, so it will not open files. Restart it normally and try again. Nothing was read.';
  }
  if (e.code === 'invalid_request' && (e.message === 'no_active_grant' || e.detail?.reason === 'no_active_grant')) {
    return 'No folder on this volume has an active grant, so there is nothing to observe. Grant a folder first. Nothing was read.';
  }
  if (e.code === 'not_found') return 'This volume is no longer listed. Use Look again.';
  if (e.code === 'capability_unavailable') return 'Observing is not available in this session.';
  return e.message;
}

export function observeProgressText(p: ReclaimObserveProgress | undefined): string {
  if (!p) return 'Starting';
  const names = `${p.names_checked.toLocaleString('en')} names checked`;
  return p.candidates_total === 0 ? `No candidates to observe · ${names}` : `${p.candidates_done.toLocaleString('en')} of ${p.candidates_total.toLocaleString('en')} candidates observed · ${names}`;
}

/** What the owner is told while a job runs and after it ends. An unfinished candidate stays unknown, never zero. */
export function observeStatus(job: Job | null, owned = true): string {
  if (!job) return '';
  const p = observeProgressText(job.reclaim);
  // The job names no volume: one this page did not start may be observing another, so never imply it is this one.
  if (!owned && ['queued', 'running', 'cancel_requested'].includes(job.state)) {
    return `An observation this page did not start is running, and the service does not say which volume it covers · ${p}. This volume's plan is read again when it ends.`;
  }
  switch (job.state) {
    case 'queued': return 'Observation queued.';
    case 'running': return `Observing hard links · ${p}`;
    case 'cancel_requested': return `Cancelling · ${p}`;
    case 'cancelled': return `Observation cancelled · ${p}. Candidates it did not finish stay unknown.`;
    case 'failed': return `Observation failed${job.error ? `: ${job.error.message}` : ''}. Candidates it did not finish stay unknown.`;
    case 'completed':
      return job.coverage === 'partial'
        ? `Observation stopped early · ${p}. Candidates it did not finish stay unknown.`
        : `Observation finished · ${p}.`;
  }
}

/** The two lines the view shows: the job's status whenever there is a job, and a fault or refusal as a separate line. */
export function observeLines(o: { job: Job | null; problem: string; pending: boolean; ownJobId: string | null }): { status: string; problem: string } {
  const status = o.job ? observeStatus(o.job, o.job.job_id === o.ownJobId) : o.pending ? 'Sending request…' : '';
  return { status, problem: o.problem };
}
