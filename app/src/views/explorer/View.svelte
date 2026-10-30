<script module lang="ts">
  export const meta = { title: 'Explorer', order: 30 };
</script>

<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { formatBytes } from '../../lib/format/bytes';
  import { hasHiddenCharacters, escapedName } from '../../lib/format/names';
  import { formatCount, formatTime } from '../../lib/format/time';
  import { LoomwardError } from '../../lib/transport/client';
  import { fallbackStep, onRefetchError, resolveTrail, type Crumb } from './refetch';
  import { session } from '../../lib/stores/session.svelte';
  import { route } from '../../lib/stores/route.svelte';
  import { planFind, PRESETS, type FindRequest } from '../../lib/search/find';
  import NodeMeaning from '../../lib/ui/NodeMeaning.svelte';
  import AddToCollection from '../../lib/ui/AddToCollection.svelte';
  import CopyTo from '../../lib/ui/CopyTo.svelte';
  import FolderInsight from '../../lib/ui/FolderInsight.svelte';
  import type { CopyItem } from '../../lib/stores/copy.svelte';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import type { Basis, EntryPage, EntryRow, NodeId, NodeDetail, SearchHit, SearchPage } from '../../lib/contracts.gen';

  type Sort = 'size_desc' | 'name_asc' | 'modified_desc';
  interface Recovery { lost: string; saved: Crumb[]; focus: HTMLElement | null }

  const PAGE = 50;
  const SIZES: [string, string][] = [['Any size', ''], ['1 MiB or more', '1048576'], ['100 MiB or more', '104857600'], ['1 GiB or more', '1073741824']];

  let detail = $state<NodeDetail | null>(null);
  let inspectError = $state('');
  let inspector = $state<HTMLElement>();
  let inspection = 0;
  let inspectAbort: AbortController | null = null;
  let detailGeneration = $state<string | undefined>();
  async function inspect(node_id: NodeId, expectedGeneration?: string): Promise<void> {
    inspectAbort?.abort();
    const ctl = inspectAbort = new AbortController();
    const mine = ++inspection;
    detail = null; inspectError = '';
    detailGeneration = expectedGeneration;
    try {
      const { result } = await session.client!.call('node.inspect', { node_id }, { signal: ctl.signal, expectedGeneration });
      if (mine !== inspection) return;
      detail = result;
      await tick(); inspector?.focus();
    } catch (e) { if (mine === inspection) inspectError = session.handle(e); }
  }
  let starts = $state<Crumb[]>([]);
  let trail = $state<Crumb[]>([]);
  let rows = $state<EntryRow[]>([]);
  let selected = $state<CopyItem[]>([]);
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
  /** The request of the search that is showing, frozen when it started so Load more repeats it (relative times included). */
  let sent: FindRequest | null = null;
  /** The box text of the search that is showing. */
  let ran = $state('');
  /** The box text as it is being typed, parsed on every keystroke: chips and notices follow it. */
  const plan = $derived(planFind(query, { extension, kind, minBytes }, Date.now()));
  /** Per-row search facts (match tier, highlights, root-relative path), by node ID. */
  let found = $state(new Map<NodeId, SearchHit>());
  /** The honesty flags of the last search page: never an empty page passed off as proof of absence. */
  let scope = $state<Pick<SearchPage, 'index_state' | 'partial' | 'substring_searched'> | null>(null);
  let tableEl = $state<HTMLTableElement>();
  let error = $state('');
  let notice = $state('');
  /** A saved folder vanished from the catalogue (#198): its name, and where the view moved to (null: the top level). */
  let gone = $state<{ lost: string; now: string | null } | null>(null);
  let busy = $state(false);
  let searchEl = $state<HTMLInputElement>();
  let moreEl = $state<HTMLButtonElement>();
  let countEl = $state<HTMLElement>();
  let locationEl = $state<HTMLElement>();
  let noticeEl = $state<HTMLElement>();

  // View generation. Every action that changes what the table shows bumps it; a response that comes
  // back for an older generation is dropped (and its request aborted), so a slow answer can never
  // overwrite a newer search, folder or basis.
  let gen = 0;
  let startsRequest = 0;
  let startsAbort: AbortController | null = null;
  let abort: AbortController | null = null;
  /** `EntryPage.generation` of the first page of the current view; a later page from another one means the folder changed. */
  let pageGen: string | null = null;

  /** IDs that have listed successfully: the saved breadcrumbs a later `not_found` falls back from. */
  let seenIds = new Set<NodeId>();

  const here = $derived(trail[trail.length - 1]);

  type Page = Pick<EntryPage, 'generation' | 'items' | 'next_cursor' | 'total' | 'budget_hit'> & { search?: SearchPage };
  async function fetchPage(cursor: string | null, signal: AbortSignal): Promise<{ result: Page }> {
    const c = session.client!;
    if (searching) {
      const { result } = await c.call('search.query', { root_id: null, ...sent!, limit: PAGE, cursor }, { signal });
      // No generation: a search cursor is a keyset over the live index, so a root's generation moving
      // between pages (every watcher run) is no reason to restart the results from the top.
      return { result: { generation: null, items: result.hits.map((h) => h.row), next_cursor: result.next_cursor, total: null, budget_hit: result.budget_hit, search: result } };
    }
    return c.call('tree.children', { node_id: here!.id, sort, basis, limit: PAGE, cursor }, { signal });
  }

  /** The name split at the match highlights; null when there are none or the name needs the hidden-character view. */
  function marked(r: EntryRow): { text: string; hit: boolean }[] | null {
    const h = found.get(r.node_id);
    if (!h?.highlights.length || hasHiddenCharacters(r.name)) return null;
    const out: { text: string; hit: boolean }[] = [];
    let at = 0;
    for (const [from, to] of h.highlights) {
      if (from === undefined || to === undefined || from < at || to > r.name.length) return null;
      if (from > at) out.push({ text: r.name.slice(at, from), hit: false });
      out.push({ text: r.name.slice(from, to), hit: true });
      at = to;
    }
    if (at < r.name.length) out.push({ text: r.name.slice(at), hit: false });
    return out;
  }

  /** Arrow keys walk the result rows' Inspect buttons; Escape goes back to the search box. */
  function rowKeys(e: KeyboardEvent): void {
    if (!['ArrowDown', 'ArrowUp', 'Escape'].includes(e.key)) return;
    const buttons = [...(tableEl?.querySelectorAll<HTMLButtonElement>('button[data-inspect]') ?? [])];
    const i = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (i < 0) return;
    e.preventDefault();
    if (e.key === 'Escape') return searchEl?.focus();
    buttons[Math.max(0, Math.min(buttons.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1)))]?.focus();
  }
  function searchKeys(e: KeyboardEvent): void {
    if (e.key !== 'ArrowDown' || !searching) return;
    const first = tableEl?.querySelector<HTMLButtonElement>('button[data-inspect]');
    if (first) { e.preventDefault(); first.focus(); }
  }

  async function restoreLocationFocus(owner: HTMLElement | null, mine: number): Promise<void> {
    if (!owner) return;
    await tick();
    if (mine === gen && (document.activeElement === owner || (!owner.isConnected && document.activeElement === document.body))) {
      (locationEl?.querySelector<HTMLButtonElement>('[aria-current="location"]') ?? noticeEl)?.focus();
    }
  }

  /** The granted path of a root node (display text, never sent back); null when the engine truncated it, so it cannot identify a root. */
  async function rootPath(node_id: string, signal?: AbortSignal): Promise<string | null> {
    const p = (await session.client!.call('node.inspect', { node_id }, { signal })).result?.display_path;
    return p && !p.truncated && typeof p.text === 'string' ? p.text : null;
  }

  /** Remember which granted root this trail starts in while its node ID is live: IDs change on an engine restart, the granted path does not (#198). Best effort. */
  async function bindRoot(crumb: Crumb): Promise<void> {
    try {
      const path = await rootPath(crumb.id);
      if (path) crumb.root = path;
    } catch { /* unbound: recovery falls back to the unique name */ }
  }

  async function show(append = false, restarted = false, recovery?: Recovery): Promise<void> {
    if (!session.client || (!searching && !here)) return;
    if (!append) {
      inspection++; inspectAbort?.abort(); detail = null; inspectError = '';
      abort?.abort();
      gen++;
    }
    const mine = gen;
    const ctl = (abort = new AbortController());
    const hadFocus = append && document.activeElement === moreEl; // Load more goes away after the last page
    const at = document.activeElement;
    const locationFocus = recovery ? recovery.focus : at instanceof HTMLElement && at.matches('button.crumb') ? at : null;
    busy = true;
    error = '';
    if (!restarted) {
      notice = '';
      gone = null;
    }
    const restart = (why: string) => {
      notice = why;
      gone = null;
      return show(false, true, recovery);
    };
    try {
      const { result: page } = await fetchPage(append ? next : null, ctl.signal);
      if (mine !== gen) return;
      if (append && pageGen !== null && page.generation !== null && page.generation !== pageGen) {
        return await restart('The listing changed while you were paging, so it starts again from the top.');
      }
      if (!append) pageGen = page.generation;
      // The fallback's notice is true only once the ancestor has listed: a failed one shows just its error.
      if (recovery && here) gone = { lost: recovery.lost, now: here.name };
      rows = append ? [...new Map([...rows, ...page.items].map(r => [r.node_id, r])).values()] : page.items;
      if (page.search) {
        const seen = append ? new Map(found) : new Map<NodeId, SearchHit>();
        for (const h of page.search.hits) seen.set(h.row.node_id, h);
        found = seen;
        scope = { index_state: page.search.index_state, partial: page.search.partial, substring_searched: page.search.substring_searched };
      } else if (!append) {
        found = new Map();
        scope = null;
      }
      total = page.total;
      next = page.next_cursor;
      budgetHit = page.budget_hit;
      if (!searching && here) seenIds.add(here.id);
      if (!searching && trail[0] && !trail[0].root) void bindRoot(trail[0]);
      if (recovery) await restoreLocationFocus(recovery.focus, mine);
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
      if (!searching && !append && here && onRefetchError(e, seenIds.has(here.id)) === 'fallback') {
        // A saved breadcrumb went stale (restart, rebuild, resync): climb to the deepest ancestor that
        // still lists, or reload the starting points when none does, and say which folder is gone.
        // A `not_found` for a node just clicked stays an error below.
        // Forgetting the ID first keeps a reused ID from falling back twice in a row.
        const step = fallbackStep(trail, recovery?.lost);
        recovery = { lost: step.lost, saved: recovery?.saved ?? trail, focus: locationFocus };
        seenIds.delete(here.id);
        rows = [];
        total = null;
        next = null;
        budgetHit = false;
        pageGen = null;
        error = '';
        busy = false;
        if (step.keep.length) {
          trail = step.keep;
          return await show(false, true, recovery);
        }
        trail = [];
        await loadStarts(recovery);
        return;
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

  /** Freeze what the box says as the request to run. False (nothing is started) when it would match the whole catalogue. */
  function commit(): boolean {
    // Re-plan with the current clock: `plan` is only re-derived when the box or controls change,
    // so a resubmitted `modified:<1h` would otherwise keep the earlier "now".
    const fresh = planFind(query, { extension, kind, minBytes }, Date.now());
    if (fresh.unconstrained) {
      notice = 'Nothing in that query can be searched yet, so no search was run.';
      return false;
    }
    sent = fresh.request;
    ran = query.trim();
    searching = true;
    return true;
  }

  function search() {
    if (!query.trim() && !extension.trim()) return clearSearch();
    if (commit()) void show();
  }

  /** A cleanup search fills the box and runs; the separate controls reset so the preset means exactly what it says. */
  function preset(text: string) {
    query = text; extension = ''; kind = 'any'; minBytes = '';
    search();
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

  /** True when this request completed and its view is still the newest one. */
  async function loadStarts(recovery?: Recovery, refreshOnly = false): Promise<boolean> {
    const c = session.client!;
    // A background root refresh must survive a newer search without changing its rows or trail.
    if (!refreshOnly) abort?.abort();
    const mine = refreshOnly ? gen : ++gen;
    const starting = ++startsRequest;
    startsAbort?.abort();
    const ctl = (startsAbort = new AbortController());
    if (!refreshOnly) abort = ctl;
    try {
      const { result } = await c.call('tree.slice', { anchor: { kind: 'atlas' }, depth: 1, max_nodes: 16, min_share: 0, basis: 'logical', include_files: false }, { signal: ctl.signal });
      if (starting !== startsRequest) return false;
      const fresh = result.nodes.filter((n) => n.parent === 0).map((n) => ({ id: n.node_id, name: n.name }));
      starts = fresh;
      if (refreshOnly) return true;
      if (mine !== gen) return false;
      const resolved = recovery ? await resolveTrail(recovery.saved, fresh, async (node_id, cursor) => {
        ctl.signal.throwIfAborted();
        return (await c.call('tree.children', { node_id, cursor, sort: 'name_asc', basis, limit: PAGE }, { signal: ctl.signal })).result;
      }, (node_id) => rootPath(node_id, ctl.signal)) : [];
      if (mine !== gen) return false;
      for (const crumb of resolved) seenIds.add(crumb.id);
      trail = resolved.length ? resolved : starts[0] ? [starts[0]] : [];
      error = '';
      if (!trail.length) {
        rows = [];
        total = null;
        next = null;
        if (recovery) {
          gone = { lost: recovery.lost, now: null };
          await restoreLocationFocus(recovery.focus, mine);
        }
        return true;
      }
      await show();
      // show() advances `gen` exactly once for a fresh view; any newer action advances it further.
      if (gen !== mine + 1 || error) return false;
      if (recovery) {
        if (resolved.length === recovery.saved.length) notice = 'The catalogue changed; this folder is still available, so its location was restored.';
        else gone = { lost: recovery.lost, now: resolved.length ? here!.name : null };
        await restoreLocationFocus(recovery.focus, mine + 1);
      }
      return gen === mine + 1;
    } catch (e) {
      if (starting !== startsRequest) return false;
      starts = [];
      if (!refreshOnly && mine !== gen) return false;
      if (!refreshOnly) { rows = []; total = null; next = null; }
      error = session.handle(e);
      return false;
    }
  }

  /** Keep the user's place (refreshing starting points beside it), or find the starting points. */
  async function reload(): Promise<void> {
    if (trail.length || searching) await Promise.all([loadStarts(undefined, true), show()]);
    else await loadStarts();
  }

  $effect(() => () => { startsRequest++; startsAbort?.abort(); inspection++; inspectAbort?.abort(); abort?.abort(); });

  // The session starts, or the engine says data may be stale (reconnect, tree.invalidated, roots.changed):
  // keep the user's place and ask again; the first time, find the starting points.
  // `?q=` in the route (a link from another view) fills the box and searches once, when the session is
  // ready; a later reconnect keeps whatever the reader has searched since.
  let appliedQ: string | null = null;
  $effect(() => {
    void session.epoch;
    const params = new URLSearchParams(route.search);
    if (!session.client) return;
    let current = true;
    untrack(() => {
      inspection++; inspectAbort?.abort(); detail = null;
      const linked = params.get('q');
      if (linked && linked !== appliedQ) {
        appliedQ = linked;
        query = linked.slice(0, 256);
        commit();
      }
      void reload().then(() => {
        if (!trail.length && starts[0]) trail = [starts[0]]; // so Clear search has a folder to go back to
        const node = params.get('node'), observed = params.get('generation');
        if (current && node) void inspect(node, observed ?? undefined);
      });
    });
    return () => { current = false; inspection++; inspectAbort?.abort(); };
  });
</script>

<h1>Explorer</h1>
<p class="lede">
  A paged table over the {session.datasetClass ?? 'unknown'} dataset: the text equivalent of the maps. Names are plain text and hidden characters
  are shown as badges; an unknown size stays <span class="unknown">unknown</span>.
</p>

<section class="panel">
  <form class="bar form" onsubmit={(e) => { e.preventDefault(); search(); }}>
    <label class="field stack grow"><span class="lbl">Search names</span>
      <input type="search" placeholder="Search names" bind:value={query} bind:this={searchEl} maxlength="256" onkeydown={searchKeys} aria-describedby="find-hint find-notes" />
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

  <p id="find-hint" class="muted small hint">Filters in the box: <code>ext:pdf</code> <code>size:&gt;100mb</code> <code>modified:&lt;30d</code> (newer) <code>modified:&gt;1y</code> (older) <code>kind:dir</code>. The Extension, Kind and Size boxes fill in only what the query leaves out.</p>
  <div class="bar" role="group" aria-label="Cleanup searches">
    {#each PRESETS as p (p.label)}<button class="btn" type="button" title={p.query} onclick={() => preset(p.query)}>{p.label}</button>{/each}
  </div>
  <div id="find-notes">
    {#if plan.chips.length}
      <ul class="chips" aria-label="Search filters">{#each plan.chips as c, i (i)}<li class="chip">{c}</li>{/each}</ul>
    {/if}
    {#if plan.notices.length}
      <ul class="scope muted small" aria-label="Search notes">{#each plan.notices as n, i (i)}<li>{n}</li>{/each}</ul>
    {/if}
  </div>

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
    <p class="where">Search results {#if ran}for <q><VisibleName name={ran} /></q>{/if}</p>
    {#if scope && (scope.index_state !== 'current' || scope.partial || !scope.substring_searched)}
      <ul class="scope muted small" aria-label="Search coverage">
        {#if scope.index_state === 'building'}<li class="bad">A searched folder is still being scanned, so more results may appear.</li>{/if}
        {#if scope.index_state === 'stale'}<li class="bad">A searched folder's catalogue is stale or failed: results may be out of date.</li>{/if}
        {#if scope.partial}<li>Not every searched folder is fully scanned, so a missing result is not proof of absence.</li>{/if}
        {#if !scope.substring_searched}<li>Short text: one character searches whole names (or the name before a dot); two also search name and word starts. Matching words separately needs two characters per word for starts and three to match inside names.</li>{/if}
      </ul>
    {/if}
  {:else}
    <nav class="crumbs" aria-label="Location" bind:this={locationEl}>
      {#each trail as c, i (c.id)}
        {#if i > 0}<span aria-hidden="true">›</span>{/if}
        <button class="crumb" type="button" aria-current={i === trail.length - 1 ? 'location' : undefined} onclick={() => go(trail.slice(0, i + 1))}><VisibleName name={c.name} /></button>
      {/each}
    </nav>
  {/if}

  {#if error}
    <p class="bad" role="alert">{error} <button class="btn" type="button" onclick={reload}>Try again</button></p>
  {/if}
  <p id="explorer-notice" class="muted live" class:has={!!notice || !!gone} role="status" tabindex="-1" bind:this={noticeEl}>{#if gone}<q><VisibleName name={gone.lost} /></q> is no longer in the catalogue, so the view {#if gone.now}moved up to <q><VisibleName name={gone.now} /></q>, the nearest folder still there.{:else}went back to the top level.{/if}{:else}{notice}{/if}</p>

  <div class="bar" role="group" aria-label="Copy selection">
    <CopyTo items={selected} />
    <span class="muted num" role="status">{formatCount(selected.length)} selected across folders</span>
    {#if selected.length}<button class="btn" type="button" onclick={() => { selected = []; }}>Clear copy selection</button>{/if}
  </div>
  <div class="tbl-wrap" aria-busy={busy}>
    <table class="tbl" bind:this={tableEl}>
      <caption class="sr-only">{searching ? 'Search results' : 'Folder contents'}</caption>
      <thead>
        <tr>
          <th scope="col">Name</th><th scope="col">Kind</th>
          <th scope="col" class="r" class:basis={basis === 'logical'}>Logical</th>
          <th scope="col" class="r" class:basis={basis === 'allocated'}>Allocated</th>
          <th scope="col" class="r">Files</th><th scope="col">Modified</th><th scope="col">Notes</th><th scope="col">Inspect</th>
        </tr>
      </thead>
      <tbody>
        {#each rows as r (r.node_id)}
          <tr>
            <th scope="row">
              <input class="copy-select" type="checkbox" aria-label={`Select ${escapedName(r.name)} for copy`} checked={selected.some(i => i.node_id === r.node_id)}
                onchange={(e) => { selected = e.currentTarget.checked ? [...selected, { node_id: r.node_id, name: r.name }] : selected.filter(i => i.node_id !== r.node_id); }} />
              {#if r.kind === 'dir' && !searching && r.coverage !== 'denied'}
                <button class="crumb" type="button" onclick={() => go([...trail, { id: r.node_id, name: r.name }])}><VisibleName name={r.name} /></button>
              {:else if searching && marked(r)}
                <span>{#each marked(r) ?? [] as part, k (k)}{#if part.hit}<mark>{part.text}</mark>{:else}{part.text}{/if}{/each}</span>
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
              {#if found.get(r.node_id)}<span class="tag">{found.get(r.node_id)?.match_kind}</span>{/if}
              {#if hasHiddenCharacters(r.name)}<span class="tag warn">hidden characters</span>{/if}
              {#if r.coverage !== 'complete'}<span class="tag warn">{r.coverage}</span>{/if}
              {#each r.flags as f (f)}<span class="tag">{String(f).replaceAll('_', ' ')}</span>{/each}
            </td>
            <td><button class="btn" type="button" data-inspect onkeydown={rowKeys} aria-label={`Inspect ${escapedName(r.name)}`} onclick={() => void inspect(r.node_id)}>Inspect</button></td>
          </tr>
        {:else}
          <tr><td colspan="8" class="muted">{busy ? 'Loading' : error ? 'Nothing is shown because the last request failed.' : searching && budgetHit ? 'Nothing found yet: the engine stopped at its work budget before finding a match.' : searching && scope?.partial ? 'No matches in what is catalogued so far; coverage is partial, so this is not proof of absence.' : searching ? 'No matches.' : 'Nothing to show.'}</td></tr>
        {/each}
      </tbody>
    </table>
  </div>

  <div class="foot">
    <span class="muted num" aria-live="polite" tabindex="-1" bind:this={countEl}>Showing {formatCount(rows.length)} of {formatCount(total)}{#if budgetHit} · the engine stopped at its work budget, so more may exist{/if}</span>
    {#if next}<button class="btn" type="button" aria-disabled={busy} bind:this={moreEl} onclick={() => { if (!busy) void show(true); }}>Load more</button>{/if}
  </div>
</section>

<p class="live bad" role="alert">{inspectError}</p>
{#if detail}
  <section class="panel" aria-label="Node inspector">
    <h2 tabindex="-1" bind:this={inspector}><VisibleName name={detail.row.name} /></h2>
    <p class="muted">Logical size {formatBytes(detail.row.logical_bytes)} · {detail.row.kind}</p>
    <p class="muted small">Location: <VisibleName name={detail.display_path.text} />{#if detail.display_path.truncated} <span class="tag warn">path truncated</span>{/if}</p>
    <h3>Meaning thread</h3>
    <NodeMeaning nodeId={detail.row.node_id} />
    {#if detail.identity.durable_reference === 'available'}
      {#key detail.row.node_id}<AddToCollection nodeId={detail.row.node_id} expectedGeneration={detailGeneration} />{/key}
    {:else}<p class="muted">This node has no durable identity, so it cannot join a collection.</p>{/if}
    {#key detail.row.node_id}<FolderInsight rootId={detail.root_id} nodeId={detail.row.node_id} kind={detail.row.kind} />{/key}
  </section>
{/if}

<style>
  .copy-select { width: 24px; height: 24px; margin: 0 8px 0 0; vertical-align: middle; }
  .bar { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; margin-bottom: 12px; }
  .field { display: inline-flex; align-items: center; gap: 6px; }
  .stack { flex-direction: column; align-items: flex-start; gap: 2px; }
  .lbl { font-size: 0.8rem; color: var(--muted); }
  .form { align-items: flex-end; }
  .grow { flex: 1 1 14rem; }
  .grow input { width: 100%; }
  .starts { display: flex; flex-wrap: wrap; gap: 6px; margin-bottom: 8px; }
  .where { margin: 0 0 8px; }
  .hint { margin: 0 0 8px; }
  .chips { display: flex; flex-wrap: wrap; gap: 6px; margin: 0 0 8px; padding: 0; list-style: none; }
  .chip { padding: 2px 10px; border: 1px solid var(--accent); border-radius: 999px; font-size: 0.85rem; }
  .scope { margin: 0 0 8px; padding-left: 1.2rem; }
  mark { background: color-mix(in srgb, var(--accent) 25%, transparent); color: inherit; border-radius: 2px; }
  @media (forced-colors: active) { mark { background: Mark; color: MarkText; } }
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
