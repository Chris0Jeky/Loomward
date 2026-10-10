<script module lang="ts">
  export const meta = { title: 'Atlas', order: 40 };
</script>

<script lang="ts">
  // Lane L10: the woven atlas over a bounded TreeSlice. Layout is client-side (app/viz).
  import { onMount, untrack } from 'svelte';
  import { session } from '../../lib/stores/session.svelte';
  import { theme } from '../../lib/stores/theme.svelte';
  import { formatCount } from '../../lib/format/time';
  import type { NodeInfo, ThreadToggles } from '../../../viz/types.js';
  import { createWovenTreemap, drawSwatch } from '../../../viz/woven-treemap.js';
  import Inspector from './Inspector.svelte';
  import RegionList from './RegionList.svelte';
  import { SliceNav, announceSlice, canvasLabel, describeNode, describeSize, formatApprox, pathTo, readPalette, type SliceKey } from './shared.svelte';
  import { Say } from '../../lib/ui/say.svelte';
  import LoadError from './LoadError.svelte';
  import VisibleName from '../../lib/ui/VisibleName.svelte';

  // 2,500 nodes is the P9 budget (docs/41 section 15); the renderer nests three levels and folds the rest.
  const nav = new SliceNav({ depth: 6, maxNodes: 2500, minShare: 0 });
  const reducedMQ = matchMedia('(prefers-reduced-motion: reduce)');

  let canvas = $state<HTMLCanvasElement>();
  let tm: ReturnType<typeof createWovenTreemap> | null = null;
  let palette = $state(readPalette());
  let hovered = $state<NodeInfo | null>(null);
  let selected = $state<NodeInfo | null>(null);
  const say = new Say();
  let tip = $state<{ x: number; y: number; name: string; sub: string } | null>(null);
  let show = $state<ThreadToggles>({ meaning: true, residency: true, permission: true });
  let revealed = false;
  let shownKey: SliceKey | null = null; // what the last "Showing ..." announced: a refetch of the same view stays silent
  let well = $state<HTMLDivElement>();

  const shown = $derived(hovered ?? selected);
  const provisional = $derived(nav.slice?.aggregate_state === 'provisional_live');
  const pathNames = $derived(shown ? pathTo(shown, nav.slice, nav.trail.map((c) => c.name)) : []);

  onMount(() => {
    if (!canvas) return;
    const t = createWovenTreemap(canvas, { palette, reducedMotion: () => reducedMQ.matches, format: formatApprox, marks: 'loomward:atlas', label: canvasLabel });
    tm = t;
    const off = [
      t.on('hover', (e: { node: NodeInfo | null; clientX: number; clientY: number; viaKeyboard: boolean }) => {
        hovered = e.node;
        if (e.node && e.viaKeyboard) say.say(describeNode(e.node));
        if (e.node && !e.viaKeyboard && well) {
          const r = well.getBoundingClientRect();
          tip = { x: e.clientX - r.left, y: e.clientY - r.top, name: e.node.name, sub: describeSize(e.node) };
        } else tip = null;
      }),
      t.on('select', (n: NodeInfo) => { selected = n; say.say(describeNode(n)); }),
      t.on('drill', (n: NodeInfo) => { hovered = null; tip = null; void nav.drill(n.id, n.name); }),
      t.on('back', () => {
        hovered = null;
        if (nav.trail.length < 2) say.say(`Already at the top: ${canvasLabel(nav.here?.name ?? '')}.`);
        void nav.back();
      }),
      t.on('edge', (e: { dir: string; node: NodeInfo }) => say.say(`Nothing further ${e.dir}. Still on ${canvasLabel(e.node.name)}.`)),
    ];
    return () => { off.forEach((f) => f()); t.destroy(); tm = null; };
  });

  // (Re)start whenever the session starts or the engine says data may be stale.
  $effect(() => {
    void session.epoch;
    if (session.client) untrack(() => void nav.start());
  });

  // Hand every new slice to the renderer; the first one is woven in.
  $effect(() => {
    const s = nav.slice;
    if (!tm) return;
    const t = tm;
    if (!s) {
      // a failed load: drop the cloth so nothing stale stays drawn or clickable
      untrack(() => { t.clear(); hovered = null; selected = null; tip = null; shownKey = null; });
      return;
    }
    untrack(() => {
      t.setSlice(s);
      if (!revealed) { revealed = true; t.reveal(); }
      // the selection follows the new slice: re-read it from the renderer (its old info is from the previous slice)
      selected = selected ? t.info(selected.id) : null;
      const a = announceSlice(shownKey, s, nav.here?.name ?? '');
      shownKey = a.key;
      if (a.text) say.say(a.text);
    });
  });

  $effect(() => {
    void theme.current;
    const p = readPalette();
    palette = p;
    untrack(() => tm?.setTheme(p));
  });
  $effect(() => { tm?.setThreads({ ...show }); });
  $effect(() => { tm?.setBasis(nav.basis); });

  function dismissTip(e: KeyboardEvent) {
    if (e.key !== 'Escape' || !tip) return;
    tip = null;
    e.stopImmediatePropagation();
  }

  function inspectById(id: string) {
    const n = tm?.info(id) ?? null;
    if (!n) return;
    selected = n;
    hovered = null;
    say.say(describeNode(n));
  }

  // Basis radiogroup: arrow keys move the choice.
  function basisKey(e: KeyboardEvent) {
    if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(e.key)) return;
    e.preventDefault();
    const next = nav.basis === 'logical' ? 'allocated' : 'logical';
    void nav.setBasis(next);
    (e.currentTarget as HTMLElement).querySelector<HTMLButtonElement>(`[data-basis="${next}"]`)?.focus();
  }

  // Legend swatches, drawn by the same renderer as the cloth.
  const legend = {
    warp: [
      { label: 'Labelled', threads: { meaning: { state: 'labelled', label: 'Projects' } } },
      { label: 'Suggested', threads: { meaning: { state: 'suggested', label: 'Projects' } } },
      { label: 'Mixed', threads: { meaning: { state: 'mixed', label: 'Media' } } },
      { label: 'None', threads: { meaning: { state: 'none', label: null } } },
      { label: 'Pending', threads: { meaning: { state: 'pending', label: null } } },
    ],
    weft: [
      { label: 'Tier 0 · hot', threads: { residency: { volume_id: 'v', tier: 0, tier_basis: 'declared' } } },
      { label: 'Tier 1 · warm', threads: { residency: { volume_id: 'v', tier: 1, tier_basis: 'declared' } } },
      { label: 'Tier 2 · cold', threads: { residency: { volume_id: 'v', tier: 2, tier_basis: 'declared' } } },
      { label: 'Online-only', threads: { residency: { volume_id: 'v', tier: 0, tier_basis: 'declared' }, permission: { state: 'granted', reason: 'cloud_placeholder' } } },
      { label: 'Unknown', threads: { residency: { volume_id: null, tier: null, tier_basis: 'unknown' } } },
    ],
    selvedge: [
      { label: 'Denied', stitch: 'band' },
      { label: 'Partial', stitch: 'running' },
      { label: 'Excluded', stitch: 'cross' },
      { label: 'Revoked', stitch: 'double' },
      { label: 'Unknown', stitch: 'dotted' },
    ],
  } as const;
  const base = { meaning: { state: 'none', label: null }, residency: { volume_id: null, tier: null, tier_basis: 'unknown' }, permission: { state: 'granted', reason: null } } as const;
  let swatches: HTMLCanvasElement[] = $state([]);
  $effect(() => {
    const p = palette;
    const items = [
      ...legend.warp.map((l) => ({ threads: { ...base, ...l.threads }, only: 'warp' as const })),
      ...legend.weft.map((l) => ({ threads: { ...base, ...l.threads }, only: 'weft' as const })),
      ...legend.selvedge.map((l) => ({ only: 'selvedge' as const, stitch: l.stitch })),
    ];
    items.forEach((o, i) => { const c = swatches[i]; if (c) drawSwatch(c, p, o as never); });
  });
</script>

<!-- WCAG 1.4.13: the hover tooltip can be dismissed without moving the pointer. On the canvas, Escape that dismisses a tooltip
     is spent on that: it does not also go back up a level (the canvas's own handler never sees it). -->
<svelte:window onkeydown={(e) => { if (e.key === 'Escape' && tip) tip = null; }} />

<div class="top">
  <h1>Atlas</h1>
  <nav class="crumbs" aria-label="Location">
    <ol>
      {#each nav.trail as c, i (c.id)}
        <li><button type="button" class="crumb" aria-current={i === nav.trail.length - 1 ? 'location' : undefined} onclick={() => void nav.jump(i)}><VisibleName name={c.name} /></button></li>
      {/each}
    </ol>
  </nav>
</div>

<div class="bar">
  <div class="seg" role="radiogroup" aria-label="Area represents" tabindex="-1" onkeydown={basisKey}>
    <button type="button" role="radio" data-basis="logical" aria-checked={nav.basis === 'logical'} tabindex={nav.basis === 'logical' ? 0 : -1} onclick={() => void nav.setBasis('logical')}>Logical size</button>
    <button type="button" role="radio" data-basis="allocated" aria-checked={nav.basis === 'allocated'} tabindex={nav.basis === 'allocated' ? 0 : -1} onclick={() => void nav.setBasis('allocated')}>Allocated</button>
  </div>
  <fieldset class="toggles">
    <legend class="sr-only">Threads shown</legend>
    <label><input type="checkbox" bind:checked={show.meaning} /> Meaning</label>
    <label><input type="checkbox" bind:checked={show.residency} /> Residency</label>
    <label><input type="checkbox" bind:checked={show.permission} /> Permission</label>
  </fieldset>
  <p class="status num" aria-live="polite">
    {#if nav.slice}
      {formatCount(nav.slice.nodes.length)} nodes{#if nav.slice.truncated} · <span class="warn">truncated</span>{/if}
      {#if provisional} · <span class="warn">provisional: running scan</span>{/if}
      · {session.datasetClass ?? 'unknown'} data
    {:else if nav.busy}Loading{/if}
  </p>
</div>

<LoadError {nav} />

<div class="stage">
  <section class="map" aria-label="Woven atlas">
    <div class="well" bind:this={well}>
      <canvas
        bind:this={canvas}
        tabindex="0"
        aria-label="Woven treemap of the slice. Arrow keys move between regions, Enter opens one, Escape goes back."
        onblur={() => say.clear()}
        onkeydowncapture={dismissTip}
      ></canvas>
      {#if tip}
        <div class="tip" style:left={`${Math.min(tip.x + 16, (well?.clientWidth ?? 0) - 240)}px`} style:top={`${tip.y + 18}px`}>
          <b><VisibleName name={tip.name} /></b><span class="num">{tip.sub}</span>
        </div>
      {/if}
    </div>
    <p id="atlas-live" class="sr-only" aria-live="polite">{say.text}</p>
    <p class="help">Click a region to open it · <kbd>Arrows</kbd> move · <kbd>Enter</kbd> opens · <kbd>Esc</kbd> goes back</p>
    <RegionList slice={nav.slice} onopen={(id, name) => void nav.drill(id, name)} oninspect={inspectById} />
  </section>

  <Inspector node={shown} status={hovered ? 'Hover' : selected ? 'Selected' : 'Nothing selected'} basis={nav.basis} {provisional} {palette} path={pathNames} />
</div>

<section class="legend" aria-labelledby="threads-h">
  <h2 id="threads-h">Three threads, never one fill</h2>
  <div class="groups">
    <div>
      <h3><em>Warp</em> carries meaning</h3>
      <p>Vertical threads, dyed per label. A suggestion is half-dyed; pending threads break.</p>
      <ul>{#each legend.warp as l, i (l.label)}<li><canvas bind:this={swatches[i]} aria-hidden="true"></canvas>{l.label}</li>{/each}</ul>
    </div>
    <div>
      <h3><em>Weft</em> carries residency</h3>
      <p>Crossing threads in metals by tier: hot is bright, cold is dark. Unknown residency has no weft.</p>
      <ul>{#each legend.weft as l, i (l.label)}<li><canvas bind:this={swatches[legend.warp.length + i]} aria-hidden="true"></canvas>{l.label}</li>{/each}</ul>
    </div>
    <div>
      <h3><em>Selvedge</em> carries permission</h3>
      <p>The edge is stitched where a state begins; inside it, children inherit. Granted needs no stitch.</p>
      <ul>{#each legend.selvedge as l, i (l.label)}<li><canvas bind:this={swatches[legend.warp.length + legend.weft.length + i]} aria-hidden="true"></canvas>{l.label}</li>{/each}</ul>
    </div>
  </div>
  <p class="muted small">Loose threads are bytes the parent counts but this slice does not list; fringe is many items too small to draw; cross-hatching is unknown allocation. None of them is zero.</p>
</section>

<style>
  .top { display: flex; align-items: baseline; gap: 18px; flex-wrap: wrap; }
  h1 { margin: 0; }
  .crumbs ol { list-style: none; display: flex; flex-wrap: wrap; gap: 2px; margin: 0; padding: 0; }
  .crumbs li + li::before { content: '/'; margin: 0 8px; color: var(--faint); }
  .crumb { font: 400 1.05rem/1.3 var(--head-font); color: var(--muted); background: none; border: 0; padding: 2px; cursor: pointer; overflow-wrap: anywhere; text-align: left; }
  .crumb:hover { color: var(--text); }
  .crumb[aria-current='location'] { color: var(--text); cursor: default; }
  @media (forced-colors: active) {
    .crumb[aria-current='location'] { text-decoration: underline; text-underline-offset: 3px; }
    .seg button[aria-checked='true'] { forced-color-adjust: none; background: Highlight; color: HighlightText; }
  }
  .bar { display: flex; flex-wrap: wrap; align-items: center; gap: 10px 18px; margin: 12px 0; }
  .seg { display: inline-flex; padding: 2px; border: 1px solid var(--control-line); border-radius: 999px; }
  .seg button { font: inherit; font-size: 0.85rem; border: 0; background: none; color: var(--muted); padding: 4px 12px; border-radius: 999px; cursor: pointer; }
  .seg button[aria-checked='true'] { background: var(--raised); color: var(--text); box-shadow: inset 0 0 0 1px var(--line-strong); }
  .toggles { display: flex; gap: 14px; border: 0; margin: 0; padding: 0; font-size: 0.88rem; color: var(--muted); }
  .toggles input { accent-color: var(--accent); }
  .status { margin: 0 0 0 auto; font-size: 0.85rem; color: var(--muted); }
  .warn { color: var(--warn); }
  .stage { display: grid; grid-template-columns: minmax(0, 1fr) minmax(260px, 340px); gap: 24px; }
  .map { min-width: 0; }
  .well { position: relative; height: clamp(380px, 62vh, 640px); border-radius: var(--radius); background: var(--cloth-ink); box-shadow: 0 0 0 1px var(--line); overflow: hidden; }
  canvas { display: block; }
  .well canvas { position: absolute; inset: 0; width: 100%; height: 100%; touch-action: none; }
  .well canvas:focus-visible { outline-offset: -2px; }
  .tip { position: absolute; z-index: 2; pointer-events: none; max-width: 260px; padding: 7px 10px; background: color-mix(in srgb, var(--surface) 94%, transparent); border: 1px solid var(--line-strong); border-radius: var(--radius); font-size: 0.82rem; box-shadow: 0 6px 14px -6px rgba(0, 0, 0, 0.7); }
  .tip b { display: block; overflow-wrap: anywhere; }
  .tip span { color: var(--muted); }
  .help { margin: 8px 0 0; font-size: 0.8rem; color: var(--faint); }
  kbd { font: 0.75rem var(--font-mono); padding: 0 4px; border: 1px solid var(--line-strong); border-radius: 3px; color: var(--muted); }
  .legend { margin-top: 24px; padding-top: 14px; border-top: 1px solid var(--line); }
  .groups { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 24px; }
  .groups h3 { font: 600 0.9rem var(--font-ui); margin: 0 0 3px; }
  .groups h3 em { font: italic 400 1rem var(--font-serif-text); color: var(--accent); margin-right: 6px; }
  .groups p { font-size: 0.84rem; color: var(--muted); margin: 0 0 8px; }
  .groups ul { list-style: none; margin: 0; padding: 0; display: flex; flex-wrap: wrap; gap: 6px 14px; }
  .groups li { display: inline-flex; align-items: center; gap: 7px; font-size: 0.84rem; color: var(--muted); }
  .groups canvas { width: 28px; height: 18px; border-radius: 1.5px; }
  .small { font-size: 0.82rem; margin-top: 10px; }
  @media (max-width: 1100px) { .stage { grid-template-columns: 1fr; } .groups { grid-template-columns: 1fr; gap: 14px; } }
  @media (max-width: 760px) { .well { height: 420px; } .status { margin-left: 0; } }
</style>
