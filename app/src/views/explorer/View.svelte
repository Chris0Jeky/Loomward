<script module lang="ts">
  export const meta = { title: 'Explorer', order: 30 };
</script>

<script lang="ts">
  // PLACEHOLDER (lane L3). Lane L13 takes this over: real paging, search filters, stale-generation handling.
  import { formatBytes } from '../../lib/format/bytes';
  import { formatCount, formatTime } from '../../lib/format/time';
  import { session } from '../../lib/stores/session.svelte';
  import type { Basis, EntryPage, EntryRow, NodeId } from '../../lib/types';

  const PAGE = 50;
  interface Crumb { id: NodeId; name: string }

  let starts = $state<Crumb[]>([]);
  let trail = $state<Crumb[]>([]);
  let rows = $state<EntryRow[]>([]);
  let total = $state<number | null>(null);
  let next = $state<string | null>(null);
  let basis = $state<Basis>('logical');
  let query = $state('');
  let searching = $state(false);
  let error = $state('');
  let busy = $state(false);
  let ticket = 0; // a response from an older request is dropped

  const here = $derived(trail[trail.length - 1]);

  async function run(fetchPage: () => Promise<EntryPage>, append: boolean) {
    const mine = ++ticket;
    busy = true;
    error = '';
    try {
      const page = await fetchPage();
      if (mine !== ticket) return;
      rows = append ? [...rows, ...page.items] : page.items;
      total = page.total;
      next = page.next_cursor;
    } catch (e) {
      if (mine === ticket) error = session.handle(e);
    } finally {
      if (mine === ticket) busy = false;
    }
  }

  function list(append = false) {
    const node = here;
    if (!node || !session.client) return;
    searching = false;
    void run(
      async () => (await session.client!.call('tree.children', { node_id: node.id, sort: 'size_desc', basis, limit: PAGE, cursor: append ? next : null })).result,
      append,
    );
  }

  function search(append = false) {
    const text = query.trim();
    if (!text) return back();
    searching = true;
    void run(
      async () => (await session.client!.call('search.query', { root_id: null, text, extension: null, min_bytes: null, kind: 'any', limit: 50, cursor: append ? next : null })).result,
      append,
    );
  }

  function back() {
    query = '';
    list();
  }

  function open(row: EntryRow) {
    trail = [...trail, { id: row.node_id, name: row.name }];
    query = '';
    list();
  }

  function jump(i: number) {
    trail = trail.slice(0, i + 1);
    query = '';
    list();
  }

  // (Re)load whenever the session starts or the engine says data may be stale.
  $effect(() => {
    void session.epoch;
    const c = session.client;
    if (!c) return;
    void (async () => {
      try {
        const { result } = await c.call('tree.slice', { anchor: { kind: 'atlas' }, depth: 1, max_nodes: 16, min_share: 0, basis: 'logical', include_files: false });
        starts = result.nodes.filter((n) => n.parent === 0).map((n) => ({ id: n.node_id, name: n.name }));
        trail = starts[0] ? [starts[0]] : [];
        query = '';
        list();
      } catch (e) {
        error = session.handle(e);
      }
    })();
  });
</script>

<h1>Explorer</h1>
<p class="muted">
  Placeholder table over the {session.datasetClass ?? 'unknown'} dataset. Names are shown as plain text; sizes are decimal strings
  converted for display only, and an unknown size stays <span class="unknown">unknown</span>.
</p>

<section class="panel">
  <div class="bar">
    <label>
      <span class="sr-only">Search names</span>
      <input type="search" placeholder="Search names" bind:value={query} onkeydown={(e) => e.key === 'Enter' && search()} maxlength="256" />
    </label>
    <button class="btn" type="button" onclick={() => search()} disabled={busy || !query.trim()}>Search</button>
    {#if searching}<button class="btn" type="button" onclick={back}>Clear search</button>{/if}
    <span class="grow"></span>
    <div role="group" aria-label="Size basis">
      <button class="btn" type="button" aria-pressed={basis === 'logical'} onclick={() => { basis = 'logical'; searching ? search() : list(); }}>Logical</button>
      <button class="btn" type="button" aria-pressed={basis === 'allocated'} onclick={() => { basis = 'allocated'; searching ? search() : list(); }}>Allocated</button>
    </div>
  </div>

  {#if starts.length > 1}
    <div class="starts" role="group" aria-label="Start from">
      {#each starts as s (s.id)}
        <button class="btn" type="button" aria-pressed={trail[0]?.id === s.id} onclick={() => { trail = [s]; query = ''; list(); }}><bdi>{s.name}</bdi></button>
      {/each}
    </div>
  {/if}

  {#if searching}
    <p class="crumbs">Search results for <q><bdi>{query}</bdi></q></p>
  {:else}
    <nav class="crumbs" aria-label="Location">
      {#each trail as c, i (c.id)}
        {#if i > 0}<span aria-hidden="true">›</span>{/if}
        <button class="crumb" type="button" aria-current={i === trail.length - 1 ? 'location' : undefined} onclick={() => jump(i)}><bdi>{c.name}</bdi></button>
      {/each}
    </nav>
  {/if}

  {#if error}<p class="bad" role="alert">{error}</p>{/if}

  <div class="tbl-wrap">
    <table class="tbl">
      <caption class="sr-only">{searching ? 'Search results' : 'Folder contents'}</caption>
      <thead>
        <tr><th scope="col">Name</th><th scope="col">Kind</th><th scope="col" class="r">Logical</th><th scope="col" class="r">Allocated</th><th scope="col" class="r">Files</th><th scope="col">Modified</th><th scope="col">Notes</th></tr>
      </thead>
      <tbody>
        {#each rows as r (r.node_id)}
          <tr>
            <td>
              {#if r.kind === 'dir' && !searching && r.coverage !== 'denied'}
                <button class="crumb" type="button" onclick={() => open(r)}><bdi>{r.name}</bdi></button>
              {:else}
                <bdi>{r.name}</bdi>
              {/if}
              {#if r.location_hint}<div class="muted small"><bdi>{r.location_hint.text}</bdi></div>{/if}
            </td>
            <td>{r.kind}</td>
            <td class="r num">{formatBytes(r.logical_bytes)}</td>
            <td class="r num">{#if r.allocated_bytes === null}<span class="unknown">unknown</span>{:else}{formatBytes(r.allocated_bytes)}{/if}</td>
            <td class="r num">{formatCount(r.files)}</td>
            <td>{formatTime(r.modified_at)}</td>
            <td>
              {#if r.coverage !== 'complete'}<span class="tag warn">{r.coverage}</span>{/if}
              {#each r.flags as f (f)}<span class="tag">{f.replaceAll('_', ' ')}</span>{/each}
            </td>
          </tr>
        {:else}
          <tr><td colspan="7" class="muted">{busy ? 'Loading' : 'Nothing to show.'}</td></tr>
        {/each}
      </tbody>
    </table>
  </div>

  <div class="foot">
    <span class="muted num">Showing {formatCount(rows.length)} of {formatCount(total)}</span>
    {#if next}<button class="btn" type="button" disabled={busy} onclick={() => (searching ? search(true) : list(true))}>Load more</button>{/if}
  </div>
</section>

<style>
  .bar { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; margin-bottom: 12px; }
  .grow { flex: 1; }
  .starts { display: flex; flex-wrap: wrap; gap: 6px; margin-bottom: 8px; }
  .crumbs { display: flex; flex-wrap: wrap; align-items: center; gap: 4px; margin: 0 0 8px; }
  .crumb { font: inherit; color: var(--accent); background: none; border: 0; padding: 0 2px; cursor: pointer; text-align: left; overflow-wrap: anywhere; }
  .crumb[aria-current='location'] { color: var(--text); cursor: default; }
  .small { font-size: 0.8rem; }
  .bad { color: var(--danger); }
  .foot { display: flex; justify-content: space-between; align-items: center; margin-top: 12px; }
</style>
