<script module lang="ts">
  export const meta = { title: 'Explorer', order: 30 };
</script>

<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { formatBytes } from '../../lib/format/bytes';
  import { hasHiddenCharacters } from '../../lib/format/names';
  import { formatCount, formatTime } from '../../lib/format/time';
  import { LoomwardError } from '../../lib/transport/client';
  import { session } from '../../lib/stores/session.svelte';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import type { Basis, EntryPage, EntryRow, NodeId } from '../../lib/contracts.gen';

  type Sort = 'size_desc' | 'name_asc' | 'modified_desc';
  interface Crumb { id: NodeId; name: string }

  const PAGE = 50;
  const SIZES: [string, string][] = [['Any size', ''], ['1 MiB or more', '1048576'], ['100 MiB or more', '104857600'], ['1 GiB or more', '1073741824']];

  let starts = $state<Crumb[]>([]);
  let trail = $state<Crumb[]>([]);
  let rows = $state<EntryRow[]>([]);
  let total = $state<number | null>(null);
  let next = $state<string | null>(null);
  let budgetHit = $state(false);
  let basis = $state<Basis>('logical');
  let sort = $state<Sort>('size_desc');
  let query = $state('');
  let kind = $state<'any' | 'file' | 'dir'>('any');
  let extension = $state('');
  let minBytes = $state('');
  let searching = $state(false);
  let error = $state('');
  let notice = $state('');
  let busy = $state(false);
  let searchEl = $state<HTMLInputElement>();
  let moreEl = $state<HTMLButtonElement>();
  let countEl = $state<HTMLElement>();

  // View generation. Every action that changes what the table shows bumps it; a response that comes
  // back for an older generation is dropped (and its request aborted), so a slow answer can never
  // overwrite a newer search, folder or basis.
  let gen = 0;
  let abort: AbortController | null = null;
  /** `EntryPage.generation` of the first page of the current view; a later page from another one means the folder changed. */
  let pageGen: string | null = null;

  const here = $derived(trail[trail.length - 1]);

  function fetchPage(cursor: string | null, signal: AbortSignal): Promise<{ result: EntryPage }> {
    const c = session.client!;
    if (searching) {
      const ext = extension.trim().replace(/^\./, '').toLowerCase();
      return c.call('search.query', { root_id: null, text: query.trim(), extension: ext || null, min_bytes: minBytes || null, kind, limit: PAGE, cursor }, { signal });
    }
    return c.call('tree.children', { node_id: here!.id, sort, basis, limit: PAGE, cursor }, { signal });
  }

  async function show(append = false, restarted = false): Promise<void> {
    if (!session.client || (!searching && !here)) return;
    if (!append) {
      abort?.abort();
      gen++;
    }
    const mine = gen;
    const ctl = (abort = new AbortController());
    const hadFocus = append && document.activeElement === moreEl; // Load more goes away after the last page
    busy = true;
    error = '';
    if (!restarted) notice = '';
    const restart = (why: string) => {
      notice = why;
      return show(false, true);
    };
    try {
      const { result: page } = await fetchPage(append ? next : null, ctl.signal);
      if (mine !== gen) return;
      if (append && pageGen !== null && page.generation !== null && page.generation !== pageGen) {
        return await restart('The listing changed while you were paging, so it starts again from the top.');
      }
      if (!append) pageGen = page.generation;
      rows = append ? [...rows, ...page.items] : page.items;
      total = page.total;
      next = page.next_cursor;
      budgetHit = page.budget_hit;
      if (hadFocus) {
        await tick();
        // only if the reader is still there: on the button, or on the body because the button just went away
        const at = document.activeElement;
        if (at === moreEl || at === document.body) (next ? moreEl : countEl)?.focus();
      }
    } catch (e) {
      if (mine !== gen) return;
      if (e instanceof LoomwardError && e.code === 'stale_generation' && !restarted) {
        return await restart('The listing changed under the cursor, so it starts again from the top.');
      }
      error = session.handle(e);
      if (!append) {
        // A failed refresh must not leave invalidated rows on screen as if they were current.
        rows = [];
        total = null;
        next = null;
        budgetHit = false;
        pageGen = null;
      }
    } finally {
      if (mine === gen) busy = false;
    }
  }

  function search() {
    if (!query.trim() && !extension.trim()) return clearSearch();
    searching = true;
    void show();
  }

  function clearSearch() {
    searching = false;
    query = '';
    extension = '';
    void show();
    void tick().then(() => searchEl?.focus()); // the Clear search button is gone: focus the search box
  }

  function go(to: Crumb[]) {
    trail = to;
    searching = false;
    query = '';
    void show();
  }

  async function loadStarts(): Promise<void> {
    const c = session.client!;
    // Same guard as show(): a newer view or a newer starting-point request supersedes this one.
    abort?.abort();
    const mine = ++gen;
    const ctl = (abort = new AbortController());
    try {
      const { result } = await c.call('tree.slice', { anchor: { kind: 'atlas' }, depth: 1, max_nodes: 16, min_share: 0, basis: 'logical', include_files: false }, { signal: ctl.signal });
      if (mine !== gen) return;
      starts = result.nodes.filter((n) => n.parent === 0).map((n) => ({ id: n.node_id, name: n.name }));
      trail = starts[0] ? [starts[0]] : [];
      error = '';
      if (trail.length) await show();
      else { rows = []; total = null; next = null; }
    } catch (e) {
      if (mine !== gen) return;
      rows = [];
      total = null;
      next = null;
      error = session.handle(e);
    }
  }

  // The session starts, or the engine says data may be stale (reconnect, tree.invalidated, roots.changed):
  // keep the user's place and ask again; the first time, find the starting points.
  $effect(() => {
    void session.epoch;
    if (!session.client) return;
    untrack(() => void (trail.length ? show() : loadStarts()));
  });
</script>

<h1>Explorer</h1>
<p class="muted">
  A paged table over the {session.datasetClass ?? 'unknown'} dataset: the text equivalent of the maps. Names are plain text and hidden characters
  are shown as badges; an unknown size stays <span class="unknown">unknown</span>.
</p>

<section class="panel">
  <form class="bar form" onsubmit={(e) => { e.preventDefault(); search(); }}>
    <label class="field stack grow"><span class="lbl">Search names</span>
      <input type="search" placeholder="Search names" bind:value={query} bind:this={searchEl} maxlength="256" />
    </label>
    <label class="field stack"><span class="lbl">Extension</span>
      <input type="text" placeholder="ext" bind:value={extension} maxlength="32" size="6" />
    </label>
    <label class="field stack"><span class="lbl">Kind</span>
      <select bind:value={kind}><option value="any">Files and folders</option><option value="file">Files</option><option value="dir">Folders</option></select>
    </label>
    <label class="field stack"><span class="lbl">Size</span>
      <select bind:value={minBytes}>{#each SIZES as [label, v] (label)}<option value={v}>{label}</option>{/each}</select>
    </label>
    <button class="btn" type="submit" disabled={!query.trim() && !extension.trim()}>Search</button>
    {#if searching}<button class="btn" type="button" onclick={clearSearch}>Clear search</button>{/if}
  </form>

  <div class="bar">
    <div role="group" aria-label="Size basis">
      <button class="btn" type="button" aria-pressed={basis === 'logical'} onclick={() => { basis = 'logical'; void show(); }}>Logical</button>
      <button class="btn" type="button" aria-pressed={basis === 'allocated'} onclick={() => { basis = 'allocated'; void show(); }}>Allocated</button>
    </div>
    <label class="field">Sort
      <select bind:value={sort} disabled={searching} onchange={() => void show()}>
        <option value="size_desc">Size, largest first</option><option value="name_asc">Name, A to Z</option><option value="modified_desc">Modified, newest first</option>
      </select>
    </label>
    {#if searching}<span class="muted small">Search results keep the engine's order.</span>{/if}
  </div>

  {#if starts.length > 1}
    <div class="starts" role="group" aria-label="Start from">
      {#each starts as s (s.id)}
        <button class="btn" type="button" aria-pressed={!searching && trail[0]?.id === s.id} onclick={() => go([s])}><VisibleName name={s.name} /></button>
      {/each}
    </div>
  {/if}

  {#if searching}
    <p class="where">Search results {#if query.trim()}for <q><VisibleName name={query.trim()} /></q>{/if}</p>
  {:else}
    <nav class="crumbs" aria-label="Location">
      {#each trail as c, i (c.id)}
        {#if i > 0}<span aria-hidden="true">›</span>{/if}
        <button class="crumb" type="button" aria-current={i === trail.length - 1 ? 'location' : undefined} onclick={() => go(trail.slice(0, i + 1))}><VisibleName name={c.name} /></button>
      {/each}
    </nav>
  {/if}

  {#if error}
    <p class="bad" role="alert">{error} <button class="btn" type="button" onclick={() => void (trail.length ? show() : loadStarts())}>Try again</button></p>
  {/if}
  {#if notice}<p class="muted" role="status">{notice}</p>{/if}

  <div class="tbl-wrap" aria-busy={busy}>
    <table class="tbl">
      <caption class="sr-only">{searching ? 'Search results' : 'Folder contents'}</caption>
      <thead>
        <tr>
          <th scope="col">Name</th><th scope="col">Kind</th>
          <th scope="col" class="r" class:basis={basis === 'logical'}>Logical</th>
          <th scope="col" class="r" class:basis={basis === 'allocated'}>Allocated</th>
          <th scope="col" class="r">Files</th><th scope="col">Modified</th><th scope="col">Notes</th>
        </tr>
      </thead>
      <tbody>
        {#each rows as r (r.node_id)}
          <tr>
            <th scope="row">
              {#if r.kind === 'dir' && !searching && r.coverage !== 'denied'}
                <button class="crumb" type="button" onclick={() => go([...trail, { id: r.node_id, name: r.name }])}><VisibleName name={r.name} /></button>
              {:else}
                <VisibleName name={r.name} />
              {/if}
              {#if r.location_hint}<div class="muted small">in <VisibleName name={r.location_hint.text} />{#if r.location_hint.truncated} <span class="tag warn">truncated</span>{/if}</div>{/if}
            </th>
            <td>{r.kind}</td>
            <td class="r num">{formatBytes(r.logical_bytes)}</td>
            <td class="r num">{#if r.allocated_bytes === null}<span class="unknown">unknown</span>{:else}{formatBytes(r.allocated_bytes)}{/if}</td>
            <td class="r num">{r.kind === 'file' ? '–' : formatCount(r.files)}</td>
            <td class="nowrap">{formatTime(r.modified_at)}</td>
            <td>
              {#if hasHiddenCharacters(r.name)}<span class="tag warn">hidden characters</span>{/if}
              {#if r.coverage !== 'complete'}<span class="tag warn">{r.coverage}</span>{/if}
              {#each r.flags as f (f)}<span class="tag">{f.replaceAll('_', ' ')}</span>{/each}
            </td>
          </tr>
        {:else}
          <tr><td colspan="7" class="muted">{busy ? 'Loading' : error ? 'Nothing is shown because the last request failed.' : 'Nothing to show.'}</td></tr>
        {/each}
      </tbody>
    </table>
  </div>

  <div class="foot">
    <span class="muted num" aria-live="polite" tabindex="-1" bind:this={countEl}>Showing {formatCount(rows.length)} of {formatCount(total)}{#if budgetHit} · the engine stopped at its work budget, so more may exist{/if}</span>
    {#if next}<button class="btn" type="button" aria-disabled={busy} bind:this={moreEl} onclick={() => { if (!busy) void show(true); }}>Load more</button>{/if}
  </div>
</section>

<style>
  .bar { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; margin-bottom: 12px; }
  .field { display: inline-flex; align-items: center; gap: 6px; }
  .stack { flex-direction: column; align-items: flex-start; gap: 2px; }
  .lbl { font-size: 0.8rem; color: var(--muted); }
  .form { align-items: flex-end; }
  .grow { flex: 1 1 14rem; }
  .grow input { width: 100%; }
  .starts { display: flex; flex-wrap: wrap; gap: 6px; margin-bottom: 8px; }
  .where { margin: 0 0 8px; }
  .crumbs { display: flex; flex-wrap: wrap; align-items: center; gap: 4px; margin: 0 0 8px; }
  .crumb { font: inherit; color: var(--accent); background: none; border: 0; padding: 0 2px; cursor: pointer; text-align: left; overflow-wrap: anywhere; }
  .crumb[aria-current='location'] { color: var(--text); cursor: default; }
  .small { font-size: 0.8rem; }
  .bad { color: var(--danger); }
  th.basis { color: var(--text); box-shadow: inset 0 -2px 0 var(--accent); }
  @media (forced-colors: active) {
    th.basis, .crumb[aria-current='location'] { text-decoration: underline; text-underline-offset: 3px; }
  }
  .foot { display: flex; justify-content: space-between; align-items: center; gap: 12px; flex-wrap: wrap; margin-top: 12px; }
</style>
