<script module lang="ts">
  import type { DuplicateReport } from '../../lib/contracts.gen';
  import { formatBytes, parseBytes } from '../../lib/format/bytes';

  export const meta = { title: 'Duplicates', order: 35 };

  export function sanitizeDuplicatesReport(report: DuplicateReport | undefined | null): {
    report: DuplicateReport | undefined;
    problem: string;
    withheldGroups: number;
    withheldLinks: number;
    unreadableSkipped: number;
    skippedUnknown: boolean;
  } {
    if (!report) return { report: undefined, problem: '', withheldGroups: 0, withheldLinks: 0, unreadableSkipped: 0, skippedUnknown: false };
    const count = Array.isArray(report.entries) ? report.entries.length : 0;
    const valid = (i: unknown): i is number =>
      typeof i === 'number' && Number.isInteger(i) && i >= 0 && i < count;
    let malformed = false;
    let unreadableSkipped = 0;
    // A verified group or a hard-link set is shown only whole: losing any member, or ending below its minimum (two for a group, one for a link set whose other names may lie outside the inspected set), withholds it.
    const cleanWhole = (lists: unknown, min: number): { sets: number[][]; withheld: number } => {
      if (!Array.isArray(lists)) {
        malformed = true;
        return { sets: [], withheld: 0 };
      }
      const sets: number[][] = [];
      let withheld = 0;
      for (const list of lists) {
        if (!Array.isArray(list)) {
          malformed = true;
          withheld += 1;
          continue;
        }
        const kept = (list as unknown[]).filter(valid);
        if (kept.length !== list.length || kept.length < min) {
          malformed = true;
          withheld += 1;
          continue;
        }
        sets.push(kept);
      }
      return { sets, withheld };
    };
    const { sets: groups, withheld: withheldGroups } = cleanWhole(report.groups, 2);
    const { sets: links, withheld: withheldLinks } = cleanWhole(report.links, 1);
    const skipped = Array.isArray(report.skipped)
      ? report.skipped.filter((skip) => {
          const ok = !!skip && valid(skip.entry);
          if (!ok) {
            malformed = true;
            unreadableSkipped += 1;
          }
          return ok;
        })
      : [];
    const skippedUnknown = !Array.isArray(report.skipped);
    const groupsUnknown = !Array.isArray(report.groups);
    const linksUnknown = !Array.isArray(report.links);
    if (!Array.isArray(report.groups) || !Array.isArray(report.links) || !Array.isArray(report.skipped)) {
      malformed = true;
    }
    let problem = '';
    if (malformed) {
      problem = 'Some inspection references were invalid and were skipped.';
      if (withheldGroups === 1) problem += ' 1 verified group was withheld as incomplete.';
      else if (withheldGroups > 1) problem += ` ${withheldGroups} verified groups were withheld as incomplete.`;
      if (groupsUnknown) problem += ' The verified groups could not be read, so they are unknown.';
      if (withheldLinks === 1) problem += ' 1 link set was withheld as incomplete.';
      else if (withheldLinks > 1) problem += ` ${withheldLinks} link sets were withheld as incomplete.`;
      if (linksUnknown) problem += ' The link sets could not be read, so they are unknown.';
    }
    return {
      report: { ...report, groups, links, skipped },
      problem,
      withheldGroups,
      withheldLinks,
      unreadableSkipped,
      skippedUnknown,
    };
  }

  const STOP_WHY: Record<string, string> = {
    entry_budget: 'the candidate cap was reached',
    byte_budget: 'the byte budget was spent',
    time_budget: 'the time budget ran out',
    cancelled: 'the inspection was cancelled',
  };

  /** Coverage wording: how many same-size candidate files this run examined out of the root's candidate space, and why the rest were not. A missing or malformed space is unknown, never zero (invariant 4). */
  export function candidateCoverage(report: DuplicateReport): { heading: string; detail: string } {
    const listed = Array.isArray(report.entries) ? report.entries.length : 0;
    const refused = new Set((Array.isArray(report.skipped) ? report.skipped : []).map((s) => s?.entry)).size;
    const examined = Math.max(0, listed - refused);
    const n = (v: number) => v.toLocaleString('en-US');
    const space = report.candidate_space;
    const files = space && Number.isSafeInteger(space.files) && space.files >= 0 ? space.files : null;
    const stop = report.stop;
    const why = stop ? (STOP_WHY[stop] ?? `the run stopped (${stop})`) : null;
    // A byte, time or cancel stop can end the run before every listed name was read (or even reached), so such a run
    // claims only what it listed, never what it examined (review of #1032).
    const early = stop === 'byte_budget' || stop === 'time_budget' || stop === 'cancelled';
    const verb = early ? 'Listed' : 'Examined';
    const count = early ? listed : examined;
    const parts: string[] = [];
    let heading: string;
    if (files === null) {
      heading = `${verb} ${n(count)} candidate files. The size of the candidate space is unknown, so how much of the root this covers is unknown.`;
    } else {
      const bytes = parseBytes(space?.bytes) === null ? 'unknown size' : formatBytes(space?.bytes);
      heading = `${verb} ${n(count)} of ${n(files)} candidate files (${bytes}).`;
      const rest = Math.max(0, files - count);
      if (rest > 0) parts.push(`${n(rest)} candidate ${rest === 1 ? 'file was' : 'files were'} not ${early ? 'listed' : 'examined'}${why ? `: ${why}` : ''}.`);
      // The catalogue leaves these names out of the total; a partial report is where they can exist.
      if (report.partial === true) parts.push('Unreadable or unrepresentable names are not in this count.');
    }
    if (early) parts.push('Some listed files may not have been read before the run stopped.');
    if (files === null && why) parts.push(`The run ended because ${why}.`);
    return { heading, detail: parts.join(' ') };
  }

  // Wording for skipped evidence that could not be read. Empty string means the list itself says enough.
  export function skippedNotice(unreadable: number, listed: number, unknown = false): string {
    if (unknown) return 'The skipped list could not be read, so what was skipped is unknown.';
    if (unreadable === 1) return '1 skipped record could not be read, so its reason is unknown.';
    if (unreadable > 1) return `${unreadable} skipped records could not be read, so their reasons are unknown.`;
    return listed > 0 ? '' : 'No listed entries were skipped.';
  }
</script>

<script lang="ts">
  import { tick } from 'svelte';
  import type { Root } from '../../lib/contracts.gen';
  import { session } from '../../lib/stores/session.svelte';
  import { Duplicates } from '../../lib/stores/duplicates.svelte';
  import { isActive } from '../../lib/scan/model';
  import { escapedName } from '../../lib/format/names';
  import VisibleName from '../../lib/ui/VisibleName.svelte';

  const duplicates = new Duplicates();
  let roots = $state<Root[]>([]);
  let selected = $state('');
  let loading = $state(true);
  let resultHeading = $state<HTMLElement>();
  const job = $derived(duplicates.book[selected]);
  const report = $derived(job?.duplicates);
  const guarded = $derived(sanitizeDuplicatesReport(report));
  const safeReport = $derived(guarded.report);
  const indexProblem = $derived(guarded.problem);
  const coverage = $derived(safeReport ? candidateCoverage(safeReport) : null);
  const skippedText = $derived(safeReport ? skippedNotice(guarded.unreadableSkipped, safeReport.skipped.length, guarded.skippedUnknown) : '');
  const active = $derived(job ? isActive(job.state) : false);
  // A session reads its own class of root only: lab roots when synthetic, folders you granted when personal (q-10).
  const origin = $derived(session.datasetClass === 'personal' ? 'owner_granted' : 'lab_generated');
  const root = $derived(roots.find((r) => r.root_id === selected));
  const status = $derived(duplicates.problem || (duplicates.pending ? 'Sending request…' : job ? `Inspection ${job.state.replace('_', ' ')}` : ''));
  const canStart = $derived(root?.grant_state === 'active' && root.origin === origin);

  $effect(() => session.listen((event) => duplicates.onEvent(event)));
  $effect(() => {
    const c = session.client;
    void session.epoch;
    let current = true;
    if (c) void (async () => {
      loading = true;
      try {
        const { result } = await c.call('roots.list', {});
        if (!current) return;
        roots = result.roots;
        if (!roots.some((r) => r.root_id === selected)) selected = roots.find((r) => r.grant_state === 'active' && r.origin === origin)?.root_id ?? roots[0]?.root_id ?? '';
        await duplicates.sync();
      } catch (e) { if (current) duplicates.problem = session.handle(e); }
      finally { if (current) loading = false; }
    })();
    return () => { current = false; };
  });
  $effect(() => {
    if (!job || !active) return;
    const observed = job;
    const timer = setInterval(() => void duplicates.refresh(observed), 1000);
    return () => clearInterval(timer);
  });
  let lastActive = false;
  $effect(() => {
    const finished = lastActive && !active;
    lastActive = active;
    if (finished) void tick().then(() => resultHeading?.focus());
  });
</script>

<h1>Duplicates</h1>
<p class="lede">Reads file contents on this machine to confirm duplicates; nothing is deleted or moved.</p>
<p class="note">One granted root at a time: same size, then partial hash, full hash and a byte-by-byte comparison. Up to 200 candidates, 64 MiB read and 10 seconds. Empty files, excluded folders and cloud placeholders are skipped; placeholders are never downloaded.</p>
<p class="note">Equal content is an observation, not a recommendation to delete.</p>

<section class="panel" aria-labelledby="duplicate-inspect">
  <h2 id="duplicate-inspect">Inspect a root</h2>
  <div class="controls">
    <div class="lw-field">
      <label class="lbl" for="duplicate-root">Root</label>
      <select id="duplicate-root" bind:value={selected} disabled={loading || active || duplicates.pending}>
        {#each roots as r (r.root_id)}<option value={r.root_id}>{escapedName(r.display_path.text)}</option>{/each}
      </select>
    </div>
    <button class="btn primary" disabled={loading || !canStart || active || duplicates.pending} onclick={() => duplicates.start(selected)}>Start inspection</button>
    <button class="btn" disabled={!active || job?.state === 'cancel_requested' || duplicates.pending} onclick={() => duplicates.cancel(selected)}>Cancel inspection</button>
  </div>
  {#if !loading && !roots.length}<p class="empty">No granted roots are available. <a href="#/health">Add a folder in Grants &amp; health</a>.</p>
  {:else if root && !canStart}<p class="empty">This root is unavailable for inspection. Use {session.datasetClass === 'personal' ? 'an active folder you granted' : 'an active registered synthetic lab root'}.</p>{/if}
  <p class="live" class:has={!!status} role="status" aria-live="polite">{status}</p>
</section>

<section class="panel" aria-labelledby="duplicate-results">
  <h2 id="duplicate-results" tabindex="-1" bind:this={resultHeading}>Inspection result</h2>
  {#if job?.error}<p class="bad" role="alert">{job.error.message}</p>{/if}
  {#if safeReport}
    {#if coverage}<p class="coverage"><strong>{coverage.heading}</strong>{#if coverage.detail} {coverage.detail}{/if}</p>{/if}
    <p class="evidence">Bytes read <strong class="num">{formatBytes(safeReport.bytes_read)}</strong> · Partial <strong class:warn={safeReport.partial}>{safeReport.partial ? 'yes' : 'no'}</strong> · Truncated <strong class:warn={safeReport.truncated}>{safeReport.truncated ? 'yes' : 'no'}</strong> · Stop <strong>{safeReport.stop ?? 'none'}</strong></p>
    {#if indexProblem}<p class="bad" role="alert">{indexProblem}</p>{/if}
    <h3>Verified groups</h3>
    {#if safeReport.groups.length}
      <div class="tbl-wrap"><table class="tbl"><caption class="sr-only">Verified equal default streams</caption>
        <thead><tr><th scope="col">Members</th><th scope="col">Logical size per file</th></tr></thead>
        <tbody>{#each safeReport.groups as group}<tr><th scope="row">{#each group as i}<div><VisibleName name={safeReport.entries[i]!.relative_path} /></div>{/each}</th><td class="num">{formatBytes(safeReport.entries[group[0]!]!.logical_bytes)}</td></tr>{/each}</tbody>
      </table></div>
    {:else}<p class="empty">No verified groups in this inspection. Unexamined content remains unknown.</p>{/if}
    <h3>Link sets</h3>
    <p class="muted">One object per set, with no space to reclaim. {safeReport.links_not_compared} listed names were not compared.</p>
    {#if safeReport.links.length}
      <div class="tbl-wrap"><table class="tbl"><caption class="sr-only">Names of the same native object</caption>
        <thead><tr><th scope="col">Names</th><th scope="col">Observation</th></tr></thead>
        <tbody>{#each safeReport.links as links}<tr><th scope="row">{#each links as i}<div><VisibleName name={safeReport.entries[i]!.relative_path} /></div>{/each}</th><td>One object · no space to reclaim</td></tr>{/each}</tbody>
      </table></div>
    {/if}
    <h3>Skipped entries</h3>
    {#if safeReport.skipped.length}
      <div class="tbl-wrap"><table class="tbl"><caption class="sr-only">Skipped entries and reasons</caption>
        <thead><tr><th scope="col">Name</th><th scope="col">Reason</th></tr></thead>
        <tbody>{#each safeReport.skipped as skip}<tr><th scope="row"><VisibleName name={safeReport.entries[skip.entry]!.relative_path} /></th><td>{skip.reason}</td></tr>{/each}</tbody>
      </table></div>
    {/if}
    {#if skippedText}<p class={safeReport.skipped.length ? 'muted' : 'empty'}>{skippedText}</p>{/if}
  {:else}<p class="empty">{active ? 'Inspection is running; verified evidence will appear when it ends.' : 'Start an inspection to see its evidence.'}</p>{/if}
</section>

<style>
  h3 { margin: var(--sp-5) 0 var(--sp-2); font-size: 1rem; }
  .evidence, .coverage { overflow-wrap: anywhere; }
  .evidence strong { font-family: var(--num-font); }
</style>
