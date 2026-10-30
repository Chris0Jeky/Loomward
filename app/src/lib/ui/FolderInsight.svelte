<script lang="ts">
  // "About this folder": an explicit, read-only request (#877). Nothing is asked until Describe is pressed.
  import { INSIGHT_FOOTER, axisRows } from '../format/insight';
  import { formatTime } from '../format/time';
  import { FolderInsight } from '../stores/insight.svelte';
  import { session } from '../stores/session.svelte';
  import VisibleName from './VisibleName.svelte';

  let { rootId, nodeId }: { rootId: string; nodeId: string } = $props();
  const insight = new FolderInsight();
  // a different folder, or leaving the page, drops the answer and any request still in flight
  $effect(() => { void rootId; void nodeId; return () => insight.clear(); });

  const rows = $derived(insight.result ? axisRows(insight.result) : []);
  const describe = () => { if (session.client) void insight.describe(session.client, rootId, nodeId, (e) => session.handle(e)); };
</script>

<section class="insight" aria-labelledby="insight-title">
  <h3 id="insight-title">About this folder</h3>
  <button class="btn" type="button" disabled={insight.busy || !session.client} aria-describedby="insight-foot" onclick={describe}>Describe</button>
  <p class="live" role="status">{#if insight.busy}Describing…{/if}</p>
  {#if insight.message}<p class="bad" role="alert">{insight.message}</p>{/if}
  {#if insight.result}
    <dl class="axes">
      {#each rows as row (row.axis)}
        <div>
          <dt>{row.name}</dt>
          <dd class:unknown={!row.decided}>{row.text}{#if row.rules.length}<span class="rules">Rule: {row.rules.join(', ')}</span>{/if}</dd>
        </div>
      {/each}
    </dl>
    <ul class="lines">
      {#each insight.result.lines as line, i (i)}<li><VisibleName name={line.text} /></li>{/each}
    </ul>
    <p class="muted small">Read from the catalogue at {formatTime(insight.result.observed_at)}.</p>
  {/if}
  <p id="insight-foot" class="muted small">{INSIGHT_FOOTER}</p>
</section>

<style>
  .insight { margin-top: 16px; }
  .axes { margin: 12px 0 0; }
  .axes div { display: grid; grid-template-columns: 8.5rem minmax(0, 1fr); gap: 10px; padding: 6px 0; border-top: 1px solid var(--line); }
  .axes dt { color: var(--muted); }
  .axes dd { margin: 0; overflow-wrap: anywhere; }
  .rules { display: block; font-size: 0.8rem; color: var(--muted); }
  .lines { margin: 12px 0 0; padding-left: 1.2rem; display: grid; gap: 4px; font-size: 0.9rem; }
  .small { font-size: 0.8rem; }
  .bad { color: var(--danger); }
  @media (max-width: 30rem) { .axes div { grid-template-columns: minmax(0, 1fr); gap: 0; } }
</style>
