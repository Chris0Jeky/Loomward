<script module lang="ts">
  export const meta = { title: 'Companion', order: 60 };
</script>

<script lang="ts">
  import { tick as flush, untrack } from 'svelte';
  import { formatBytes, parseBytes } from '../../lib/format/bytes';
  import { escapedName, hasHiddenCharacters } from '../../lib/format/names';
  import { formatCount, formatTime } from '../../lib/format/time';
  import { session } from '../../lib/stores/session.svelte';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import type { ProcessExplanation, ProcessRow } from '../../lib/contracts.gen';
  import type { ProcessSort } from '../../lib/derived';
  import { ProcessTable } from './table.svelte';

  const SORTS: [ProcessSort, string][] = [
    ['private_desc', 'Private commit'], ['working_set_desc', 'Working set'], ['cpu_desc', 'CPU'], ['io_desc', 'Disk I/O'], ['gpu_desc', 'GPU memory'], ['name_asc', 'Name'],
  ];

  let explain = $state<{ row: ProcessRow; result: ProcessExplanation } | null>(null);
  let explainError = $state('');
  let aboutEl = $state<HTMLElement>();
  let tableEl = $state<HTMLElement>();

  const available = $derived(session.info?.capabilities.observation.process_observation === true);
  // WCAG 2.2.2: the 3 s polling can be paused, and a row the reader is working in does not move: a new sample
  // waits for focus to leave. Whether focus is in the table is read at each sample, not remembered from events,
  // so a table that unmounts or hides while focused (no focusout) cannot hold the rows for good.
  const table = new ProcessTable({
    client: () => session.client,
    available: () => available,
    handle: (e) => session.handle(e),
    holdsFocus: () => !!tableEl?.contains(document.activeElement),
  });

  $effect(() => {
    void session.epoch;
    if (!session.client || !available || table.paused) return;
    return untrack(() => table.run());
  });

  async function explainRow(row: ProcessRow) {
    const c = session.client;
    if (!c) return;
    explainError = '';
    explain = null;
    try {
      const { result } = await c.call('processes.explain', { process_ref: row.process_ref });
      explain = { row, result };
      // the panel appears below the table: move focus to it so the reader lands on the answer
      await flush();
      aboutEl?.focus();
    } catch (e) {
      explainError = session.handle(e);
    }
  }

  const pct = (f: number | null) => (f === null ? null : `${(f * 100).toFixed(1)}%`);
  const rate = (n: number | null) => (n === null ? null : `${formatBytes(Math.round(n).toString())}/s`);
  const list = $derived(table.list);
  const sample = $derived(table.sample);
  const budgets = $derived(table.budgets);
  const mem = $derived(sample?.system?.memory ?? null);
  const total = $derived(parseBytes(mem?.total_bytes));
  const avail = $derived(parseBytes(mem?.available_bytes));
  const inUse = $derived(total !== null && avail !== null && avail <= total ? total - avail : null);
  const share = (part: bigint | null, whole: bigint | null) => (part === null || whole === null || whole === 0n ? null : Number((part * 1000n) / whole) / 10);
  const commit = $derived(parseBytes(mem?.commit_bytes));
  const commitLimit = $derived(parseBytes(mem?.commit_limit_bytes));
  const pools = $derived(new Map((sample?.engine?.pools ?? []).map((p) => [p.pool, p])));
</script>

{#snippet B(v: string | null, why: string)}{#if v === null}<span class="unknown">unknown<span class="sr-only"> ({why})</span></span>{:else}{formatBytes(v)}{/if}{/snippet}
{#snippet P(v: string | null, why: string)}{#if v === null}<span class="unknown">unknown<span class="sr-only"> ({why})</span></span>{:else}{v}{/if}{/snippet}

<h1>Companion</h1>
<p class="simbanner" role="note"><strong>Observation only.</strong> Loomward reads what the system reports about memory and processes. This page has no control that acts on any process.</p>

{#if !available}
  <section class="panel" role="status">
    <h2>Process observation is not available</h2>
    <p class="muted">This session does not report process observation, so nothing is shown here and nothing is guessed.</p>
  </section>
{:else}
  <p class="bad live" class:has={!!table.error} role="alert">{table.error}</p>

  <section class="panel" aria-labelledby="h-mem">
    <h2 id="h-mem">Memory</h2>
    {#if mem}
      <div class="memrow">
        <div>
          <h3>Memory ledger</h3>
          <div class="bar" role="img" aria-label={`In use ${formatBytes(inUse?.toString() ?? null)} of ${formatBytes(mem.total_bytes)}`}>
            {#if share(inUse, total) !== null}<div class="fill" style={`width:${share(inUse, total)}%`}></div>{/if}
          </div>
          <div class="tbl-wrap">
          <table class="tbl ledger">
            <caption class="sr-only">Physical memory ledger</caption>
            <thead><tr><th scope="col">Category</th><th scope="col" class="r">Bytes</th><th scope="col" class="r">Share</th><th scope="col">Source</th></tr></thead>
            <tbody>
              <tr><th scope="row">Total</th><td class="r num">{formatBytes(mem.total_bytes)}</td><td class="r num">100%</td><td class="muted">reported</td></tr>
              <tr><th scope="row">In use</th><td class="r num">{@render B(inUse?.toString() ?? null, 'needs total and available')}</td><td class="r num">{share(inUse, total) ?? 'unknown'}{share(inUse, total) === null ? '' : '%'}</td><td class="muted">derived: total minus available</td></tr>
              <tr><th scope="row">Modified</th><td class="r num">{@render B(null, 'not reported')}</td><td class="r num"><span class="unknown">unknown</span></td><td class="muted">not reported by the engine</td></tr>
              <tr><th scope="row">Standby</th><td class="r num">{@render B(null, 'not reported')}</td><td class="r num"><span class="unknown">unknown</span></td><td class="muted">not reported by the engine</td></tr>
              <tr><th scope="row">Free</th><td class="r num">{@render B(null, 'not reported')}</td><td class="r num"><span class="unknown">unknown</span></td><td class="muted">not reported by the engine</td></tr>
              <tr><th scope="row">Available</th><td class="r num">{@render B(mem.available_bytes, 'not reported')}</td><td class="r num">{share(avail, total) ?? 'unknown'}{share(avail, total) === null ? '' : '%'}</td><td class="muted">standby plus free, reported as one figure</td></tr>
            </tbody>
          </table>
          </div>
        </div>
        <div>
          <h3>Commit charge <span class="muted small">(separate from the ledger)</span></h3>
          <div class="bar commit" role="img" aria-label={`Commit ${formatBytes(mem.commit_bytes)} of limit ${formatBytes(mem.commit_limit_bytes)}`}>
            {#if share(commit, commitLimit) !== null}<div class="fill" style={`width:${share(commit, commitLimit)}%`}></div>{/if}
          </div>
          <p class="num">{formatBytes(mem.commit_bytes)} committed of a {formatBytes(mem.commit_limit_bytes)} limit{#if share(commit, commitLimit) !== null} · {share(commit, commitLimit)}%{/if}</p>
          <p class="muted small">Commit is memory promised to processes and backed by RAM or the page file. It is not memory in use, and it can be larger than the ledger's total.</p>
          <p class="num">Memory load {pct(mem.load_fraction) ?? 'unknown'} · CPU busy {pct(sample?.system?.cpu.busy_fraction ?? null) ?? 'unknown (needs two samples)'} across {formatCount(sample?.system?.cpu.logical_cpus ?? null)} logical CPUs</p>
          <p class="muted small">Sample {formatCount(sample?.sample_seq)} at {formatTime(sample?.observed_at)}</p>
        </div>
      </div>
    {:else}<p class="muted">Waiting for the first sample.</p>{/if}
  </section>

  <section class="panel" aria-labelledby="h-proc">
    <h2 id="h-proc">Processes</h2>
    <div class="ctl">
      <button type="button" class="btn" onclick={() => (table.paused = !table.paused)}>{table.paused ? 'Resume updates' : 'Pause updates'}</button>
      <label class="field">Largest by
        <select bind:value={table.sort} onchange={() => void table.reorder()}>{#each SORTS as [id, label] (id)}<option value={id}>{label}</option>{/each}</select>
      </label>
      <label class="field">Show
        <select bind:value={table.limit} onchange={() => void table.reorder()}>{#each [10, 20, 50, 100] as n (n)}<option value={n}>top {n}</option>{/each}</select>
      </label>
      {#if list}
        <span class="muted num" aria-live="polite">
          Showing {formatCount(list.rows.length)} of {formatCount(list.observed_count)} observed ·
          {formatCount(list.denied_count)} could not be read (their figures are unknown, not zero){list.truncated ? ' · the rest are not listed' : ''}
        </span>
      {/if}
    </div>
    <!-- persistent status regions: text put into a region that is already in the page is announced, a region inserted already filled often is not -->
    <p id="proc-paused" class="warn small live" class:has={table.paused} role="status">{table.paused ? (list ? `Paused: the table keeps sample ${formatCount(list.sample_seq)}.` : 'Paused before the first sample arrived.') : ''}</p>
    {#if list}
      <p id="proc-waiting" class="muted small live" class:has={!!table.waiting} role="status">{table.waiting ? 'A newer sample is waiting; it appears when focus leaves the table, so rows do not move under it.' : ''}</p>
      <div class="tbl-wrap" bind:this={tableEl} onfocusout={(e) => { if (!e.currentTarget.contains(e.relatedTarget as Node | null)) table.release(); }}>
        <table class="tbl procs">
          <caption class="sr-only">Top processes by {SORTS.find(([id]) => id === table.sort)?.[1]}</caption>
          <thead>
            <tr>
              <th scope="col">Process</th><th scope="col" class="r">Private commit</th><th scope="col" class="r">Private working set</th><th scope="col" class="r">Working set</th>
              <th scope="col" class="r">CPU</th><th scope="col" class="r">Read</th><th scope="col" class="r">Write</th><th scope="col" class="r">GPU memory</th>
            </tr>
          </thead>
          <tbody>
            {#each list.rows as r (r.process_ref)}
              {@const denied = r.access === 'denied'}
              <tr class:sel={explain?.row.process_ref === r.process_ref}>
                <th scope="row">
                  <button class="link" type="button" onclick={() => void explainRow(r)} aria-label={`Explain ${escapedName(r.name)}, process ${r.pid}`}><VisibleName name={r.name} /></button>
                  <span class="muted small num">pid {r.pid}</span>
                  <div class="muted small">Observed <time datetime={r.observed_at}>{r.observed_at}</time></div>
                  {#if r.loomward_owned}<span class="tag">Loomward</span>{/if}
                  {#if hasHiddenCharacters(r.name)}<span class="tag warn">hidden characters</span>{/if}
                  {#if r.access !== 'full'}<span class="tag warn">{r.access === 'denied' ? 'access denied' : 'limited access'}</span>{/if}
                  {#if denied}<div class="muted small">Windows would not describe this process: every figure is unknown.</div>{/if}
                </th>
                <td class="r num">{@render B(r.private_commit_bytes, denied ? 'access denied' : 'not reported')}</td>
                <td class="r num">{@render B(r.private_working_set_bytes, denied ? 'access denied' : 'not reported for this process')}</td>
                <td class="r num">{@render B(r.working_set_bytes, denied ? 'access denied' : 'not reported')}</td>
                <td class="r num">{@render P(pct(r.cpu_fraction), denied ? 'access denied' : 'needs two samples')}</td>
                <td class="r num">{@render P(rate(r.io_read_bytes_per_s), denied ? 'access denied' : 'needs two samples')}</td>
                <td class="r num">{@render P(rate(r.io_write_bytes_per_s), denied ? 'access denied' : 'needs two samples')}</td>
                <td class="r num">{@render B(r.gpu_dedicated_bytes, denied ? 'access denied' : 'no GPU counter for this process')}</td>
              </tr>
            {:else}
              <tr><td colspan="8" class="muted">No processes reported.</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
      <p class="muted small">Rows from sample {formatCount(list.sample_seq)}. {list.note} CPU is a share of the whole machine. Working sets include shared pages, so do not add rows up. Private commit is what a process has promised, not what is resident. GPU memory is per process and is never summed into an adapter total.</p>
    {:else}<p class="muted">Waiting for the first sample.</p>{/if}

    <p class="bad live" class:has={!!explainError} role="alert">{explainError}</p>
    {#if explain}
      <div class="explain" role="region" aria-label="Explanation">
        <h3 tabindex="-1" bind:this={aboutEl}>About <VisibleName name={explain.row.name} /></h3>
        <p>{explain.result.summary}</p>
        <ul class="list">{#each explain.result.facts as f (f.code)}<li>{f.text}</li>{/each}</ul>
        {#each explain.result.caveats as cv (cv)}<p class="muted small">{cv}</p>{/each}
        {#if explain.result.available_actions.length}
          <p class="bad" role="alert">The engine offered an action, which this build ignores: Loomward v0.3 is observation only.</p>
        {:else}
          <p class="muted small">Loomward offers no action on any process. Observed {formatTime(explain.result.observed_at)}; this is rule-based text, not advice.</p>
        {/if}
      </div>
    {/if}
  </section>

  <section class="panel" aria-labelledby="h-own">
    <h2 id="h-own">Loomward itself</h2>
    {#if sample?.engine}
      {@const e = sample.engine}
      <dl class="kv">
        <dt>Private commit</dt><dd class="num">{formatBytes(e.private_commit_bytes)}</dd>
        <dt>Working set</dt><dd class="num">{formatBytes(e.working_set_bytes)}</dd>
        <dt>CPU</dt><dd class="num">{pct(e.cpu_fraction) ?? 'unknown (needs two samples)'}</dd>
        <dt>Threads</dt><dd class="num">{formatCount(e.threads)}</dd>
      </dl>
    {:else}<p class="muted">The engine reported nothing about itself in this sample.</p>{/if}
    {#if budgets}
      <h3>Worker budgets</h3>
      <div class="tbl-wrap">
        <table class="tbl">
          <caption class="sr-only">Loomward's own worker pools</caption>
          <thead><tr><th scope="col">Pool</th><th scope="col" class="r">Workers now</th><th scope="col" class="r">Default</th><th scope="col" class="r">Limit</th><th scope="col" class="r">Busy</th><th scope="col" class="r">Queue</th></tr></thead>
          <tbody>
            {#each budgets.pools as b (b.pool)}
              {@const use = pools.get(b.pool)}
              <tr>
                <th scope="row">{b.pool.replaceAll('_', ' ')}</th>
                <td class="r num">{b.current_workers}</td><td class="r num">{b.default_workers}</td><td class="r num">{b.max_workers}</td>
                <td class="r num">{use ? use.busy_workers : 'unknown'}</td>
                <td class="r num">{use ? `${use.queue_depth} / ${use.queue_capacity}` : `unknown / ${b.queue_capacity}`}</td>
              </tr>
            {/each}
            <tr>
              <th scope="row">teacher</th>
              <td colspan="5">at most {budgets.teacher.max_in_flight} request at a time, {budgets.teacher.timeout_s} s timeout, memory limit {formatBytes(budgets.teacher.memory_limit_bytes)}
                ({budgets.teacher.enforcement === 'job_object' ? 'enforced by a job object' : 'not enforced on this system'})</td>
            </tr>
          </tbody>
        </table>
      </div>
      <p class="muted small">{budgets.enforcement_note}</p>
    {/if}
  </section>
{/if}

<style>
  .simbanner { border: 1px dashed var(--permission); border-radius: var(--radius); padding: 8px 12px; background: var(--surface); }
  .bad { color: var(--danger); }
  .warn { color: var(--warn); }
  .small { font-size: 0.8rem; }
  .memrow { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(300px, 100%), 1fr)); gap: 20px; }
  .memrow > div { min-width: 0; }
  .bar { height: 14px; border: 1px solid var(--control-line); border-radius: 7px; background: var(--raised); overflow: hidden; margin-bottom: 10px; }
  .fill { height: 100%; background: var(--residency); opacity: 0.8; }
  .commit .fill { background: repeating-linear-gradient(135deg, var(--permission) 0 4px, color-mix(in srgb, var(--permission) 45%, transparent) 4px 8px); }
  .ledger th[scope='row'] { text-align: left; font-weight: 600; text-transform: none; letter-spacing: 0; font-size: inherit; color: var(--text); }
  .ctl { display: flex; flex-wrap: wrap; gap: 10px 20px; align-items: center; margin-bottom: 12px; }
  .field { display: inline-flex; align-items: center; gap: 6px; }
  .link { font: inherit; color: var(--accent); background: none; border: 0; padding: 0; cursor: pointer; text-align: left; }
  .procs tr.sel td { background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .explain { margin-top: 16px; padding: 12px; border: 1px solid var(--line); border-left: 3px solid var(--meaning); border-radius: var(--radius); background: var(--bg); }
  .list { margin: 0 0 8px; padding-left: 1.1em; }
  .kv { display: grid; grid-template-columns: max-content 1fr; gap: 4px 20px; margin: 0 0 12px; }
  .kv dt { color: var(--muted); }
  .kv dd { margin: 0; }
</style>
