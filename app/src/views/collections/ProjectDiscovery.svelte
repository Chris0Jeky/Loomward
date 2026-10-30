<script lang="ts">
  import { untrack } from 'svelte';
  import { session } from '../../lib/stores/session.svelte';
  import type { ProjectsPage, Root } from '../../lib/contracts.gen';
  import { escapedName } from '../../lib/format/names';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import AddToCollection from '../../lib/ui/AddToCollection.svelte';
  import ProjectFootprint from './ProjectFootprint.svelte';
  import { loadFootprint, type Footprint } from './footprint';
  import { LoomwardError } from '../../lib/transport/client';

  let { onAdded, collectionListKey }: { onAdded: () => void; collectionListKey: string } = $props();
  let roots = $state<Root[]>([]);
  let selected = $state('');
  let page = $state<ProjectsPage | null>(null);
  let busy = $state(false);
  let error = $state('');
  let adding = $state<string | null>(null);
  let sequence = 0;
  let abort: AbortController | null = null;
  let expanded = $state<string | null>(null);
  let footprint = $state<Footprint | null>(null);
  let footprintError = $state('');
  let footprintAbort: AbortController | null = null;

  function closeFootprint(): void {
    footprintAbort?.abort(); expanded = null; footprint = null; footprintError = '';
  }
  async function expand(node: string): Promise<void> {
    const wasOpen = expanded === node;
    closeFootprint();
    const client = session.client, generation = page?.generation, root = page?.root_id, epoch = session.epoch;
    if (wasOpen || !client || generation == null || !root) return;
    expanded = node;
    const ctl = footprintAbort = new AbortController();
    try {
      const result = await loadFootprint(client, node, root, generation, ctl.signal);
      if (!ctl.signal.aborted && session.client === client && session.epoch === epoch) footprint = result;
    } catch (e) {
      if (ctl.signal.aborted || session.client !== client || session.epoch !== epoch) return;
      if (e instanceof LoomwardError && ['stale_generation', 'permission_denied', 'not_found'].includes(e.code)) {
        clear(); error = 'Project observations or access changed. Find projects again.';
      } else footprintError = session.handle(e);
    }
  }

  function clear(): void {
    sequence++; abort?.abort(); closeFootprint(); page = null; adding = null; error = ''; busy = false;
  }
  async function find(append = false): Promise<void> {
    const client = session.client, root = selected, previous = append ? page : null;
    if (!client || !root || busy) return;
    clear(); busy = true;
    const mine = sequence, ctl = abort = new AbortController();
    try {
      const { result } = await client.call('projects.find', { root_id: root, limit: 50, cursor: previous?.next_cursor ?? null },
        { signal: ctl.signal, expectedGeneration: previous?.generation ?? undefined });
      if (mine !== sequence) return;
      if (result.root_id !== root || (previous && result.generation !== previous.generation)) throw new Error('Project observations changed. Find projects again.');
      page = { ...result, candidates: [...(previous?.candidates ?? []), ...result.candidates] };
    } catch (e) { if (mine === sequence) error = session.handle(e); }
    finally { if (mine === sequence) busy = false; }
  }
  $effect(() => {
    const client = session.client, epoch = session.epoch, personal = session.datasetClass === 'personal';
    void epoch;
    untrack(clear);
    const ctl = new AbortController();
    if (client) void client.call('roots.list', {}, { signal: ctl.signal }).then(({ result }) => {
      if (ctl.signal.aborted) return;
      roots = result.roots.filter((r) => r.grant_state === 'active' && (r.origin === 'fixture' || r.origin === (personal ? 'owner_granted' : 'lab_generated')));
      if (!roots.some((r) => r.root_id === selected)) selected = roots[0]?.root_id ?? '';
    }).catch((e) => { if (!ctl.signal.aborted) error = session.handle(e); });
    return () => { ctl.abort(); clear(); };
  });
</script>

<section class="panel" aria-labelledby="projects-heading" aria-busy={busy}>
  <h2 id="projects-heading">Projects found here</h2>
  <p>Find probable project folders from names already recorded in a granted folder. This does not scan or open files.</p>
  <p class="muted">A manifest name is a clue, not proof of a complete project or its dependencies. Nested projects stay separate. Adding one changes only your collection.</p>
  <div class="controls">
    <label class="lw-field"><span class="lbl">Project search folder</span>
      <select bind:value={selected} onchange={clear} disabled={!roots.length}>
        {#each roots as root (root.root_id)}<option value={root.root_id}>{escapedName(root.display_path.text)}</option>{/each}
      </select>
    </label>
    <button class="btn primary" type="button" disabled={busy || !selected} onclick={() => void find()}>Find projects</button>
  </div>
  <p class="live bad" class:has={!!error} role="alert">{error}</p>
  {#if page}
    <p role="status">{page.candidates.length} probable project folders shown. Recorded coverage: {page.coverage}.</p>
    {#if page.partial || page.budget_hit}<p class="note warn">{page.budget_hit ? 'The discovery work budget was reached.' : 'This folder is not fully known.'} Projects may be missing, including in excluded or unscanned areas.</p>{/if}
    {#each page.candidates as candidate (candidate.node_id)}
      <article aria-label={escapedName(candidate.name)}>
        <h3><VisibleName name={candidate.name} /></h3>
        <p class="path"><VisibleName name={candidate.path.text} />{candidate.path.truncated ? ' (path shortened)' : ''}</p>
        <p>Recorded markers: {#each candidate.markers as marker, i}{i ? ', ' : ''}<VisibleName name={marker.name} /> ({marker.provider}){/each}{candidate.markers_truncated ? '; more markers recorded' : ''}.</p>
        <p class="muted">Coverage: {candidate.coverage}.</p>
        {#if page.generation !== null}
          <button class="btn" type="button" aria-expanded={expanded === candidate.node_id} onclick={() => void expand(candidate.node_id)}>{expanded === candidate.node_id ? 'Hide recorded footprint' : 'Show recorded footprint'}</button>
          {#if expanded === candidate.node_id}
            {#if footprint}<ProjectFootprint data={footprint} root={roots.find(r => r.root_id === page!.root_id)} generation={page.generation} />
            {:else if footprintError}<p class="note bad" role="alert">Recorded footprint unavailable: {footprintError}</p>
            {:else}<p role="status">Loading recorded footprint…</p>{/if}
          {/if}
        {/if}
        {#if page.generation !== null && (candidate.coverage === 'complete' || candidate.coverage === 'partial')}
          <div class="controls">
            <a class="btn" href={`#/explorer?node=${encodeURIComponent(candidate.node_id)}&generation=${encodeURIComponent(page.generation)}`}>Inspect in Explorer</a>
            <button class="btn" type="button" onclick={() => adding = candidate.node_id}>Choose collection</button>
          </div>
          {#if adding === candidate.node_id}{#key collectionListKey}<AddToCollection nodeId={candidate.node_id} expectedGeneration={page.generation} {onAdded} />{/key}{/if}
        {:else}<p class="note">Current observations are unavailable. Refresh the catalogue before inspecting or adding this result.</p>{/if}
      </article>
    {:else}
      <p class="empty">{page.partial || page.budget_hit ? 'No candidates returned from the available observations. This does not establish that no projects exist.' : 'No recognised project markers are recorded in this folder.'}</p>
    {/each}
    {#if page.next_cursor}<button class="btn" type="button" disabled={busy} onclick={() => void find(true)}>Load more projects</button>{/if}
  {:else if !error}<p class="empty">{busy ? 'Finding recorded project markers…' : roots.length ? 'Choose a folder, then find projects when you are ready.' : 'No granted folder is available.'}{#if !busy && !roots.length} <a href="#/health">Add a folder in Grants &amp; health</a>.{/if}</p>{/if}
</section>
<style>
  select { max-width: 100%; }
  article { border-top: 1px solid var(--line); padding-block: var(--sp-3); }
  .path { overflow-wrap: anywhere; }
</style>
