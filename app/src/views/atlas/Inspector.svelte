<script lang="ts">
  // Evidence for one node: sizes, counts and the three threads, with every unknown named.
  // Describes only: there is no effect control here, by design.
  import { formatBytes } from '../../lib/format/bytes';
  import { formatCount, formatTime } from '../../lib/format/time';
  import type { Basis } from '../../lib/types';
  import type { NodeInfo, Palette } from '../../../viz/types.js';
  import { drawSwatch, zeroCounts } from '../../../viz/woven-treemap.js';
  import { formatApprox } from './shared.svelte';

  interface Props {
    node: NodeInfo | null;
    status: string;
    basis: Basis;
    provisional: boolean;
    palette: Palette;
    path: string[];
  }
  let { node, status, basis, provisional, palette, path }: Props = $props();

  let swatch = $state<HTMLCanvasElement>();
  $effect(() => {
    const n = node, p = palette;
    if (swatch && n?.src) drawSwatch(swatch, p, { threads: n.src.threads });
  });

  const MEANING: Record<string, string> = {
    labelled: 'Labelled', suggested: 'Suggested, not confirmed', mixed: 'Mixed below here', none: 'No meaning yet', pending: 'Pending', unknown: 'Unknown',
  };
  const PERM: Record<string, string> = {
    granted: 'Observed under the grant', partial: 'Partly observable', excluded: 'Excluded from observation', denied: 'Access denied', revoked: 'Grant revoked', unknown: 'Unknown',
  };
  const TIER = ['hot', 'warm', 'cold'];
  const human = (s: string | null | undefined) => (s ? s.replaceAll('_', ' ') : '');

  const z = $derived(node ? zeroCounts(node) : { denied: 0, unmeasured: 0, empty: 0 });
  const src = $derived(node?.src ?? null);
  const denied = $derived(src?.coverage === 'denied' || src?.threads.permission.state === 'denied');
  const unknowns = $derived.by(() => {
    const out: string[] = [];
    if (!node) return out;
    if (node.synthetic === 'remainder') out.push('These bytes are counted by the parent but not listed in this slice. Open the parent to see them.');
    if (node.synthetic === 'fold') out.push(`${node.folded} items too small to draw at this scale; their bytes are kept in the total.`);
    if (src?.kind === 'other') out.push(`${formatCount(src.folded_count)} items folded by the engine into one node.`);
    if (denied) out.push('Access denied: contents, file count and size below here are unknown, not zero.');
    else if (src && src.coverage !== 'complete') out.push(`Coverage is ${human(src.coverage)}: counts are at least these.`);
    if (z.denied) out.push(`${z.denied} folder${z.denied > 1 ? 's' : ''} below here denied access: no area, no count.`);
    if (z.unmeasured) out.push(`${z.unmeasured} item${z.unmeasured > 1 ? 's' : ''} below here unmeasured.`);
    if (src && src.size_unknown_files > 0) out.push(`${formatCount(src.size_unknown_files)} file${src.size_unknown_files > 1 ? 's have' : ' has'} unknown allocation${basis === 'allocated' ? ' (hatched; counted as 0 under this basis)' : ''}.`);
    if (src && src.threads.residency.tier === null) out.push('Residency unknown: no weft is drawn.');
    if (src?.live || provisional) out.push('Provisional: these sums come from a running scan and may still change.');
    if (src && !src.modified_at && src.kind === 'file') out.push('Modification time not recorded.');
    return out;
  });
</script>

<aside class="inspector" aria-labelledby="insp-name">
  <div class="state"><span>Inspector</span><span class="chip">{status}</span></div>
  {#if !node}
    <p class="muted">Hover or move the keyboard cursor over the cloth to inspect a region.</p>
  {:else}
    <div class="head">
      {#if src}<canvas class="swatch" bind:this={swatch} aria-hidden="true"></canvas>{/if}
      <div>
        <h2 id="insp-name"><bdi>{node.name}</bdi></h2>
        <p class="path"><bdi>{[...path, node.name].join(' / ')}</bdi></p>
      </div>
    </div>

    {#if src}
      <dl class="facts">
        <div><dt>Kind</dt><dd>{src.kind}{#if src.ext_family} · {src.ext_family}{/if}</dd></div>
        <div><dt>Logical</dt><dd class="num">{formatBytes(src.logical_bytes)}</dd></div>
        <div><dt>Allocated</dt><dd class="num">{#if src.allocated_bytes === null}<span class="unknown">unknown</span>{:else}{formatBytes(src.allocated_bytes)}{/if}</dd></div>
        {#if src.kind !== 'file'}
          <div><dt>Contents</dt><dd class="num">{#if denied}<span class="unknown">access denied: unknown</span>{:else}{src.coverage === 'complete' ? '' : 'at least '}{formatCount(src.files)} files · {formatCount(src.dirs)} folders{/if}</dd></div>
        {/if}
        <div><dt>Modified</dt><dd>{formatTime(src.modified_at)}</dd></div>
      </dl>

      <dl class="channels">
        <div class="ch"><dt>Warp<small>meaning</small></dt><dd>
          {MEANING[src.threads.meaning.state] ?? src.threads.meaning.state}{#if src.threads.meaning.label}: <bdi>{src.threads.meaning.label}</bdi>{/if}
          {#if src.threads.meaning.source}<span class="sub">source: {src.threads.meaning.source}{#if src.threads.meaning.state === 'suggested'}; a suggestion is not an approval{/if}</span>{/if}
        </dd></div>
        <div class="ch"><dt>Weft<small>residency</small></dt><dd>
          {#if src.threads.residency.tier === null}<span class="unknown">unknown</span>
          {:else}Tier {src.threads.residency.tier} · {TIER[Math.min(src.threads.residency.tier, 2)]}{#if src.threads.permission.reason === 'cloud_placeholder'} · online-only placeholder{/if}
            <span class="sub">{src.threads.residency.volume_id ?? 'volume unknown'} · {human(src.threads.residency.tier_basis)}</span>{/if}
        </dd></div>
        <div class="ch"><dt>Selvedge<small>permission</small></dt><dd>
          {PERM[src.threads.permission.state] ?? src.threads.permission.state}
          {#if src.threads.permission.reason}<span class="sub">{human(src.threads.permission.reason)}</span>{/if}
        </dd></div>
      </dl>
    {:else}
      <p class="num">{formatApprox(node.size)} <span class="muted">(approximate, from the parent's total)</span></p>
    {/if}

    {#if unknowns.length}
      <h3>Unknown, kept visible</h3>
      <ul class="unknowns">{#each unknowns as u (u)}<li>{u}</li>{/each}</ul>
    {/if}
  {/if}
  <p class="foot">Describes only. Nothing here moves, deletes or changes a file.</p>
</aside>

<style>
  .inspector { min-width: 0; }
  .state { display: flex; justify-content: space-between; align-items: center; font-size: 0.8rem; color: var(--faint); margin-bottom: 10px; }
  .chip { padding: 1px 8px; border: 1px solid var(--line-strong); border-radius: 999px; color: var(--muted); }
  .head { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 12px; align-items: center; }
  .swatch { width: 48px; height: 48px; border-radius: 2px; display: block; }
  h2 { margin: 0; font-size: 1.45rem; overflow-wrap: anywhere; }
  h3 { font-size: 0.95rem; margin: 14px 0 6px; }
  .path { margin: 2px 0 0; font: 0.8rem/1.4 var(--font-mono); color: var(--muted); overflow-wrap: anywhere; }
  .facts, .channels { margin: 12px 0 0; }
  .facts div { display: flex; justify-content: space-between; gap: 12px; padding: 3px 0; font-size: 0.88rem; }
  .facts dt { color: var(--muted); }
  .facts dd { margin: 0; text-align: right; }
  .ch { display: grid; grid-template-columns: 78px minmax(0, 1fr); gap: 10px; padding: 9px 0; border-top: 1px solid var(--line); }
  .ch dt { font: italic 400 0.95rem/1.3 var(--font-serif-text); color: var(--muted); }
  :global([data-theme='observatory']) .ch dt { font: 500 0.8rem/1.4 var(--font-mono); }
  .ch dt small { display: block; font: 400 0.75rem var(--font-ui); color: var(--faint); }
  .ch dd { margin: 0; font-size: 0.9rem; overflow-wrap: anywhere; }
  .sub { display: block; font-size: 0.8rem; color: var(--muted); }
  .unknowns { margin: 0; padding: 0; list-style: none; display: grid; gap: 5px; }
  .unknowns li { position: relative; padding-left: 16px; font-size: 0.84rem; color: var(--muted); }
  .unknowns li::before { content: ''; position: absolute; left: 2px; top: 0.5em; width: 7px; height: 7px; border: 1px dashed var(--unknown); border-radius: 50%; }
  .foot { margin-top: 14px; font-size: 0.8rem; color: var(--faint); }
</style>
