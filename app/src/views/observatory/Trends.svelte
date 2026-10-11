<script lang="ts">
  import type { TelemetrySample, TrendMetric, ProcessRow } from '../../lib/contracts.gen';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import { formatDuration } from '../../lib/format/time';
  import { segments, trendLabel } from './trends';

  let { sample, paused, rows = [] }: { sample: TelemetrySample | null; paused: boolean; rows?: ProcessRow[] } = $props();
  let metric = $state<TrendMetric>('cpu_busy_fraction');
  let key = $state('system');
  const report = $derived(sample?.trends);
  const metrics = $derived([...new Set(report?.series.map(s => s.metric) ?? [])]);
  const selectedMetric = $derived(metrics.includes(metric) ? metric : metrics[0]);
  const matching = $derived(report?.series.filter(s => s.metric === selectedMetric) ?? []);
  const series = $derived(matching.find(s => s.key === key) ?? matching[0]);
  const paths = $derived(series ? segments(series) : []);
  const windowMs = $derived(series && series.points.length ? series.points.at(-1)!.offset_ms - series.points[0]!.offset_ms : null);
  const words = (s: string) => s.replaceAll('_', ' ');
  const name = (key: string) => rows.find(r => r.process_ref === key)?.name ?? key;
</script>

<section class="trends" aria-labelledby="trends-h">
  <h2 id="trends-h">Recent trends</h2>
  <p class="fine">{paused ? 'Paused; retained observation window.' : 'Recent leased observations.'} Candidates describe changes and contention; workload intent remains unknown.</p>
  {#if report?.series.length}
    <div class="controls">
      <label>Trend metric <select value={selectedMetric ?? ''} onchange={e => metric = e.currentTarget.value as TrendMetric}>{#each metrics as m}<option value={m}>{words(m)}</option>{/each}</select></label>
      <label>Observed series <select value={series?.key ?? ''} onchange={e => key = e.currentTarget.value}>{#each matching as s}<option value={s.key}>{s.key}</option>{/each}</select></label>
    </div>
    {#if series}
      <figure>
        <figcaption><VisibleName name={name(series.key)} /> · {words(series.metric)} · {words(series.unit)} · {formatDuration(windowMs)} · {series.points.length} samples</figcaption>
        <svg viewBox="0 0 100 40" role="img" aria-label={`${words(series.metric)} in ${words(series.unit)} over ${formatDuration(windowMs)}; ${series.points.length} samples. Breaks indicate gaps.`}>
          {#each paths as d}<path {d} />{/each}
        </svg>
        {#if !paths.length}<p class="unknown">No known values in this window</p>{/if}
        <p class="fine">{series.points[0]?.observed_at ?? 'unknown'} → {series.points.at(-1)?.observed_at ?? 'unknown'}</p>
      </figure>
      {#each series.unknowns as unknown}<p class="unknown">{unknown}</p>{/each}
      <details><summary>Sample facts</summary><ul class="samples">{#each series.points as p}<li><time datetime={p.observed_at}>{p.observed_at}</time> · {trendLabel(p.value, series.unit)}</li>{/each}</ul></details>
    {/if}
  {:else}<p class="unknown">Trend history unavailable or awaiting leased samples.</p>{/if}
  <h3>Explanations</h3>
  {#if report?.candidates.length}
    <ul class="explanations">
      {#each report.candidates as c}
        {@const source = report.series[c.series_index]}
        <li>
          <strong>{words(c.kind)} candidate</strong> · <VisibleName name={source ? name(source.key) : 'unknown series'} /> · {source?.key ?? 'unknown series'} · {source ? words(source.metric) : 'unknown metric'}
          <p class="num">{trendLabel(c.start_value, c.unit)} → {trendLabel(c.end_value, c.unit)} · {formatDuration(c.window_ms)} · {c.sample_count} samples · slope {c.slope_per_s} {words(c.unit)}/s</p>
          <p class="fine"><time datetime={c.start_at}>{c.start_at}</time> → <time datetime={c.end_at}>{c.end_at}</time></p>
          <ul>{#each c.facts as fact}<li>{fact}</li>{/each}</ul>
          {#each c.unknowns as unknown}<p class="unknown">Unknown: {unknown}</p>{/each}
        </li>
      {/each}
    </ul>
  {:else}<p class="fine">No candidate in the observed windows; this does not establish absence of a problem.</p>{/if}
  {#if report?.candidates_truncated}<p class="unknown">Candidate list truncated at 64; other candidates may be omitted.</p>{/if}
  {#each report?.unknowns ?? [] as unknown}<p class="unknown">{unknown}</p>{/each}
</section>

<style>
  h2, h3 { margin: 0 0 8px; }
  h3 { margin-top: 18px; font-size: 0.9rem; }
  .trends { min-width: 0; border-top: 1px solid var(--line); padding-top: 16px; }
  .controls { display: grid; gap: 8px; }
  label { display: grid; gap: 4px; color: var(--muted); font-size: 0.8rem; }
  select { min-width: 0; width: 100%; }
  figure { margin: 12px 0; }
  figcaption, .fine, .unknown { font-size: 0.78rem; overflow-wrap: anywhere; }
  .fine, figcaption { color: var(--muted); }
  .unknown { color: var(--unknown); }
  svg { width: 100%; height: 90px; display: block; background: var(--raised); }
  path { fill: none; stroke: var(--gauge-value); stroke-width: 1.5; vector-effect: non-scaling-stroke; stroke-linecap: round; }
  .samples, .explanations { padding-left: 18px; font-size: 0.8rem; overflow-wrap: anywhere; }
  .explanations > li { margin: 12px 0; }
  .explanations p { margin: 6px 0; }
  @media (forced-colors: active) { path { stroke: CanvasText; } }
</style>
