<script module lang="ts">
  export const meta = { title: 'Reclaim', order: 37 };
</script>
<script lang="ts">
  import { untrack } from 'svelte';
  import type { ReclaimPlan, ReclaimSelection, Volume } from '../../lib/contracts.gen';
  import { formatBytes } from '../../lib/format/bytes';
  import { escapedName } from '../../lib/format/names';
  import { ReclaimObserve } from '../../lib/stores/reclaim-observe.svelte';
  import { session } from '../../lib/stores/session.svelte';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import { COVERAGE_WORDS } from '../clutter/model';
  import { defaultVolume, GROUP_HEADINGS, OBSERVE_EXPLAINER, observationAge, observeLines, selectionComparison, targetBytes, unaccountedText } from './model';

  let volumes = $state<Volume[]>([]);
  let selected = $state('');
  let report = $state<ReclaimPlan | null>(null);
  let loading = $state(true);
  let error = $state('');
  let generation = 0;
  let members = $state<string[]>([]);
  let target = $state('');
  let selectionResult = $state<ReclaimSelection | null>(null);
  let now = $state(Date.now());
  const observe = new ReclaimObserve();
  const lines = $derived(observeLines(observe));

  async function read(id: string): Promise<void> {
    const c = session.client;
    if (!c || !id) { generation++; report = null; loading = false; return; }
    const mine = ++generation;
    const requested = [...members];
    loading = true; error = ''; report = null; selectionResult = null;
    try {
      const { result } = await c.call('reclaim.plan', { volume_id: id, limit: 50, ...(requested.length ? { selection: requested } : {}) });
      if (mine === generation) {
        report = result;
        if (requested.join() === members.join() && result.selection?.members.length === members.length
            && result.selection.members.every((m) => members.includes(m))) selectionResult = result.selection;
      }
    } catch (e) {
      if (mine === generation) { error = session.handle(e); report = null; }
    } finally { if (mine === generation) loading = false; }
  }

  function toggle(id: string, checked: boolean): void {
    members = checked ? [...members, id] : members.filter((m) => m !== id);
    selectionResult = null;
  }

  async function observeSelection(): Promise<void> {
    selectionResult = null;
    await observe.start(selected, [...members]);
    if (!observe.active) await read(selected);
  }

  $effect(() => {
    if (!selectionResult?.observed_at) return;
    now = Date.now();
    const timer = setInterval(() => { now = Date.now(); }, 10000);
    return () => clearInterval(timer);
  });

  $effect(() => {
    const c = session.client;
    void session.epoch;
    let current = true;
    if (c) void (async () => {
      generation++; report = null; selectionResult = null; loading = true; error = '';
      try {
        const { result } = await c.call('volumes.list', {});
        if (!current) return;
        volumes = result.volumes;
        if (!volumes.some((v) => v.volume_id === selected)) { selected = defaultVolume(volumes); members = []; }
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
      <select id="reclaim-volume" bind:value={selected} disabled={loading || !volumes.length || observe.active} onchange={(e) => { selected = e.currentTarget.value; members = []; selectionResult = null; void read(selected); }}>
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

<section class="panel" aria-labelledby="selection-h">
  <h2 id="selection-h">Inspect a selection</h2>
  <p>Choose up to 12 candidates on this volume. Observing them together checks shared hard links once across the selected set.</p>
  <div class="controls">
    <div class="lw-field">
      <label class="lbl" for="reclaim-target">Space target (bytes)</label>
      <input id="reclaim-target" type="text" inputmode="numeric" maxlength="20" bind:value={target}
        aria-invalid={target !== '' && targetBytes(target) === null} aria-describedby="selection-comparison" />
    </div>
    <button class="btn" disabled={!members.length || loading || observe.pending || observe.active || targetBytes(target) === null}
      onclick={() => void observeSelection()}>Observe selection</button>
    <button class="btn" disabled={!members.length} onclick={() => { members = []; selectionResult = null; }}>Clear selection</button>
  </div>
  <p class="muted small">{members.length} of 12 selected. Moving items to the Recycle Bin or renaming them on the same volume does not immediately free this space.</p>
  <div role="status" aria-label="Selection estimate">
    {#if selectionResult?.status === 'recorded'}
      <p>Recorded union estimate: {selectionResult.reclaimable_bytes === null ? 'unknown' : `${selectionResult.reclaimable_bytes} bytes (${formatBytes(selectionResult.reclaimable_bytes)})`}.</p>
    {:else if selectionResult?.status === 'expired'}
      <p><strong>This observation has expired.</strong> Current union estimate: unknown.</p>
    {:else}
      <p>This selection has not been observed. Union estimate: unknown.</p>
    {/if}
    {#if selectionResult?.observed_at}<p class="muted">Recorded {observationAge(selectionResult.observed_at, now)} · <time datetime={selectionResult.observed_at}>{selectionResult.observed_at}</time></p>{/if}
    {#if selectionResult?.reason}<p>{selectionResult.reason}</p>{/if}
    <p id="selection-comparison">{selectionComparison(selectionResult, target)}</p>
  </div>
  <p class="muted small">Recorded allocation evidence only. This does not approve removal or promise physical space recovery.</p>
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
                {#if candidate.node_id}
                  <label class="selection-choice"><input type="checkbox" checked={members.includes(candidate.node_id)}
                    disabled={members.length === 12 && !members.includes(candidate.node_id)}
                    onchange={(e) => toggle(candidate.node_id!, e.currentTarget.checked)} />
                    Include <VisibleName name={candidate.name ?? 'Name unknown'} /> in selection</label>
                {/if}
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
  .selection-choice { display: inline-flex; align-items: center; gap: var(--sp-2); min-height: 44px; }
  .selection-choice input { width: 1.125rem; height: 1.125rem; flex-shrink: 0; }
  #reclaim-target { max-width: 100%; }
  .candidates { padding-inline-start: var(--sp-4); margin: 0; }
  .candidates > li + li { border-top: 1px solid var(--line); padding-top: var(--sp-4); margin-top: var(--sp-4); }
  .candidates p, .candidates li { overflow-wrap: anywhere; }
</style>
