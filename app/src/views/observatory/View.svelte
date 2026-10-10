<script module lang="ts">
  export const meta = { title: 'Observatory', order: 45 };
</script>

<script lang="ts">
  // Lane L10: the orbitable sunburst plus resource instruments. Telemetry is polled from
  // `telemetry.snapshot` (one call in flight, backoff after a failure, resume on success or on a new
  // session epoch); the mock serves synthetic samples and they are labelled as such. Every scale comes
  // from the sample: disks by their own labels, VRAM against the adapter's total, disk I/O auto-ranged.
  import { onMount, untrack } from 'svelte';
  import { session } from '../../lib/stores/session.svelte';
  import { theme } from '../../lib/stores/theme.svelte';
  import { formatBytes } from '../../lib/format/bytes';
  import { formatCount } from '../../lib/format/time';
  import { LoomwardError } from '../../lib/transport/client';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import type { DatasetClass, ResponseMeta } from '../../lib/types';
  import type { TelemetrySample } from '../../lib/types.views';
  import type { NodeInfo } from '../../../viz/types.js';
  import { createSunburst } from '../../../viz/sunburst.js';
  import { createGauge, type GaugeTheme } from '../../../viz/gauges.js';
  import Inspector from '../atlas/Inspector.svelte';
  import RegionList from '../atlas/RegionList.svelte';
  import { SliceNav, canvasLabel, formatApprox, readPalette } from '../atlas/shared.svelte';
  import { Poller } from './poller';

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
  let ioScale = $state(100); // MB/s; auto-ranged from recent samples
  const gaugeEls: Record<'cpu' | 'gpu' | 'vram' | 'io', HTMLCanvasElement | undefined> = $state({ cpu: undefined, gpu: undefined, vram: undefined, io: undefined });
  let gauges: Record<string, ReturnType<typeof createGauge>> = {};
  const sparks = new Map<string, ReturnType<typeof createGauge>>();
  const recentIo: number[] = [];
  let poller: Poller<{ result: TelemetrySample; meta: ResponseMeta }> | null = null;

  function gaugeTheme(): GaugeTheme {
    const cs = getComputedStyle(document.documentElement);
    const t = (k: string) => cs.getPropertyValue(k).trim();
    const num = t('--num-font') || t('--font-ui');
    return { track: t('--gauge-track'), value: t('--gauge-value'), glow: t('--gauge-glow'), text: t('--text'), dim: t('--muted'), unknown: t('--unknown'), fontNum: `500 19px ${num}`, fontSmall: `400 10.5px ${num}` };
  }
  const frac = (f: number | null | undefined) => (typeof f === 'number' ? f * 100 : null);
  const num = (s: string | null | undefined) => (typeof s === 'string' && /^\d+$/.test(s) ? Number(s) : null);
  const mbps = (v: number | null) => (v === null ? 'unknown' : `${(v / 1e6).toFixed(v >= 1e8 ? 0 : 1)} MB/s`);
  const rateOf = (d: { read_bytes_per_s: number | null; write_bytes_per_s: number | null }) =>
    d.read_bytes_per_s === null || d.write_bytes_per_s === null ? null : d.read_bytes_per_s + d.write_bytes_per_s;
  /** The smallest step that clears the recent peak; it only shrinks when the peak falls far below it. */
  const STEPS = [10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000, 25000, 50000];
  function rangeFor(peak: number, current: number): number {
    const want = STEPS.find((s) => s >= peak * 1.1) ?? STEPS[STEPS.length - 1]!;
    return want > current || peak < current / 4 ? want : current;
  }

  function apply(s: TelemetrySample) {
    tel = s;
    const gpu = s.gpu?.adapters[0];
    gauges.cpu?.push(frac(s.system?.cpu.busy_fraction));
    gauges.gpu?.push(frac(gpu?.engine_busy_fraction));
    const used = num(gpu?.dedicated_used_bytes), total = num(gpu?.dedicated_total_bytes);
    if (total !== null && total > 0) gauges.vram?.setRange(0, total / 2 ** 30);
    gauges.vram?.push(used === null || total === null ? null : used / 2 ** 30); // no total, no scale: unknown
    const disks = s.disks?.disks ?? [];
    const rates = disks.map(rateOf);
    const sum = rates.length && rates.every((r) => r !== null) ? (rates as number[]).reduce((a, b) => a + b, 0) / 1e6 : null;
    if (sum !== null) {
      recentIo.push(sum); if (recentIo.length > 30) recentIo.shift();
      ioScale = rangeFor(Math.max(...recentIo), ioScale);
      gauges.io?.setRange(0, ioScale);
    }
    gauges.io?.push(sum);
    disks.forEach((d, i) => { const r = rates[i] ?? null; sparks.get(d.disk_label)?.push(r === null ? null : r / 1e6); });
  }

  function fail(e: unknown) {
    telState = 'unavailable';
    telNote = e instanceof LoomwardError ? e.message : session.handle(e);
    tel = null;
    // nothing stale stays drawn: every dial reads unknown and every trace starts again
    for (const g of Object.values(gauges)) g.push(null);
    for (const g of sparks.values()) g.clear();
    recentIo.length = 0;
  }

  /** Svelte action: a sparkline per disk, keyed by the disk's own label. */
  function sparkline(node: HTMLCanvasElement, label: string) {
    const g = createGauge(node, { kind: 'spark', unit: 'MB/s', theme: gaugeTheme() });
    sparks.set(label, g);
    return { destroy() { g.destroy(); sparks.delete(label); } };
  }

  const memory = $derived.by(() => {
    const m = tel?.system?.memory;
    const total = num(m?.total_bytes), avail = num(m?.available_bytes);
    return { m, total, avail, inUse: total !== null && avail !== null ? total - avail : null };
  });

  onMount(() => {
    if (!canvas) return;
    const s = createSunburst(canvas, { palette, reducedMotion: () => reducedMQ.matches, format: formatApprox, label: canvasLabel });
    sb = s;
    const off = [
      s.on('hover', (e: { node: NodeInfo | null; viaKeyboard: boolean }) => { hovered = e.node; if (e.node && e.viaKeyboard) live = `${canvasLabel(e.node.name)}, ${formatApprox(e.node.size)}.${e.node.drillable ? ' Enter opens it.' : ''}`; }),
      s.on('select', (n: NodeInfo) => { selected = n; }),
      s.on('drill', (n: NodeInfo) => { hovered = null; void nav.drill(n.id, n.name); }),
      s.on('back', () => { hovered = null; void nav.back(); }),
    ];
    const th = gaugeTheme();
    const fmtPct = (v: number) => v.toFixed(0);
    gauges = {
      cpu: createGauge(gaugeEls.cpu!, { kind: 'arc', unit: '%', min: 0, max: 100, theme: th, format: fmtPct }),
      gpu: createGauge(gaugeEls.gpu!, { kind: 'arc', unit: '%', min: 0, max: 100, theme: th, format: fmtPct }),
      vram: createGauge(gaugeEls.vram!, { kind: 'arc', unit: 'GiB', min: 0, max: 1, theme: th }),
      io: createGauge(gaugeEls.io!, { kind: 'arc', unit: 'MB/s', min: 0, max: ioScale, theme: th, format: (v) => (v >= 1000 ? `${(v / 1000).toFixed(1)}k` : v.toFixed(0)) }),
    };
    const p = new Poller({
      call: async () => {
        const c = session.client;
        if (!c) throw new Error('no session');
        return c.call('telemetry.snapshot', { channels: ['system', 'gpu', 'disks'] });
      },
      onSample: ({ result, meta }) => { telClass = meta.dataset_class; telState = 'live'; telNote = ''; apply(result); },
      onError: (e) => fail(e),
      intervalMs: 1000,
      backoffMs: [2000, 4000, 8000, 15000, 30000],
      isHidden: () => document.hidden,
    });
    poller = p;
    p.start();
    return () => {
      p.stop(); poller = null;
      off.forEach((f) => f());
      s.destroy(); sb = null;
      Object.values(gauges).forEach((g) => g.destroy());
    };
  });

  // A new session (or stale data) restarts the slice and pokes the poller out of any backoff.
  $effect(() => {
    void session.epoch;
    if (session.client) untrack(() => { void nav.start(); poller?.kick(); });
  });
  $effect(() => {
    const s = nav.slice;
    if (!sb) return;
    const b = sb;
    if (!s) { untrack(() => { b.clear(); hovered = null; selected = null; }); return; }
    untrack(() => {
      b.setSlice(s);
      if (selected && !s.nodes.some((n) => n.node_id === selected!.id)) selected = null;
      live = `Showing ${canvasLabel(nav.here?.name ?? '')}: ${formatCount(s.nodes.length)} nodes${s.aggregate_state === 'provisional_live' ? ', provisional sums from a running scan' : ''}.`;
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

  function inspectById(id: string) {
    const n = sb?.info(id) ?? null;
    if (n) { selected = n; hovered = null; }
  }

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
        <li><button type="button" class="crumb" aria-current={i === nav.trail.length - 1 ? 'location' : undefined} onclick={() => void nav.jump(i)}><VisibleName name={c.name} /></button></li>
      {/each}
    </ol>
  </nav>
  <div class="seg" role="radiogroup" aria-label="Area represents" tabindex="-1" onkeydown={basisKey}>
    <button type="button" role="radio" data-basis="logical" aria-checked={nav.basis === 'logical'} tabindex={nav.basis === 'logical' ? 0 : -1} onclick={() => void nav.setBasis('logical')}>Logical</button>
    <button type="button" role="radio" data-basis="allocated" aria-checked={nav.basis === 'allocated'} tabindex={nav.basis === 'allocated' ? 0 : -1} onclick={() => void nav.setBasis('allocated')}>Allocated</button>
  </div>
</div>

{#if nav.error}<p class="bad" role="alert">Could not load this view: {nav.error} Nothing is drawn until a load succeeds.</p>{/if}

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
    <RegionList slice={nav.slice} onopen={(id, name) => void nav.drill(id, name)} oninspect={inspectById} />
  </section>

  <div class="side">
    <section class="resources" aria-labelledby="res-h">
      <div class="res-top">
        <h2 id="res-h">Resources</h2>
        <p class="res-note">
          <span role="status">
            {#if telState === 'unavailable'}<span class="warn">Telemetry unavailable, retrying</span>
            {:else if telState === 'waiting'}Waiting for a sample
            {:else}<span class="dot" aria-hidden="true"></span>{telClass === 'synthetic' ? 'Synthetic telemetry' : 'Observed telemetry'}{/if}
          </span>
          {#if telState === 'live' && tel}<span class="seq num" aria-hidden="true">· sample {tel.sample_seq}</span>{/if}
        </p>
      </div>
      {#if telState === 'unavailable'}<p class="muted small">{telNote} Nothing is shown in its place: every reading is unknown until a sample arrives.</p>{/if}
      <div class="gauges" class:off={telState === 'unavailable'}>
        <figure><figcaption>CPU</figcaption><canvas bind:this={gaugeEls.cpu} aria-label={`CPU busy ${frac(tel?.system?.cpu.busy_fraction)?.toFixed(0) ?? 'unknown'} percent`}></canvas></figure>
        <figure><figcaption>GPU</figcaption><canvas bind:this={gaugeEls.gpu} aria-label={`GPU busy ${frac(tel?.gpu?.adapters[0]?.engine_busy_fraction)?.toFixed(0) ?? 'unknown'} percent`}></canvas></figure>
        <figure><figcaption>VRAM</figcaption><canvas bind:this={gaugeEls.vram} aria-label={`GPU memory used ${formatBytes(tel?.gpu?.adapters[0]?.dedicated_used_bytes)} of ${formatBytes(tel?.gpu?.adapters[0]?.dedicated_total_bytes)}`}></canvas></figure>
        <figure><figcaption>Disk I/O <span class="scale num">0–{ioScale >= 1000 ? `${ioScale / 1000}k` : ioScale}</span></figcaption><canvas bind:this={gaugeEls.io} aria-label={`Disk read plus write rate, scale 0 to ${ioScale} MB/s`}></canvas></figure>
      </div>
      <div class="disks">
        {#each tel?.disks?.disks ?? [] as d (d.disk_label)}
          <div class="disk">
            <span class="dl"><VisibleName name={d.disk_label} /></span>
            <canvas use:sparkline={d.disk_label} aria-hidden="true"></canvas>
            <span class="dv num">r {mbps(d.read_bytes_per_s)} · w {mbps(d.write_bytes_per_s)}</span>
          </div>
        {/each}
      </div>
      <dl class="mem">
        <div><dt>In use</dt><dd class="num">{memory.inUse === null ? 'unknown' : formatBytes(String(memory.inUse))}</dd></div>
        <div><dt>Available</dt><dd class="num">{formatBytes(memory.m?.available_bytes)}</dd></div>
        <div><dt>Total</dt><dd class="num">{formatBytes(memory.m?.total_bytes)}</dd></div>
        <div class="wide"><dt>Commit</dt><dd class="num">{formatBytes(memory.m?.commit_bytes)} of {formatBytes(memory.m?.commit_limit_bytes)}</dd></div>
      </dl>
      {#if memory.inUse !== null && memory.avail !== null}
        <div class="membar" role="img" aria-label={`Physical memory: ${formatBytes(String(memory.inUse))} in use, ${formatBytes(String(memory.avail))} available`}>
          <span style:flex-grow={memory.inUse} class="use"></span><span style:flex-grow={memory.avail} class="avail"></span>
        </div>
      {:else}
        <div class="membar unknown-bar" role="img" aria-label="Physical memory split unknown"><span>unknown</span></div>
      {/if}
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
  .seq { color: var(--faint); }
  .scale { color: var(--faint); font-size: 0.72rem; }
  .unknown-bar { align-items: center; justify-content: center; height: 16px; font-size: 0.72rem; color: var(--unknown); border: 1px dashed var(--unknown); background: none; }
  .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--gauge-value); }
  .gauges { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 4px; margin-top: 8px; }
  .gauges.off { opacity: 0.35; }
  figure { margin: 0; text-align: center; }
  figcaption { font-size: 0.78rem; color: var(--muted); }
  figure canvas { width: 100%; height: 86px; display: block; }
  .disks { display: grid; gap: 4px; margin: 10px 0; }
  .disk { display: grid; grid-template-columns: minmax(0, 6.5rem) minmax(60px, 1fr) auto; gap: 8px; align-items: center; }
  .dl { font-size: 0.8rem; color: var(--muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
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
    .disk { grid-template-columns: minmax(0, 6.5rem) minmax(0, 1fr); }
    .dv { grid-column: 2; text-align: left; }
    .mem { grid-template-columns: 1fr; }
  }
</style>
