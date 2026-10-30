<script lang="ts">
  import { tick } from 'svelte';
  import type { Root } from '../../lib/contracts.gen';
  import { session } from '../../lib/stores/session.svelte';
  import { Covers } from '../../lib/stores/covers.svelte';
  import { escapedName } from '../../lib/format/names';
  import { formatBytes } from '../../lib/format/bytes';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import {
    BYTE_CHOICES, COVERS_CAVEAT, COVERS_EXPLAINER, SECOND_CHOICES, explorerLink, headline, reasonText, statusText, totalRows,
  } from './covers-model';

  let { roots, origin, loading }: { roots: Root[]; origin: string; loading: boolean } = $props();

  const covers = new Covers();
  const usable = $derived(roots.filter((r) => r.grant_state === 'active' && r.origin === origin));
  let a = $state('');
  let b = $state('');
  let bytes = $state<string>(BYTE_CHOICES[0].value);
  let secs = $state<number>(SECOND_CHOICES[0].value);
  let now = $state(Date.now());
  let resultHeading = $state<HTMLElement>();

  const job = $derived(covers.job);
  const report = $derived(covers.report);
  const active = $derived(covers.active);
  const summary = $derived(report ? headline(report) : null);
  const status = $derived(covers.problem || statusText(job, report, covers.pending, now, secs));
  const nameOf = (id: string) => escapedName(roots.find((r) => r.root_id === id)?.display_path.text ?? id);
  const canStart = $derived(!loading && !!a && !!b && a !== b && !active && !covers.pending);

  // Two different folders by default; keep a choice that is still usable.
  $effect(() => {
    if (!usable.some((r) => r.root_id === a)) a = usable[0]?.root_id ?? '';
    if (!usable.some((r) => r.root_id === b) || b === a) b = usable.find((r) => r.root_id !== a)?.root_id ?? '';
  });
  $effect(() => session.listen((event) => covers.onEvent(event)));
  $effect(() => {
    void session.epoch;
    if (session.client) void covers.sync();
  });
  $effect(() => {
    const observed = covers.job;
    if (!observed || !active) return;
    const timer = setInterval(() => { now = Date.now(); void covers.refresh(); }, 1000);
    return () => clearInterval(timer);
  });
  let lastActive = false;
  $effect(() => {
    const finished = lastActive && !active;
    lastActive = active;
    if (finished) void tick().then(() => resultHeading?.focus());
  });
</script>

<section class="panel" aria-labelledby="covers-start">
  <h2 id="covers-start">Is this folder already in another?</h2>
  <p class="note">{COVERS_EXPLAINER}</p>
  <div class="controls">
    <div class="lw-field">
      <label class="lbl" for="covers-a">Folder to check (A)</label>
      <select id="covers-a" bind:value={a} disabled={loading || active || covers.pending}>
        {#each usable as r (r.root_id)}<option value={r.root_id}>{escapedName(r.display_path.text)}</option>{/each}
      </select>
    </div>
    <div class="lw-field">
      <label class="lbl" for="covers-b">Look for its files in (B)</label>
      <select id="covers-b" bind:value={b} disabled={loading || active || covers.pending}>
        {#each usable as r (r.root_id)}<option value={r.root_id}>{escapedName(r.display_path.text)}</option>{/each}
      </select>
    </div>
    <div class="lw-field">
      <label class="lbl" for="covers-bytes">Read up to</label>
      <select id="covers-bytes" bind:value={bytes} disabled={active || covers.pending}>
        {#each BYTE_CHOICES as c (c.value)}<option value={c.value}>{c.label}</option>{/each}
      </select>
    </div>
    <div class="lw-field">
      <label class="lbl" for="covers-secs">For up to</label>
      <select id="covers-secs" bind:value={secs} disabled={active || covers.pending}>
        {#each SECOND_CHOICES as c (c.value)}<option value={c.value}>{c.label}</option>{/each}
      </select>
    </div>
    <button class="btn primary" disabled={!canStart} onclick={() => covers.start(a, b, { bytes, secs })}>Start check</button>
    <button class="btn" disabled={!active || job?.state === 'cancel_requested' || covers.pending} onclick={() => covers.cancel()}>Cancel check</button>
  </div>
  {#if !loading && usable.length < 2}<p class="empty">This check needs two granted folders. <a href="#/health">Add a folder in Grants &amp; health</a>.</p>
  {:else if a && a === b}<p class="empty">Choose two different folders.</p>{/if}
  <p class="live" class:has={!!status} role="status" aria-live="polite">{status}</p>
</section>

<section class="panel" aria-labelledby="covers-results">
  <h2 id="covers-results" tabindex="-1" bind:this={resultHeading}>Coverage result</h2>
  {#if job?.error}<p class="bad" role="alert">{job.error.message}</p>{/if}
  {#if report && summary}
    <p class="covers-line"><span class="muted">A:</span> <VisibleName name={nameOf(report.a_root_id)} /> · <span class="muted">B:</span> <VisibleName name={nameOf(report.b_root_id)} /></p>
    <p class="covers-verdict" class:partial={summary.tone === 'partial'}><strong>{summary.heading}</strong> {summary.detail}</p>
    <p class="covers-evidence">Bytes read <strong class="num">{formatBytes(report.bytes_read)}</strong> · Outcome <strong class:warn={report.outcome === 'partial'}>{report.outcome}</strong></p>
    <div class="tbl-wrap"><table class="tbl"><caption class="sr-only">Files and bytes of the first folder by verdict</caption>
      <thead><tr><th scope="col">Verdict</th><th scope="col">Files</th><th scope="col">Bytes</th></tr></thead>
      <tbody>{#each totalRows(report) as row (row.label)}<tr><th scope="row">{row.label}</th><td class="num">{row.files}</td><td class="num">{row.bytes}</td></tr>{/each}</tbody>
    </table></div>
    <p class="muted">Files that were covered are counted, not listed.</p>

    <h3>Not found in the second folder</h3>
    {#if report.not_covered.length}
      <div class="tbl-wrap"><table class="tbl"><caption class="sr-only">Largest files of the first folder without an equal-content file in the second</caption>
        <thead><tr><th scope="col">File in A</th><th scope="col">Size</th><th scope="col">Catalogue</th></tr></thead>
        <tbody>{#each report.not_covered as item, i (i)}
          <tr>
            <th scope="row"><VisibleName name={item.path} /></th>
            <td class="num">{formatBytes(item.bytes)}</td>
            <td>{#if explorerLink(item.node_id)}<a href={explorerLink(item.node_id)} aria-label={`Open in Explorer: ${item.path}`}>Open in Explorer</a>{:else}Not in the catalogue yet{/if}</td>
          </tr>{/each}</tbody>
      </table></div>
      {#if report.not_covered_truncated}<p class="muted">Showing the largest {report.not_covered.length} of {report.totals.not_covered.files.toLocaleString('en-US')} files; the totals above count all of them.</p>{/if}
    {:else}<p class="empty">{report.outcome === 'partial' ? 'None were proven missing. Files that could not be judged are listed below, so this is not a clean answer.' : 'None.'}</p>{/if}

    <h3>Could not be judged</h3>
    {#if report.unknown.length}
      <div class="tbl-wrap"><table class="tbl"><caption class="sr-only">Files of the first folder whose verdict is unknown</caption>
        <thead><tr><th scope="col">File in A</th><th scope="col">Size</th><th scope="col">Why</th><th scope="col">Catalogue</th></tr></thead>
        <tbody>{#each report.unknown as item, i (i)}
          <tr>
            <th scope="row"><VisibleName name={item.path} /></th>
            <td class="num">{formatBytes(item.bytes)}</td>
            <td>{reasonText(item.reason)} ({item.side === 'a' ? 'first folder' : 'second folder'})</td>
            <td>{#if explorerLink(item.node_id)}<a href={explorerLink(item.node_id)} aria-label={`Open in Explorer: ${item.path}`}>Open in Explorer</a>{:else}Not in the catalogue yet{/if}</td>
          </tr>{/each}</tbody>
      </table></div>
      {#if report.unknown_truncated}<p class="muted">Showing the largest {report.unknown.length} of {report.totals.unknown.files.toLocaleString('en-US')} files; the totals above count all of them.</p>{/if}
    {:else}<p class="empty">Nothing was left unknown.</p>{/if}
    <p class="muted">{COVERS_CAVEAT}</p>
  {:else}<p class="empty">{active ? 'The check is running; the result appears when it ends.' : job?.state === 'completed' ? '' : 'Start a check to see which files of one folder are not in another.'}</p>{/if}
</section>

<style>
  h3 { margin: var(--sp-5) 0 var(--sp-2); font-size: 1rem; }
  .covers-verdict, .covers-evidence, .covers-line { overflow-wrap: anywhere; }
  .covers-evidence strong { font-family: var(--num-font); }
  .covers-verdict.partial strong { color: var(--warn); }
</style>
