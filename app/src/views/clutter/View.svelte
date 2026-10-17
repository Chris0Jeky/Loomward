<script module lang="ts">
  export const meta = { title: 'Developer clutter', order: 36 };
</script>
<script lang="ts">
  import { untrack } from 'svelte';
  import { session } from '../../lib/stores/session.svelte';
  import { escapedName } from '../../lib/format/names';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import type { DeveloperClutterReport, Root } from '../../lib/contracts.gen';
  import { allocatedText, countText, COVERAGE_WORDS, KIND_WORDS, outcome, PROVIDER_WORDS, sizeText, TOPIC_WORDS } from './model';

  let roots = $state<Root[]>([]);
  let selected = $state('');
  let report = $state<DeveloperClutterReport | null>(null);
  let loading = $state(true);
  let error = $state('');
  let generation = 0;

  // A session reads its own class of root only: lab roots when synthetic, folders you granted when personal (q-10).
  const origin = $derived(session.datasetClass === 'personal' ? 'owner_granted' : 'lab_generated');
  const usable = $derived(roots.filter((r) => r.grant_state === 'active' && (r.origin === origin || r.origin === 'fixture')));
  const shape = $derived(report ? outcome(report) : null);

  /** Reads the report for `id`; a reply for an older request or root is dropped, so it cannot show another root's folders. */
  async function read(id: string): Promise<void> {
    const c = session.client;
    if (!c || !id) { report = null; return; }
    const mine = ++generation;
    loading = true; error = '';
    try {
      const { result } = await c.call('clutter.developer', { root_id: id, limit: 50 });
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
      loading = true;
      try {
        const { result } = await c.call('roots.list', {});
        if (!current) return;
        roots = result.roots;
        const pool = result.roots.filter((r) => r.grant_state === 'active' && (r.origin === origin || r.origin === 'fixture'));
        if (!pool.some((r) => r.root_id === selected)) selected = pool[0]?.root_id ?? '';
        await untrack(() => read(selected));
      } catch (e) { if (current) { error = session.handle(e); loading = false; } }
    })();
    return () => { current = false; generation++; };
  });
</script>

<h1>Developer clutter</h1>
<p class="lede">Build outputs, installed dependencies and caches that sit beside a project file, largest first. <strong>You could remove these; Loomward will not.</strong></p>
<p class="note">Advice from folder names Loomward already recorded. It does not open the project, check that anything is unused or promise a safe removal: a folder with a familiar name can still hold work that exists nowhere else. Sizes are what was recorded, not space you will get back.</p>

<section class="panel" aria-labelledby="clutter-root-h">
  <h2 id="clutter-root-h">Root</h2>
  <div class="controls">
    <div class="lw-field">
      <label class="lbl" for="clutter-root">Granted root</label>
      <select id="clutter-root" bind:value={selected} disabled={loading || !usable.length} onchange={(e) => { selected = e.currentTarget.value; void read(selected); }}>
        {#each usable as r (r.root_id)}<option value={r.root_id}>{escapedName(r.display_path.text)}</option>{/each}
      </select>
    </div>
    <button class="btn" disabled={loading || !selected} onclick={() => void read(selected)}>Look again</button>
  </div>
  {#if !loading && !usable.length && !error}<p class="empty">No granted root is available. Add and scan a folder first.</p>{/if}
  <p id="clutter-error" class="live bad" class:has={!!error} role="alert">{error}</p>
</section>

<section class="panel" aria-labelledby="clutter-found" aria-busy={loading}>
  <h2 id="clutter-found">Folders found{report && shape === 'list' ? ` (${countText(report)})` : ''}</h2>
  {#if !report}
    <p class="empty">{loading ? 'Looking at the recorded folders' : error ? 'Developer clutter unavailable.' : 'Choose a root to look at.'}</p>
  {:else}
    {#if shape === 'budget'}
      <p class="note warn" role="status"><strong>The search ran out of budget.</strong> {report.budget_note ?? ''} This is not an empty result: try again after a narrower scan.</p>
    {:else if report.partial}
      <p class="note warn" role="status"><strong>This root is not fully known.</strong> It is still scanning, stale or only partly covered, or a size is unknown, so projects may be missing and sizes may be lower than the truth.</p>
    {/if}
    {#if shape === 'none'}
      <p class="empty">No build output or cache folder is recorded beside a project file in this root.</p>
    {:else if shape === 'none-partial'}
      <p class="empty">None found in what has been scanned. That is not a clean result.</p>
    {:else if shape === 'list'}
      <div class="tbl-wrap">
        <table class="tbl">
          <caption class="sr-only">Candidate folders, largest recorded size first. Advice only: nothing here can remove them.</caption>
          <thead><tr><th scope="col">Folder</th><th scope="col">Kind</th><th scope="col" class="r">Size</th><th scope="col">Coverage</th><th scope="col">If you removed it</th></tr></thead>
          <tbody>
            {#each report.candidates as c (c.node_id)}
              <tr>
                <th scope="row"><VisibleName name={c.name} /><small>beside <VisibleName name={c.anchor_name} />, {PROVIDER_WORDS[c.provider]}</small></th>
                <td>{KIND_WORDS[c.kind]}<small>{c.reason}</small></td>
                <td class="r num" class:unknown={c.logical_bytes === null}>{sizeText(c)}<small>{allocatedText(c)}</small></td>
                <td>{COVERAGE_WORDS[c.coverage] ?? c.coverage}</td>
                <td>{c.regeneration_hint}<small>Rebuild cost: not assessed yet. {c.uncertainty}</small></td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      {#if report.truncated}<p class="muted small">More folders exist than are shown. Only the largest {report.candidates.length} are listed.</p>{/if}
    {/if}
  {/if}
</section>

{#if report}
  <section class="panel" aria-labelledby="clutter-not-assessed">
    <h2 id="clutter-not-assessed">Not assessed yet</h2>
    <p class="muted">These parts of developer clutter have no source of evidence yet, so nothing is shown for them rather than a guess.</p>
    <ul class="plain">
      {#each report.not_assessed as n (n.topic)}
        <li><span class="tag unk">Not assessed yet</span> <strong>{TOPIC_WORDS[n.topic]}</strong> <span class="muted">{n.note}</span></li>
      {/each}
    </ul>
  </section>
{/if}

<style>
  .plain { list-style: none; margin: 0; padding: 0; display: grid; gap: var(--sp-2); }
</style>
