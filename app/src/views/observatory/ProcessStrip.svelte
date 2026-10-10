<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import { escapedName } from '../../lib/format/names';
  import type { ProcessReading } from './feed';
  import { observationAge, processValue, stripRows, stripStatus, type ProcessLens } from './processes';

  let { reading, paused, lens = $bindable<ProcessLens>('cpu_desc'), onchange }: {
    reading: ProcessReading | null; paused: boolean; lens?: ProcessLens; onchange: () => void;
  } = $props();
  let shown = $state<ProcessReading | null>(null);
  let waiting = $state<ProcessReading | null>(null);
  let rowsEl = $state<HTMLOListElement>();
  let now = $state(Date.now());
  const label = (sort: ProcessLens) => sort === 'cpu_desc' ? 'machine CPU' : 'private commit';
  const rows = $derived(stripRows(shown?.list?.rows ?? [], shown?.lens ?? lens));
  const status = $derived(`${paused ? 'Paused; showing retained observations. Resume updates to change the lens. ' : ''}${shown?.dataset === 'synthetic' ? 'Synthetic process observations. ' : ''}${stripStatus(shown?.list ?? null, shown?.error ?? '')}${waiting ? ' A newer observation is waiting until focus leaves the strip.' : ''}`);

  $effect(() => {
    const next = reading;
    untrack(() => {
      now = Date.now();
      if (shown && next?.list && rowsEl?.contains(document.activeElement)) waiting = next;
      else { shown = next; waiting = null; }
    });
  });

  function release() { if (waiting) { shown = waiting; waiting = null; } }

  onMount(() => {
    const timer = setInterval(() => {
      if (document.hidden) return;
      now = Date.now();
      if (!rowsEl?.contains(document.activeElement)) release();
    }, 1000);
    return () => clearInterval(timer);
  });
</script>

<section class="process-strip" aria-labelledby="strip-h">
  <div class="strip-top">
    <h2 id="strip-h">Processes</h2>
    <label>Process lens
      <select bind:value={lens} {onchange} disabled={paused}>
        <option value="cpu_desc">CPU</option>
        <option value="private_desc">Private commit</option>
      </select>
    </label>
  </div>
  <p id="strip-status" class="strip-note" role="status" aria-live="polite" aria-atomic="true">{status}</p>
  {#if shown?.note}<p class="strip-note">{shown.note}</p>{/if}
  {#if rows.length}
    <ol bind:this={rowsEl} aria-label={`Top processes by ${label(shown?.lens ?? lens)}`}
      onfocusout={(e) => { if (!e.currentTarget.contains(e.relatedTarget as Node | null)) release(); }}>
      {#each rows as row (row.process_ref)}
        <li>
          <a href={`#/companion?process=${encodeURIComponent(row.process_ref)}`} aria-label={`Explain ${escapedName(row.name)}, process ${row.pid}, in Companion`}>
            <span class="process-name"><VisibleName name={row.name} /></span>
            <span class="process-metric num">{processValue(row, shown?.lens ?? lens)} <span class="metric-label">{label(shown?.lens ?? lens)}</span></span>
          </a>
          <span class="process-detail num">pid {row.pid} · <time datetime={row.observed_at}>{observationAge(row.observed_at, now)}</time></span>
          {#if row.access !== 'full'}<span class="process-detail">{row.access === 'denied' ? 'Access denied · readings unknown' : 'Limited access'}</span>{/if}
        </li>
      {/each}
    </ol>
  {/if}
  <p class="strip-note">Observation only. CPU is a share of the whole machine; private commit is promised memory, not residency.</p>
</section>

<style>
  .process-strip { min-width: 0; border-block: 1px solid var(--line-strong); padding-block: 12px; }
  .strip-top { display: flex; flex-wrap: wrap; align-items: baseline; justify-content: space-between; gap: 8px 16px; }
  h2 { margin: 0; }
  label { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; font-size: 0.8rem; color: var(--muted); }
  select { min-width: 24px; min-height: 28px; max-width: 100%; font: inherit; color: var(--text); background: var(--surface); border: 1px solid var(--control-line); border-radius: var(--radius); padding: 2px 6px; }
  .strip-note { font-size: 0.78rem; color: var(--muted); margin: 8px 0 0; overflow-wrap: anywhere; }
  ol { list-style: none; padding: 0; margin: 10px 0; }
  li { padding: 6px 0; border-bottom: 1px solid var(--line); min-width: 0; }
  a { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 2px 12px; min-width: 24px; min-height: 28px; align-items: baseline; padding-block: 2px; color: var(--text); text-decoration: underline; text-decoration-color: var(--control-line); text-underline-offset: 3px; }
  a:hover { color: var(--accent); text-decoration-color: currentColor; }
  .process-name { min-width: 0; overflow-wrap: anywhere; font-size: 0.85rem; }
  .process-metric { font-size: 0.82rem; }
  .metric-label { color: var(--muted); font-size: 0.72rem; }
  .process-detail { display: block; font-size: 0.72rem; color: var(--muted); overflow-wrap: anywhere; }
  @media (forced-colors: active) {
    .process-strip, li { border-color: CanvasText; }
    a { color: LinkText; text-decoration-color: LinkText; }
    select { border-color: ButtonText; }
  }
</style>
