import type { AlertItem, AlertReport, AlertSettings, GroupKind, Job, JobKind, ReclaimPlan, Root, Volume } from '../../lib/contracts.gen';
import { formatBytes, parseBytes } from '../../lib/format/bytes';
import { diskHealthLine, hasDiskHealthWarning } from '../../lib/format/volume';
import { measured, threshold } from '../alerts/model';

/**
 * Today is assembled from commands other views already call. Every source can be missing on its own
 * (null): a missing source is named, never read as "nothing there" (invariant 4).
 */
export interface Sources {
  volumes: Volume[] | null;
  roots: Root[] | null;
  alerts: AlertReport | null;
  jobs: Job[] | null;
  /** reclaim.plan per nearly full volume with a granted folder; null is a plan that could not be read. */
  reclaim: Map<string, ReclaimPlan | null>;
}

const MINUTE = 60_000;

/**
 * "5 min ago". A missing or unparsable time, and a time ahead of this clock (clock skew), are
 * "at an unknown time" (the clock-ahead form says so): never "just now" and never a fresh age (invariant 4).
 * Same wording and rule as reclaim/model.ts observationAge.
 */
export function ageText(at: string | null | undefined, now: number): string {
  const t = at ? Date.parse(at) : NaN;
  if (Number.isNaN(t)) return 'at an unknown time';
  const elapsed = now - t;
  if (elapsed < 0) return 'at an unknown time (clock ahead)';
  const m = Math.floor(elapsed / MINUTE);
  if (m < 1) return 'just now';
  if (m < 60) return `${m} min ago`;
  const h = Math.floor(m / 60);
  if (h < 48) return `${h} h ago`;
  return `${Math.floor(h / 24)} d ago`;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** Fraction of capacity in use, or null when either figure is unknown or they contradict each other. */
export function usedFraction(v: Pick<Volume, 'capacity_bytes' | 'free_bytes'>): number | null {
  const cap = parseBytes(v.capacity_bytes);
  const free = parseBytes(v.free_bytes);
  if (cap === null || free === null || cap === 0n || free > cap) return null;
  return Number(((cap - free) * 10000n) / cap) / 10000;
}

export interface VolumeRow {
  id: string;
  name: string;
  online: boolean;
  used: number | null;
  /** "212 GiB free of 2.00 TiB", or what is unknown; "Last observed: ..." for an offline volume. */
  space: string;
  tier: string;
  /** Last scan of the granted folders on this volume, in words. */
  scan: string;
  /** The "Disk health" line for this volume (see healthReading). */
  health: string;
  /** True when the health line is unknown: not observed, hidden behind a bridge, or not reported. Never a healthy reading. */
  healthUnknown: boolean;
  /** True when the drive itself reported a critical warning; shown prominently, still as the drive's report. */
  healthWarning: boolean;
  /** Everything not known or not current about the row, said plainly. */
  unknowns: string[];
}

export function tierText(v: Volume): string {
  return v.tier.tier === null ? 'Tier unknown' : `Tier ${v.tier.tier} · ${v.tier.basis === 'declared' ? 'declared' : 'hinted'}`;
}

/**
 * The Disk health line for a Today row: diskHealthLine() unchanged. An offline volume was not queried (its
 * storage path is not read), so it reads not observed whatever it last held. Unknown is never healthy (invariant 4).
 */
export function healthReading(v: Volume): { text: string; unknown: boolean; warning: boolean } {
  const queried: Volume = v.online ? v : { ...v, storage_path: undefined };
  const health = queried.storage_path?.health;
  const unknown = health?.state !== 'counters_present' || !health.counters;
  return { text: diskHealthLine(queried), unknown, warning: hasDiskHealthWarning(queried) };
}

const activeRoots = (roots: Root[]) => roots.filter((r) => r.grant_state === 'active');

export function volumeRows(volumes: Volume[], roots: Root[] | null, now: number): VolumeRow[] {
  return volumes.map((v) => {
    const unknowns: string[] = [];
    const free = parseBytes(v.free_bytes);
    const cap = parseBytes(v.capacity_bytes);
    if (!v.online) unknowns.push(`Offline: space figures are from the last observation, ${ageText(v.observed_at, now)}.`);
    if (free === null && cap === null) unknowns.push('Free space and capacity were not observed.');
    else if (free === null) unknowns.push('Free space was not observed.');
    else if (cap === null) unknowns.push('Capacity was not observed.');
    else if (usedFraction(v) === null) unknowns.push('Free space and capacity contradict each other, so no fill level is drawn.');
    if (v.tier.tier === null) unknowns.push('No tier is declared or hinted.');

    let scan: string;
    if (roots === null) {
      scan = 'unknown';
      unknowns.push('Granted folders could not be read, so scan age is unknown.');
    } else {
      const mine = activeRoots(roots).filter((r) => r.volume_id === v.volume_id);
      const done = mine.flatMap((r) => (r.scan.finished_at ? [r.scan.finished_at] : []));
      const newest = done.reduce<string | null>((a, b) => (a === null || Date.parse(b) > Date.parse(a) ? b : a), null);
      if (!mine.length) scan = 'No granted folder';
      else if (newest === null) scan = 'Never scanned';
      else scan = ageText(newest, now);
      if (mine.length && done.length < mine.length && newest !== null) unknowns.push(`${plural(mine.length - done.length, 'granted folder')} not scanned yet.`);
      const partial = mine.filter((r) => r.scan.state === 'partial' || r.scan.coverage === 'partial').length;
      if (partial) unknowns.push(`${plural(partial, 'granted folder')} scanned only in part.`);
    }
    // An offline volume's figures are old: they are worded as such and draw no fill level (never shown as current).
    const space = `${formatBytes(v.free_bytes)} free of ${formatBytes(v.capacity_bytes)}`;
    const health = healthReading(v);
    return {
      id: v.volume_id, name: v.display_name, online: v.online, used: v.online ? usedFraction(v) : null,
      space: v.online ? space : `Last observed: ${space}`,
      tier: tierText(v), scan, health: health.text, healthUnknown: health.unknown, healthWarning: health.warning, unknowns,
    };
  });
}

/** Granted folders that sit on no listed volume: their space cannot be placed, and that is said. */
export function unplacedRoots(volumes: Volume[], roots: Root[]): number {
  return activeRoots(roots).filter((r) => r.volume_id === null || !volumes.some((v) => v.volume_id === r.volume_id)).length;
}

/** Below the larger of the percentage and the size, exactly as the alert is defined. Unknown is never "nearly full". */
export function nearlyFull(v: Volume, s: AlertSettings): boolean {
  const cap = parseBytes(v.capacity_bytes);
  const free = parseBytes(v.free_bytes);
  if (!v.online || cap === null || free === null || cap === 0n) return false;
  const byPercent = (cap * BigInt(s.low_space_free_percent)) / 100n;
  const bySize = parseBytes(s.low_space_free_bytes) ?? 0n;
  return free < (byPercent > bySize ? byPercent : bySize);
}

/** The volumes Today asks reclaim.plan about: nearly full, with an active granted folder (without one the plan is refused). */
export function reclaimTargets(volumes: Volume[], roots: Root[], alerts: AlertReport): string[] {
  return volumes.filter((v) => nearlyFull(v, alerts.settings) && activeRoots(roots).some((r) => r.volume_id === v.volume_id)).map((v) => v.volume_id);
}

export type Section = 'alerts' | 'roots' | 'jobs' | 'reclaim';

export interface Need {
  key: string;
  section: Section;
  /** A file or volume name from the machine: always rendered through VisibleName. */
  name: string | null;
  text: string;
  view: string;
  linkText: string;
}

export const SECTION_TITLES: Record<Section, string> = {
  alerts: 'Alerts', roots: 'Granted folders', jobs: 'Unfinished work', reclaim: 'Reclaim on a nearly full volume',
};

/** The human name of every view Today links to; a view with no entry falls back to its slug. */
const VIEW_NAMES: Record<string, string> = {
  alerts: 'Alerts', health: 'Grants & health', duplicates: 'Duplicates', reclaim: 'Reclaim', review: 'Learning review',
};
export const linkName = (view: string): string => VIEW_NAMES[view] ?? view;

/** Where "and N more" points: every distinct view among the items not shown (jobs of mixed kinds live in several views). */
export function moreViews(items: Need[], shown: number): string[] {
  return [...new Set(items.slice(shown).map((i) => i.view))];
}

export function rootNeed(r: Root): Need | null {
  if (r.grant_state === 'identity_changed') {
    return { key: `root-${r.root_id}`, section: 'roots', name: r.display_path.text, text: 'now looks like a different folder, so its grant no longer applies. Grant it again to use it.', view: 'health', linkText: 'Open Grants & health' };
  }
  if (r.grant_state !== 'active') return null;
  const why: Partial<Record<Root['scan']['state'], string>> = {
    never_scanned: 'has not been scanned yet.',
    stale: 'was scanned before the folder changed, so the numbers are out of date.',
    failed: 'failed its last scan.',
    partial: 'was only partly scanned, so its numbers are incomplete.',
  };
  // A finished scan whose coverage is not complete is not a clean result either (invariant 4).
  const incompleteCoverage = r.scan.state === 'complete' && r.scan.coverage !== 'complete'
    ? `finished, but its coverage is ${r.scan.coverage}, so its numbers may be incomplete.` : undefined;
  const text = why[r.scan.state] ?? incompleteCoverage;
  return text ? { key: `root-${r.root_id}`, section: 'roots', name: r.display_path.text, text, view: 'health', linkText: 'Rescan in Grants & health' } : null;
}

const JOB_WORDS: Record<JobKind, { what: string; view: string; link: string }> = {
  scan: { what: 'Scan', view: 'health', link: 'Open Grants & health' },
  duplicates: { what: 'Duplicate inspection', view: 'duplicates', link: 'Open Duplicates' },
  reclaim: { what: 'Reclaim observation', view: 'reclaim', link: 'Open Reclaim' },
  refit: { what: 'Model refit', view: 'review', link: 'Open Learning review' },
  teacher: { what: 'Teacher request', view: 'review', link: 'Open Learning review' },
};
const OPEN: Job['state'][] = ['queued', 'running', 'cancel_requested'];
const STATE_WORDS: Partial<Record<Job['state'], string>> = { queued: 'is queued', running: 'is running', cancel_requested: 'is being cancelled' };

export function jobNeed(j: Job, roots: Root[] | null, now: number): Need | null {
  if (!OPEN.includes(j.state)) return null;
  const w = JOB_WORDS[j.kind];
  const root = j.root_id ? roots?.find((r) => r.root_id === j.root_id) : undefined;
  const since = j.started_at ? `, started ${ageText(j.started_at, now)}` : '';
  const what = root ? w.what.toLowerCase() : w.what; // a named folder comes first: "<folder> scan is running"
  return { key: `job-${j.job_id}`, section: 'jobs', name: root ? root.display_path.text : null, text: `${what} ${STATE_WORDS[j.state]}${since}.`, view: w.view, linkText: w.link };
}

const GROUP_WORDS: Record<GroupKind, string> = {
  tool_owned_cleanup: 'Tool-owned cleanup', relocate: 'Relocate', archive_candidates: 'Archive candidates', informational: 'Informational',
};

/**
 * The group of candidates with the most known reclaimable bytes. Candidates whose reclaimable size is not yet
 * observed stay unknown, never zero; a group with nothing known is ranked by what it holds, and says so.
 */
export function largestGroup(plan: ReclaimPlan): { name: string; text: string } | null {
  const groups = plan.groups.filter((g) => g.kind !== 'informational' && g.candidates.length).map((g) => {
    let known = 0n;
    let held = 0n;
    let unknown = 0;
    let heldUnknown = 0; // a stale root's candidates carry no sizes at all: unknown, never zero
    for (const c of g.candidates) {
      const r = parseBytes(c.reclaimable_bytes);
      if (r === null) unknown++; else known += r;
      const h = parseBytes(c.allocated_bytes) ?? parseBytes(c.logical_bytes);
      if (h === null) heldUnknown++; else held += h;
    }
    return { g, known, held, unknown, heldUnknown };
  });
  if (!groups.length) return null;
  const best = groups.reduce((a, b) => (b.known > a.known || (b.known === a.known && b.held > a.held) ? b : a));
  const n = plural(best.g.candidates.length, 'candidate');
  const holds = best.heldUnknown === best.g.candidates.length ? 'their size is not observed either'
    : `the folders hold ${best.heldUnknown ? 'at least ' : ''}${formatBytes(String(best.held))}`;
  const size = best.unknown === best.g.candidates.length
    ? `reclaimable size not observed yet (${holds})`
    : `${formatBytes(String(best.known))} reclaimable${best.unknown ? `, ${best.unknown} not observed yet` : ''}`;
  const caveat = plan.partial || plan.truncated || plan.budget_hit ? ' The plan is partial.' : '';
  return { name: GROUP_WORDS[best.g.kind], text: `${n}, ${size}.${caveat}` };
}

/** The "needs you" items, grouped by section in display order. Sections with nothing in them are left out. */
export function needsYou(src: Sources, now: number): { section: Section; items: Need[] }[] {
  const items: Need[] = [];
  for (const [n, i] of (src.alerts?.items ?? []).entries()) {
    if (i.status === 'alert') {
      items.push({ key: `alert-${n}`, section: 'alerts', name: i.label, text: i.kind === 'fast_growth' ? `grew ${measured(i)} between complete scans, flagged at ${threshold(i)}.` : `has ${measured(i)}, ${threshold(i)}.`, view: 'alerts', linkText: 'Open Alerts' });
    }
  }
  const rootItems = (src.roots ?? []).flatMap((r) => rootNeed(r) ?? []);
  rootItems.sort((a, b) => Number(b.text.startsWith('now looks')) - Number(a.text.startsWith('now looks')));
  items.push(...rootItems);
  for (const j of src.jobs ?? []) { const n = jobNeed(j, src.roots, now); if (n) items.push(n); }
  for (const [id, plan] of src.reclaim) {
    const g = plan ? largestGroup(plan) : null;
    const vol = src.volumes?.find((v) => v.volume_id === id);
    if (g) items.push({ key: `reclaim-${id}`, section: 'reclaim', name: vol?.display_name ?? id, text: `largest group is ${g.name.toLowerCase()}, ${g.text}`, view: 'reclaim', linkText: 'Open Reclaim' });
    // An incomplete plan with no candidates is not a clean volume: keep its uncertainty visible.
    else if (plan && (plan.partial || plan.budget_hit || plan.truncated)) items.push({ key: `reclaim-${id}`, section: 'reclaim', name: vol?.display_name ?? id, text: 'reclaim plan is incomplete and found no candidates yet; that is not a clean result.', view: 'reclaim', linkText: 'Open Reclaim' });
  }
  const order: Section[] = ['alerts', 'roots', 'jobs', 'reclaim'];
  return order.map((section) => ({ section, items: items.filter((i) => i.section === section) })).filter((s) => s.items.length);
}

/** Alert statuses that say why no alert was raised: the threshold could not be evaluated, so the alert is not clean either. */
const NOT_EVALUATED: AlertItem['status'][] = ['unknown', 'insufficient_history', 'incomplete_coverage'];

/** How many alerts Loomward could not evaluate. They raise nothing in "needs you", so they are counted here instead. */
export function unevaluatedAlerts(alerts: AlertReport | null): number {
  return (alerts?.items ?? []).filter((i) => NOT_EVALUATED.includes(i.status)).length;
}

/** The sentence that counts unevaluated alerts and points at Alerts, or null when every alert was evaluated. */
export function unevaluatedLine(n: number): string | null {
  return n ? `${plural(n, 'alert')} could not be evaluated; see Alerts.` : null;
}

/** The empty "needs you" sentence: "Nothing needs you." only when nothing was missing and no alert went unevaluated. */
export function needsEmptyText(missingCount: number, unevaluatedCount: number): string {
  return missingCount || unevaluatedCount ? 'Nothing needs you in what was read.' : 'Nothing needs you.';
}

/** Sources that could not be read, by name. Empty means everything Today asked for arrived. */
export function missingSources(src: Sources, failedReclaim: number): string[] {
  const out: string[] = [];
  if (src.volumes === null) out.push('volumes');
  if (src.roots === null) out.push('granted folders');
  if (src.alerts === null) out.push('alerts');
  if (src.jobs === null) out.push('jobs');
  if (failedReclaim) out.push(failedReclaim === 1 ? 'one reclaim plan' : `${failedReclaim} reclaim plans`);
  return out;
}
