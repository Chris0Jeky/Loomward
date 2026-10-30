import type { GroupKind, Job, ReclaimObserveProgress, ReclaimPlan, ReclaimRelief, ReclaimSelection, Volume } from '../../lib/contracts.gen';
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
  'Observing reads only metadata (link counts) of the folders you granted to record an allocation estimate. It can take minutes and changes nothing. The estimate does not guarantee physical space recovery.';

export function targetBytes(text: string): bigint | null {
  const bytes = parseBytes(text);
  return bytes !== null && bytes <= 18446744073709551615n ? bytes : null;
}

export function selectionComparison(result: ReclaimSelection | null, target: string): string {
  const need = targetBytes(target);
  if (need === null) return 'Enter a whole number of bytes from 0 to 18446744073709551615.';
  const bytes = result?.status === 'recorded' ? parseBytes(result.reclaimable_bytes) : null;
  if (bytes === null) return 'Target coverage is unknown.';
  return bytes >= need
    ? `The recorded estimate covers the target, with ${(bytes - need).toString()} bytes above it.`
    : `The recorded estimate is ${(need - bytes).toString()} bytes short of the target.`;
}

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
  // Finished but not ours: its counts may describe another volume, so state only that it ended.
  if (!owned) return "An observation this page did not start has ended; it may have covered another volume. The plan below is this volume's.";
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

export const RELIEF_EXPLAINER =
  'Recording a sample reads only this volume\'s free and total space and, where Loomward may read it, the Recycle Bin size. It changes no file. Recording a new before sample replaces the earlier check. The recorded selection estimate (the union estimate of the current selection), if there is one, is kept with a before sample as this app supplied it.';

/** The plan's estimate to keep with a before sample: only a recorded, known selection estimate; otherwise none. */
export function reliefEstimate(selection: ReclaimSelection | null): string | undefined {
  return selection?.status === 'recorded' && parseBytes(selection.reclaimable_bytes) !== null ? selection.reclaimable_bytes! : undefined;
}

/** Plain words for a refused or failed sample request. Nothing is recorded on a refusal. */
export function sampleRefusal(e: unknown): string {
  if (!(e instanceof LoomwardError)) return e instanceof Error ? e.message : 'unexpected failure';
  if (e.code === 'device_offline' && e.detail?.reason === 'volume_unreadable') return 'The free space of this volume could not be read just now, so the earlier before sample was kept. Try again. Nothing was recorded.';
  if (e.code === 'device_offline') return 'This volume is offline, or another disk is using its drive letter. Nothing was recorded.';
  if (e.code === 'invalid_request' && (e.detail?.reason === 'no_before_sample' || e.message.includes('before sample'))) return 'Record a before sample first. Nothing was recorded.';
  if (e.code === 'invalid_request' && (e.message === 'no_active_grant' || e.detail?.reason === 'no_active_grant')) {
    return 'No folder on this volume has an active grant, so Loomward may not look at this volume. Nothing was recorded.';
  }
  if (e.code === 'not_found') return 'This volume is no longer listed. Use Look again.';
  return e.message;
}

const binText = (bytes: string | null) => (bytes === null ? 'unknown (not read)' : formatBytes(bytes));

/** What the owner is told about a relief check. It states an observed change and never credits it to the cleanup. */
export function reliefSummary(relief: ReclaimRelief): { headline: string; estimate: string; bin: string; caveat: string } {
  const change = relief.change_bytes === null ? '' : `${formatBytes(relief.change_bytes)} (${relief.change_bytes} bytes)`;
  const headline = {
    not_checked: 'Only the before sample is recorded. Clean up in the owning tool, then choose Check now.',
    different_volume: 'The two samples come from different volume identities, so they are not compared. Record a new before sample for this volume.',
    unreadable: 'Free space could not be read in one of the samples, so no change is reported. Unknown is not zero.',
    compared: relief.direction === 'unchanged' ? 'Free space did not change between the two samples.'
      : relief.direction === 'more_free' ? `Free space rose by ${change} between the two samples.`
      : `Free space fell by ${change} between the two samples.`,
  }[relief.status];
  const estimate = relief.before.estimate_bytes === null
    ? 'No recorded selection estimate was kept with the before sample.'
    : `The recorded selection estimate kept with the before sample was ${formatBytes(relief.before.estimate_bytes)} (${relief.before.estimate_bytes} bytes). The app supplied it; the service stored it as given and did not recheck it.`;
  const bin = `Recycle Bin: ${binText(relief.before.recycle_bin_bytes)} at the before sample${relief.after ? `, ${binText(relief.after.recycle_bin_bytes)} at the second` : ''}. Files sent to the Recycle Bin keep taking space on this volume until it is emptied.`;
  const spansCapacity = relief.capacity_changed === true ? 'The total size of the volume differs between the two samples, so this comparison spans a capacity change. ' : '';
  const caveat = spansCapacity + 'This is the whole volume\'s change between the samples, not proof of what a cleanup freed: other writes, an unemptied Recycle Bin, hard links elsewhere and restore points all affect it.';
  return { headline, estimate, bin, caveat };
}
