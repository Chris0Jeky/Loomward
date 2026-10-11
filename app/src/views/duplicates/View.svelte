<script module lang="ts">
  export const meta = { title: 'Duplicates', order: 35 };
</script>

<script lang="ts">
  import { tick } from 'svelte';
  import type { Root } from '../../lib/contracts.gen';
  import { session } from '../../lib/stores/session.svelte';
  import { Duplicates } from '../../lib/stores/duplicates.svelte';
  import { isActive } from '../../lib/scan/model';
  import { formatBytes } from '../../lib/format/bytes';
  import VisibleName from '../../lib/ui/VisibleName.svelte';

  const duplicates = new Duplicates();
  let roots = $state<Root[]>([]);
  let selected = $state('');
  let loading = $state(true);
  let resultHeading = $state<HTMLElement>();
  const job = $derived(duplicates.book[selected]);
  const report = $derived(job?.duplicates);
  const active = $derived(job ? isActive(job.state) : false);
  const personal = $derived(session.datasetClass === 'personal' || duplicates.gated);
  const root = $derived(roots.find((r) => r.root_id === selected));
  const status = $derived(duplicates.problem || (duplicates.pending ? 'Sending request…' : job ? `Inspection ${job.state.replace('_', ' ')}` : ''));
  const canStart = $derived(!personal && root?.grant_state === 'active' && root.origin === 'lab_generated');

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
        if (!roots.some((r) => r.root_id === selected)) selected = roots.find((r) => r.grant_state === 'active' && r.origin === 'lab_generated')?.root_id ?? roots[0]?.root_id ?? '';
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
<p class="lede">Inspect default streams in one granted synthetic root. Up to 200 candidates, 64 MiB read and 10 seconds. Empty files are excluded.</p>
<p class="note">Equal content is an observation, not a recommendation to delete.</p>

<section class="panel" aria-labelledby="duplicate-inspect">
  <h2 id="duplicate-inspect">Inspect a root</h2>
  {#if personal}
    <p class="note gated" role="status">Personal content inspection is gated. No content is opened.</p>
  {:else}
    <div class="controls">
      <div class="ctl">
        <label class="lbl" for="duplicate-root">Root</label>
        <select id="duplicate-root" bind:value={selected} disabled={loading || active || duplicates.pending}>
          {#each roots as r (r.root_id)}<option value={r.root_id}>{r.display_path.text}</option>{/each}
        </select>
      </div>
      <button class="btn primary" disabled={loading || !canStart || active || duplicates.pending} onclick={() => duplicates.start(selected)}>Start inspection</button>
      <button class="btn" disabled={!active || job?.state === 'cancel_requested' || duplicates.pending} onclick={() => duplicates.cancel(selected)}>Cancel inspection</button>
    </div>
    {#if !loading && !roots.length}<p class="empty">No granted roots are available.</p>
    {:else if root && !canStart}<p class="empty">This root is unavailable for inspection. Use an active registered synthetic lab root.</p>{/if}
  {/if}
  <p class="live" class:has={!!status} role="status" aria-live="polite">{status}</p>
</section>

<section class="panel" aria-labelledby="duplicate-results">
  <h2 id="duplicate-results" tabindex="-1" bind:this={resultHeading}>Inspection result</h2>
  {#if job?.error}<p class="bad">{job.error.message}</p>{/if}
  {#if report}
    <p class="evidence">Bytes read <strong class="num">{formatBytes(report.bytes_read)}</strong> · Partial <strong class:warn={report.partial}>{report.partial ? 'yes' : 'no'}</strong> · Truncated <strong class:warn={report.truncated}>{report.truncated ? 'yes' : 'no'}</strong> · Stop <strong>{report.stop ?? 'none'}</strong></p>
    <h3>Verified groups</h3>
    {#if report.groups.length}
      <div class="tbl-wrap"><table class="tbl"><caption class="sr-only">Verified equal default streams</caption>
        <thead><tr><th scope="col">Members</th><th scope="col">Logical size per file</th></tr></thead>
        <tbody>{#each report.groups as group}<tr><th scope="row">{#each group as i}<div><VisibleName name={report.entries[i]!.relative_path} /></div>{/each}</th><td class="num">{formatBytes(report.entries[group[0]!]!.logical_bytes)}</td></tr>{/each}</tbody>
      </table></div>
    {:else}<p class="empty">No verified groups in this inspection. Unexamined content remains unknown.</p>{/if}
    <h3>Link sets</h3>
    <p class="muted">One object per set, with no space to reclaim. {report.links_not_compared} listed names were not compared.</p>
    {#if report.links.length}
      <div class="tbl-wrap"><table class="tbl"><caption class="sr-only">Names of the same native object</caption>
        <thead><tr><th scope="col">Names</th><th scope="col">Observation</th></tr></thead>
        <tbody>{#each report.links as links}<tr><th scope="row">{#each links as i}<div><VisibleName name={report.entries[i]!.relative_path} /></div>{/each}</th><td>One object · no space to reclaim</td></tr>{/each}</tbody>
      </table></div>
    {/if}
    <h3>Skipped entries</h3>
    {#if report.skipped.length}
      <div class="tbl-wrap"><table class="tbl"><caption class="sr-only">Skipped entries and reasons</caption>
        <thead><tr><th scope="col">Name</th><th scope="col">Reason</th></tr></thead>
        <tbody>{#each report.skipped as skip}<tr><th scope="row"><VisibleName name={report.entries[skip.entry]!.relative_path} /></th><td>{skip.reason}</td></tr>{/each}</tbody>
      </table></div>
    {:else}<p class="empty">No listed entries were skipped.</p>{/if}
  {:else}<p class="empty">{active ? 'Inspection is running; verified evidence will appear when it ends.' : 'Start an inspection to see its evidence.'}</p>{/if}
</section>

<style>
  h3 { margin: var(--sp-5) 0 var(--sp-2); font-size: 1rem; }
  .evidence { overflow-wrap: anywhere; }
  .evidence strong { font-family: var(--num-font); }
</style>
