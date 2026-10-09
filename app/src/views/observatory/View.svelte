<script module lang="ts">
  export const meta = { title: 'Observatory', order: 45 };
</script>

<script lang="ts">
  // Lane L10: the orbitable sunburst plus resource instruments. Telemetry is polled from
  // `telemetry.snapshot`; the mock serves synthetic samples and they are labelled as such.
  import { onMount, untrack } from 'svelte';
  import { session } from '../../lib/stores/session.svelte';
  import { theme } from '../../lib/stores/theme.svelte';
  import { formatBytes } from '../../lib/format/bytes';
  import { formatCount } from '../../lib/format/time';
  import { LoomwardError } from '../../lib/transport/client';
  import type { DatasetClass, TelemetrySample } from '../../lib/types';
  import type { NodeInfo } from '../../../viz/types.js';
  import { createSunburst } from '../../../viz/sunburst.js';
  import { createGauge, type GaugeTheme } from '../../../viz/gauges.js';
  import Inspector from '../atlas/Inspector.svelte';
  import RegionList from '../atlas/RegionList.svelte';
  import { SliceNav, formatApprox, readPalette } from '../atlas/shared.svelte';

  const nav = new SliceNav({ depth: 4, maxNodes: 2500, minShare: 0 });
  const reducedMQ = matchMedia('(prefers-reduced-motion: reduce)');

  let canvas = $state<HTMLCanvasElement>();
  let sb: ReturnType<typeof createSunburst> | null = null;
  let palette = $state(readPalette());
  let hovered = $state<NodeInfo | null>(null);
  let selected = $state<NodeInfo | null>(null);
  let live = $state('');
  const shown = $derived(hovered ?? selected);
  const provisional = $derived(nav.slice?.aggregate_state === 'provisional_live');

  // --- telemetry ---------------------------------------------------------------------------
  type TelState = 'waiting' | 'live' | 'unavailable';
  let tel = $state<TelemetrySample | null>(null);
  let telState = $state<TelState>('waiting');
  let telClass = $state<DatasetClass | null>(null);
  let telNote = $state('');
  const gaugeEls: Record<'cpu' | 'gpu' | 'vram' | 'io', HTMLCanvasElement | undefined> = $state({ cpu: undefined, gpu: undefined, vram: undefined, io: undefined });
  const sparkEls: (HTMLCanvasElement | undefined)[] = $state([]);
  let gauges: Record<string, ReturnType<typeof createGauge>> = {};
  let sparks: ReturnType<typeof createGauge>[] = [];

  function gaugeTheme(): GaugeTheme {
    const cs = getComputedStyle(document.documentElement);
    const t = (k: string) => cs.getPropertyValue(k).trim();
    const num = t('--num-font') || t('--font-ui');
    return { track: t('--gauge-track'), value: t('--gauge-value'), glow: t('--gauge-glow'), text: t('--text'), dim: t('--muted'), unknown: t('--unknown'), fontNum: `500 19px ${num}`, fontSmall: `400 10.5px ${num}` };
  }
  const frac = (f: number | null | undefined) => (typeof f === 'number' ? f * 100 : null);
  const num = (s: string | null | undefined) => (typeof s === 'string' && /^\d+$/.test(s) ? Number(s) : null);
  const mbps = (v: number | null) => (v === null ? 'unknown' : `${(v / 1e6).toFixed(v >= 1e8 ? 0 : 1)} MB/s`);

  function apply(s: TelemetrySample) {
    tel = s;
    const gpu = s.gpu?.adapters[0];
    gauges.cpu?.push(frac(s.system?.cpu.busy_fraction));
    gauges.gpu?.push(frac(gpu?.engine_busy_fraction));
    const used = num(gpu?.dedicated_used_bytes);
    gauges.vram?.push(used === null ? null : used / 2 ** 30);
    const disks = s.disks?.disks ?? [];
    const rates = disks.map((d) => (d.read_bytes_per_s === null || d.write_bytes_per_s === null ? null : d.read_bytes_per_s + d.write_bytes_per_s));
    gauges.io?.push(rates.length && rates.every((r) => r !== null) ? (rates as number[]).reduce((a, b) => a + b, 0) / 1e6 : null);
    rates.forEach((r, i) => sparks[i]?.push(r === null ? null : r / 1e6));
  }

  async function poll(mine: number) {
    const c = session.client;
    if (!c || document.hidden) return;
    try {
      const { result, meta } = await c.call('telemetry.snapshot', { channels: ['system', 'gpu', 'disks'] });
      if (mine !== pollTicket) return;
      telClass = meta.dataset_class;
      telState = 'live';
      apply(result);
    } catch (e) {
      if (mine !== pollTicket) return;
      telState = 'unavailable';
      telNote = e instanceof LoomwardError ? e.message : session.handle(e);
      tel = null;
      clearInterval(timer);
    }
  }
  let timer: ReturnType<typeof setInterval> | undefined;
  let pollTicket = 0;

  const memory = $derived.by(() => {
    const m = tel?.system?.memory;
    const total = num(m?.total_bytes), avail = num(m?.available_bytes), commit = num(m?.commit_bytes), limit = num(m?.commit_limit_bytes);
    return { m, total, avail, inUse: total !== null && avail !== null ? total - avail : null, commit, limit };
  });

  onMount(() => {
    if (!canvas) return;
    const s = createSunburst(canvas, { palette, reducedMotion: () => reducedMQ.matches, format: formatApprox });
    sb = s;
    const off = [
      s.on('hover', (e: { node: NodeInfo | null; viaKeyboard: boolean }) => { hovered = e.node; if (e.node && e.viaKeyboard) live = `${e.node.name}, ${formatApprox(e.node.size)}.${e.node.drillable ? ' Enter opens it.' : ''}`; }),
      s.on('select', (n: NodeInfo) => { selected = n; }),
      s.on('drill', (n: NodeInfo) => { hovered = null; void nav.drill(n.id, n.name); }),
      s.on('back', () => { hovered = null; void nav.back(); }),
    ];
    const th = gaugeTheme();
    const fmtPct = (v: number) => v.toFixed(0);
    gauges = {
      cpu: createGauge(gaugeEls.cpu!, { kind: 'arc', unit: '%', min: 0, max: 100, theme: th, format: fmtPct }),
      gpu: createGauge(gaugeEls.gpu!, { kind: 'arc', unit: '%', min: 0, max: 100, theme: th, format: fmtPct }),
      vram: createGauge(gaugeEls.vram!, { kind: 'arc', unit: 'GiB', min: 0, max: 24, theme: th }),
      io: createGauge(gaugeEls.io!, { kind: 'arc', unit: 'MB/s', min: 0, max: 3000, theme: th, format: (v) => (v >= 1000 ? `${(v / 1000).toFixed(1)}k` : v.toFixed(0)) }),
    };
    sparks = sparkEls.map((el) => createGauge(el!, { kind: 'spark', unit: 'MB/s', theme: th }));
    const mine = ++pollTicket;
    void poll(mine);
    timer = setInterval(() => void poll(mine), 1000);
    return () => {
      pollTicket++;
      clearInterval(timer);
      off.forEach((f) => f());
      s.destroy(); sb = null;
      Object.values(gauges).forEach((g) => g.destroy());
      sparks.forEach((g) => g.destroy());
    };
  });

  $effect(() => {
    void session.epoch;
    if (session.client) untrack(() => void nav.start());
  });
  $effect(() => {
    const s = nav.slice;
    if (!s || !sb) return;
    const b = sb;
    untrack(() => {
      b.setSlice(s);
      if (selected && !s.nodes.some((n) => n.node_id === selected!.id)) selected = null;
      live = `Showing ${nav.here?.name ?? ''}: ${formatCount(s.nodes.length)} nodes${s.aggregate_state === 'provisional_live' ? ', provisional sums from a running scan' : ''}.`;
    });
  });
  $effect(() => {
    void theme.current;
    const p = readPalette();
    palette = p;
    untrack(() => {
      sb?.setTheme(p);
      const th = gaugeTheme();
      Object.values(gauges).forEach((g) => g.setTheme(th));
      sparks.forEach((g) => g.setTheme(th));
    });
  });

  function basisKey(e: KeyboardEvent) {
    if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(e.key)) return;
    e.preventDefault();
    const next = nav.basis === 'logical' ? 'allocated' : 'logical';
    void nav.setBasis(next);
    (e.currentTarget as HTMLElement).querySelector<HTMLButtonElement>(`[data-basis="${next}"]`)?.focus();
  }
</script>

<div class="top">
  <h1>Observatory</h1>
  <nav class="crumbs" aria-label="Location">
    <ol>
      {#each nav.trail as c, i (c.id)}
        <li><button type="button" class="crumb" aria-current={i === nav.trail.length - 1 ? 'location' : undefined} onclick={() => void nav.jump(i)}><bdi>{c.name}</bdi></button></li>
      {/each}
    </ol>
  </nav>
  <div class="seg" role="radiogroup" aria-label="Area represents" tabindex="-1" onkeydown={basisKey}>
    <button type="button" role="radio" data-basis="logical" aria-checked={nav.basis === 'logical'} tabindex={nav.basis === 'logical' ? 0 : -1} onclick={() => void nav.setBasis('logical')}>Logical</button>
    <button type="button" role="radio" data-basis="allocated" aria-checked={nav.basis === 'allocated'} tabindex={nav.basis === 'allocated' ? 0 : -1} onclick={() => void nav.setBasis('allocated')}>Allocated</button>
  </div>
</div>

{#if nav.error}<p class="bad" role="alert">{nav.error}</p>{/if}

<div class="stage">
  <section class="orbit" aria-label="Sunburst">
    <div class="well">
      <canvas
        bind:this={canvas}
        tabindex="0"
        aria-label="Sunburst of the slice. Drag to orbit. Arrow keys move between arcs, Enter opens one, Escape goes back, [ and ] rotate."
        aria-describedby="obs-live"
      ></canvas>
    </div>
    <p id="obs-live" class="sr-only" aria-live="polite">{live}</p>
    <p class="help">
      Drag to orbit · click an arc to open it · the centre goes back · <kbd>[ ]</kbd> rotate
      {#if nav.slice} · <span class="num">{formatCount(nav.slice.nodes.length)}</span> nodes{#if provisional} · <span class="warn">provisional: running scan</span>{/if}{/if}
    </p>
    <RegionList slice={nav.slice} onopen={(id, name) => void nav.drill(id, name)} />
  </section>

  <div class="side">
    <section class="resources" aria-labelledby="res-h">
      <div class="res-top">
        <h2 id="res-h">Resources</h2>
        <p class="res-note" role="status">
          {#if telState === 'unavailable'}<span class="warn">Telemetry unavailable</span>
          {:else if telState === 'waiting'}Waiting for a sample
          {:else}<span class="dot" aria-hidden="true"></span>{telClass === 'synthetic' ? 'Synthetic telemetry' : 'Observed'} · sample {tel?.sample_seq ?? ''}{/if}
        </p>
      </div>
      {#if telState === 'unavailable'}<p class="muted small">{telNote} Nothing is shown in its place.</p>{/if}
      <div class="gauges" class:off={telState === 'unavailable'}>
        <figure><figcaption>CPU</figcaption><canvas bind:this={gaugeEls.cpu} aria-label={`CPU busy ${frac(tel?.system?.cpu.busy_fraction)?.toFixed(0) ?? 'unknown'} percent`}></canvas></figure>
        <figure><figcaption>GPU</figcaption><canvas bind:this={gaugeEls.gpu} aria-label={`GPU busy ${frac(tel?.gpu?.adapters[0]?.engine_busy_fraction)?.toFixed(0) ?? 'unknown'} percent`}></canvas></figure>
        <figure><figcaption>VRAM</figcaption><canvas bind:this={gaugeEls.vram} aria-label={`GPU memory used ${formatBytes(tel?.gpu?.adapters[0]?.dedicated_used_bytes)}`}></canvas></figure>
        <figure><figcaption>Disk I/O</figcaption><canvas bind:this={gaugeEls.io} aria-label="Disk read plus write rate"></canvas></figure>
      </div>
      <div class="disks">
        {#each ['C:', 'G:', 'E:'] as label, i (label)}
          {@const d = tel?.disks?.disks[i]}
          <div class="disk">
            <span class="dl num">{label}</span>
            <canvas bind:this={sparkEls[i]} aria-hidden="true"></canvas>
            <span class="dv num">r {mbps(d?.read_bytes_per_s ?? null)} · w {mbps(d?.write_bytes_per_s ?? null)}</span>
          </div>
        {/each}
      </div>
      <dl class="mem">
        <div><dt>In use</dt><dd class="num">{memory.inUse === null ? 'unknown' : formatBytes(String(memory.inUse))}</dd></div>
        <div><dt>Available</dt><dd class="num">{formatBytes(memory.m?.available_bytes)}</dd></div>
        <div><dt>Total</dt><dd class="num">{formatBytes(memory.m?.total_bytes)}</dd></div>
        <div class="wide"><dt>Commit</dt><dd class="num">{formatBytes(memory.m?.commit_bytes)} of {formatBytes(memory.m?.commit_limit_bytes)}</dd></div>
      </dl>
      <div class="membar" role="img" aria-label="Physical memory in use versus available">
        <span style:flex-grow={memory.inUse ?? 0} class="use"></span><span style:flex-grow={memory.avail ?? 0} class="avail"></span>
      </div>
      <p class="fine">Read-only. There is no memory cleaner, trim or kill control here, by design. Available already includes the standby cache.</p>
    </section>

    <Inspector node={shown} status={hovered ? 'Hover' : selected ? 'Selected' : 'Nothing selected'} basis={nav.basis} {provisional} {palette} path={nav.trail.map((c) => c.name)} />
  </div>
</div>

<style>
  .top { display: flex; align-items: baseline; gap: 12px 18px; flex-wrap: wrap; }
  h1 { margin: 0; }
  .crumbs { flex: 1 1 12rem; min-width: 0; }
  .crumbs ol { list-style: none; display: flex; flex-wrap: wrap; gap: 2px; margin: 0; padding: 0; }
  .crumbs li + li::before { content: '/'; margin: 0 8px; color: var(--faint); }
  .crumb { font: 500 0.95rem/1.3 var(--num-font); color: var(--muted); background: none; border: 0; padding: 2px; cursor: pointer; overflow-wrap: anywhere; text-align: left; }
  .crumb:hover { color: var(--text); }
  .crumb[aria-current='location'] { color: var(--text); cursor: default; }
  .seg { display: inline-flex; padding: 2px; border: 1px solid var(--line); border-radius: 999px; }
  .seg button { font: inherit; font-size: 0.85rem; border: 0; background: none; color: var(--muted); padding: 4px 12px; border-radius: 999px; cursor: pointer; }
  .seg button[aria-checked='true'] { background: var(--raised); color: var(--text); box-shadow: inset 0 0 0 1px var(--line-strong); }
  .bad { color: var(--danger); }
  .warn { color: var(--warn); }
  .stage { display: grid; grid-template-columns: minmax(0, 1fr) minmax(300px, 380px); gap: 24px; margin-top: 12px; }
  .orbit { min-width: 0; }
  .well { position: relative; height: clamp(420px, 70vh, 720px); border-radius: var(--radius); background: radial-gradient(closest-side, var(--bg-weave), var(--cloth-ink)); box-shadow: 0 0 0 1px var(--line); overflow: hidden; }
  .well canvas { position: absolute; inset: 0; width: 100%; height: 100%; display: block; touch-action: none; }
  .well canvas:focus-visible { outline-offset: -2px; }
  .help { margin: 8px 0 0; font-size: 0.8rem; color: var(--faint); }
  kbd { font: 0.75rem var(--font-mono); padding: 0 4px; border: 1px solid var(--line-strong); border-radius: 3px; color: var(--muted); }
  .side { display: grid; gap: 22px; align-content: start; min-width: 0; }
  .res-top { display: flex; justify-content: space-between; align-items: baseline; gap: 10px; }
  .res-top h2 { margin: 0; }
  .res-note { margin: 0; font-size: 0.8rem; color: var(--muted); display: inline-flex; gap: 6px; align-items: center; }
  .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--gauge-value); }
  .gauges { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 4px; margin-top: 8px; }
  .gauges.off { opacity: 0.35; }
  figure { margin: 0; text-align: center; }
  figcaption { font-size: 0.78rem; color: var(--muted); }
  figure canvas { width: 100%; height: 86px; display: block; }
  .disks { display: grid; gap: 4px; margin: 10px 0; }
  .disk { display: grid; grid-template-columns: 26px minmax(60px, 1fr) auto; gap: 8px; align-items: center; }
  .dl { font-size: 0.8rem; color: var(--muted); }
  .disk canvas { width: 100%; height: 24px; display: block; }
  .dv { font-size: 0.75rem; color: var(--muted); text-align: right; white-space: nowrap; }
  .mem { display: grid; grid-template-columns: 1fr 1fr; gap: 3px 16px; margin: 6px 0; }
  .mem div { display: flex; justify-content: space-between; gap: 8px; font-size: 0.82rem; }
  .mem dt { color: var(--muted); }
  .mem dd { margin: 0; }
  .mem .wide { grid-column: 1 / -1; }
  .membar { display: flex; height: 10px; gap: 2px; border-radius: 1px; overflow: hidden; background: var(--raised); }
  .membar .use { background: var(--dye-2); }
  .membar .avail { background: var(--dye-1); opacity: 0.6; }
  .fine, .small { font-size: 0.78rem; color: var(--faint); margin-top: 8px; }
  @media (max-width: 1100px) { .stage { grid-template-columns: 1fr; } }
  @media (max-width: 760px) {
    .well { height: 380px; }
    .gauges { grid-template-columns: repeat(2, minmax(0, 1fr)); }
    .disk { grid-template-columns: 26px minmax(0, 1fr); }
    .dv { grid-column: 2; text-align: left; }
    .mem { grid-template-columns: 1fr; }
  }
</style>
