<script module lang="ts">
  export const meta = { title: 'Reclaim', order: 37 };
</script>
<script lang="ts">
  import { untrack } from 'svelte';
  import type { ReclaimPlan, Volume } from '../../lib/contracts.gen';
  import { formatBytes } from '../../lib/format/bytes';
  import { escapedName } from '../../lib/format/names';
  import { ReclaimObserve } from '../../lib/stores/reclaim-observe.svelte';
  import { session } from '../../lib/stores/session.svelte';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import { COVERAGE_WORDS } from '../clutter/model';
  import { defaultVolume, GROUP_HEADINGS, OBSERVE_EXPLAINER, observeLines, unaccountedText } from './model';

  let volumes = $state<Volume[]>([]);
  let selected = $state('');
  let report = $state<ReclaimPlan | null>(null);
  let loading = $state(true);
  let error = $state('');
  let generation = 0;
  const observe = new ReclaimObserve();
  const lines = $derived(observeLines(observe));

  async function read(id: string): Promise<void> {
    const c = session.client;
    if (!c || !id) { generation++; report = null; loading = false; return; }
    const mine = ++generation;
    loading = true; error = ''; report = null;
    try {
      const { result } = await c.call('reclaim.plan', { volume_id: id, limit: 50 });
      if (mine === generation) report = result;
    } catch (e) {
      if (mine === generation) { error = session.handle(e); report = null; }
    } finally { if (mine === generation) loading = false; }
  }

  $effect(() => {
    const c = session.client;
    void session.epoch;
    let current = true;
    if (c) void (async () => {
      generation++; report = null; loading = true; error = '';
      try {
        const { result } = await c.call('volumes.list', {});
        if (!current) return;
        volumes = result.volumes;
        if (!volumes.some((v) => v.volume_id === selected)) selected = defaultVolume(volumes);
        await untrack(() => read(selected));
      } catch (e) { if (current) { error = session.handle(e); loading = false; } }
    })();
    return () => { current = false; generation++; };
  });

  // Observation (R1g): a read job. Events and a slow poll keep it current; when it ends the plan is read again.
  $effect(() => session.listen((event) => observe.onEvent(event)));
  $effect(() => { void session.epoch; if (session.client) void observe.sync(); });
  $effect(() => {
    if (!observe.active) return;
    const timer = setInterval(() => void observe.refresh(), 1000);
    return () => clearInterval(timer);
  });
  let wasObserving = false;
  $effect(() => {
    const finished = wasObserving && !observe.active;
    wasObserving = observe.active;
    if (finished) untrack(() => void read(selected));
  });
</script>

<h1>Reclaim</h1>
<p class="lede">Recorded facts about space on one volume, grouped by the owner's possible next step. <strong>No reclaim action has been approved.</strong></p>
<p class="note">Advice only. Owner steps and settings below are display text. Loomward does not run them or change files, processes or settings. Allocated space is not a promise of space you could recover.</p>
<p class="muted small">Within each group, known estimated reclaimable sizes rank largest first; unknowns follow. Candidates may overlap, so their sizes are not added together.</p>

<section class="panel" aria-labelledby="reclaim-volume-h">
  <h2 id="reclaim-volume-h">Volume</h2>
  <div class="controls">
    <div class="lw-field">
      <label class="lbl" for="reclaim-volume">Volume to review</label>
      <select id="reclaim-volume" bind:value={selected} disabled={loading || !volumes.length || observe.active} onchange={(e) => { selected = e.currentTarget.value; void read(selected); }}>
        {#each volumes as v (v.volume_id)}<option value={v.volume_id}>{escapedName(v.display_name)}</option>{/each}
      </select>
    </div>
    <button class="btn" disabled={loading || !selected} onclick={() => void read(selected)}>Look again</button>
    <button class="btn" disabled={loading || !selected || observe.pending || observe.active} onclick={() => void observe.start(selected)}>Observe hard links</button>
    {#if observe.active}<button class="btn" disabled={observe.pending || observe.job?.state === 'cancel_requested'} onclick={() => void observe.cancel()}>Cancel observation</button>{/if}
  </div>
  <p class="muted small">{OBSERVE_EXPLAINER}</p>
  <p class="live muted" class:has={!!lines.status} role="status" aria-label="Observation status">{lines.status}</p>
  <p class="live bad" class:has={!!lines.problem} role="status" aria-label="Observation problem">{lines.problem}</p>
  {#if !loading && !volumes.length && !error}<p class="empty">No recorded volume is available.</p>{/if}
  <p class="live bad" class:has={!!error} role="alert">{error}</p>
</section>

<div aria-busy={loading}>
  {#if !report}
    <p class="empty">{loading ? 'Looking at recorded space' : error ? 'Reclaim plan unavailable.' : 'Choose a volume to look at.'}</p>
  {:else}
    <p class="muted">Capacity: {formatBytes(report.volume.capacity_bytes)} · Free: {formatBytes(report.volume.free_bytes)} · Reserve: {formatBytes(report.volume.reserve_bytes)} · Bus: {report.volume.bus_type ?? 'unknown'}</p>
    {#if report.budget_hit}<p class="note warn" role="status"><strong>The search ran out of budget.</strong> Only the evidence reached is shown. An empty group says nothing about what this volume holds.</p>{/if}
    {#if report.partial}<p class="note warn" role="status"><strong>This volume is not fully known.</strong> Coverage or measurements are partial, stale or unknown. Candidates may be missing.</p>{/if}
    {#if report.truncated}<p class="note warn" role="status">More candidates exist than are shown. Up to 50 per group are listed in recorded order.</p>{/if}
    {#each report.groups as group (group.kind)}
      <section class="panel" aria-labelledby={`reclaim-${group.kind}`}>
        <h2 id={`reclaim-${group.kind}`}>{GROUP_HEADINGS[group.kind]}</h2>
        {#if !group.candidates.length}
          <p class="empty">{report.partial || report.budget_hit ? 'None found in the evidence reached. That is not a clean result.' : 'No candidates recorded in this group.'}</p>
        {:else}
          <ul class="candidates">
            {#each group.candidates as candidate}
              <li>
                <h3><VisibleName name={candidate.name ?? 'Name unknown'} /></h3>
                <p>{candidate.class.replaceAll('_', ' ')} · {candidate.provider}{candidate.kind ? ` · ${candidate.kind.replaceAll('_', ' ')}` : ''}</p>
                <p>Allocated: {formatBytes(candidate.allocated_bytes)} · Logical: {formatBytes(candidate.logical_bytes)} · Estimated reclaimable: {formatBytes(candidate.reclaimable_bytes)}</p>
                {#if candidate.reclaimable_reason}<p class="muted">{candidate.reclaimable_reason}</p>{/if}
                <p>Coverage: {COVERAGE_WORDS[candidate.coverage] ?? candidate.coverage}</p>
                <p class="owner-step">Owner step: {candidate.owner_step}</p>
                {#if candidate.relocation}
                  <p>Relocation setting: {candidate.relocation.variable_or_setting} · Suggested target volume: {candidate.relocation.suggested_target_volume ?? 'unknown'}</p>
                {/if}
                <ul aria-label="Evidence">{#each candidate.evidence as fact}<li>{fact}</li>{/each}</ul>
                <p class="muted">Uncertainty: {candidate.uncertainty}</p>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {/each}
    <footer class="note" aria-label="Unaccounted space">
      <p>{unaccountedText(report)}</p>
      <p class="muted small">Volume used minus covered-root allocation; this is not estimated reclaimable space.</p>
    </footer>
  {/if}
</div>

<style>
  .candidates { padding-inline-start: var(--sp-4); margin: 0; }
  .candidates > li + li { border-top: 1px solid var(--line); padding-top: var(--sp-4); margin-top: var(--sp-4); }
  .candidates p, .candidates li { overflow-wrap: anywhere; }
</style>
