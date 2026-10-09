<script module lang="ts">
  export const meta = { title: 'Tiers', order: 50 };
</script>

<script lang="ts">
  import { untrack } from 'svelte';
  import { formatBytes, parseBytes } from '../../lib/format/bytes';
  import { formatCount, formatTime } from '../../lib/format/time';
  import { session } from '../../lib/stores/session.svelte';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import type { CandidateGroup, GroupOverride, HeatPolicy, PlacementCandidates, PlacementPlan, PreRejectReason, RejectReason, ReliefPolicy, TierInfo, TierModel, TierVolume } from '../../lib/types.views';

  type Outcome = { plan: PlacementPlan } | { error: string };
  interface Assumption { heat: '' | '0.1' | '0.5' | '0.9'; free: boolean }

  const HEAT: { id: HeatPolicy; label: string; note: string }[] = [
    { id: 'unknown_is_ineligible', label: 'Unknown heat is ineligible', note: 'The default. Nothing is guessed to be cold.' },
    { id: 'mtime_proxy_whatif', label: 'Modification age stands in for heat', note: 'What-if only: age is not access heat.' },
    { id: 'assumed_only', label: 'Only the heat I assume below', note: 'Uses nothing but the what-if choices in the table.' },
  ];
  const REJECTED: Record<RejectReason, string> = {
    not_on_source: 'Not on the source volume',
    pinned_active_protected_or_unspecified: 'Pinned, active, protected, or not stated (unknown counts as set)',
    heat_unknown: 'Heat is unknown',
    not_cold: 'Not cold enough',
    cooldown_or_history_unknown: 'Moved too recently, or move history is unknown',
    source_offline_or_readonly: 'Source is offline or read-only',
    not_selected_by_bounded_allocator: 'Eligible, but not needed to reach the target',
  };
  const PRE_REJECTED: Record<PreRejectReason, string> = {
    relief_unknown: 'Space relief cannot be verified, so it is unknown rather than zero',
    shares_objects_with_other_group: 'Shares file objects with another group (hard links)',
    group_coverage_incomplete: 'Scan coverage of this folder is incomplete',
  };
  const EXCLUDED = { tier_unknown: 'Tier unknown', capacity_unknown: 'Capacity unknown', offline: 'Offline' } as const;
  const PRESSURE = { ok: 'Comfortable', watch: 'Watch', pressure: 'Under pressure', unknown: 'Unknown' } as const;
  const BUS: Record<string, string> = { nvme: 'NVMe', sata: 'SATA', sas: 'SAS', scsi: 'SCSI', usb: 'USB', raid: 'RAID', virtual: 'virtual', sd: 'SD', other: 'other bus', unknown: 'unknown bus' };

  let model = $state<TierModel | null>(null);
  let source = $state('');
  let cands = $state<PlacementCandidates | null>(null);
  let error = $state('');
  let loading = $state(true);

  let pct = $state<number | null>(20);
  let transferGiB = $state<number | null>(400);
  let relief = $state<ReliefPolicy>('verified_only');
  let assume = $state<Record<string, Assumption>>({});
  let outcomes = $state<Record<HeatPolicy, Outcome> | null>(null);
  let shown = $state<'none' | HeatPolicy>('unknown_is_ineligible');
  let simulating = $state(false);
  let formError = $state('');
  let modelTicket = 0;
  let candTicket = 0;
  let simTicket = 0;

  const volume = (id: string): TierVolume | undefined => model?.volumes.find((v) => v.volume_id === id);
  const vname = (id: string) => volume(id)?.display_name ?? id;
  const src = $derived(volume(source));
  const group = (id: string): CandidateGroup | undefined => cands?.groups.find((g) => g.group_id === id);
  const percent = (f: number | null) => (f === null ? 'unknown' : `${(f * 100).toFixed(1)}%`);

  function tierLabel(t: TierInfo): string {
    return t.tier === null ? 'Tier unknown' : `Tier ${t.tier} · ${t.basis === 'declared' ? 'declared' : 'hinted'}`;
  }

  async function loadCandidates(): Promise<void> {
    const c = session.client;
    const v = src;
    outcomes = null;
    cands = null;
    if (!c || !v) return;
    const mine = ++candTicket;
    try {
      const { result } = await c.call('placement.candidates', { source_volume_id: v.volume_id, basis: 'largest_dirs', max_groups: 50, min_bytes: '0' });
      if (mine === candTicket) cands = result;
    } catch (e) {
      if (mine === candTicket) error = session.handle(e);
    }
  }

  async function loadModel(): Promise<void> {
    const c = session.client;
    if (!c) return;
    const mine = ++modelTicket;
    loading = true;
    try {
      const { result } = await c.call('tiers.model', {});
      if (mine !== modelTicket) return;
      model = result;
      error = '';
      const keep = result.volumes.find((v) => v.volume_id === source && v.tier.tier !== null);
      source = (keep ?? result.volumes.find((v) => v.pressure === 'pressure' && v.tier.tier !== null) ?? result.volumes.find((v) => v.tier.tier !== null))?.volume_id ?? '';
      await loadCandidates();
    } catch (e) {
      if (mine === modelTicket) { model = null; error = session.handle(e); }
    } finally {
      if (mine === modelTicket) loading = false;
    }
  }

  function choose(id: string) {
    source = id;
    assume = {};
    void loadCandidates();
  }

  $effect(() => {
    void session.epoch;
    untrack(() => void loadModel());
  });

  function overrides(): GroupOverride[] {
    const out: GroupOverride[] = [];
    for (const [group_id, a] of Object.entries(assume)) {
      const o: GroupOverride = { group_id };
      if (a.heat) o.heat = Number(a.heat);
      if (a.free) { o.pinned = false; o.active = false; o.protected = false; }
      if (a.heat || a.free) out.push(o);
    }
    return out;
  }

  async function simulate() {
    const c = session.client;
    const cap = parseBytes(src?.capacity_bytes);
    formError = '';
    if (!c || !src || cap === null) { formError = 'The source volume has no known capacity, so there is nothing to simulate.'; return; }
    if (pct === null || !(pct >= 1 && pct <= 90)) { formError = 'Target free space must be between 1% and 90% of the volume.'; return; }
    if (transferGiB === null || !(transferGiB >= 0 && transferGiB <= 100000)) { formError = 'The transfer cap must be between 0 and 100,000 GiB.'; return; }
    const target = (cap * BigInt(Math.round(pct * 10))) / 1000n;
    const cap_ = BigInt(Math.round(transferGiB)) * 2n ** 30n;
    const mine = ++simTicket;
    simulating = true;
    const base = { source_volume_id: src.volume_id, target_free_bytes: target.toString(), max_transfer_bytes: cap_.toString(), relief_policy: relief, candidate_basis: 'largest_dirs' as const, max_groups: 50, overrides: overrides(), node_budget: 100000, save: false };
    const settled = await Promise.allSettled(HEAT.map((h) => c.call('placement.simulate', { ...base, heat_policy: h.id })));
    if (mine !== simTicket) return;
    simulating = false;
    outcomes = Object.fromEntries(HEAT.map((h, i) => {
      const s = settled[i]!;
      return [h.id, s.status === 'fulfilled' ? { plan: s.value.result } : { error: session.handle(s.reason) }];
    })) as Record<HeatPolicy, Outcome>;
    shown = 'unknown_is_ineligible';
  }

  /** The plan is only trusted for display when it says it is a simulation that changed nothing. */
  const sound = (p: PlacementPlan) => p.mode === 'simulation' && p.filesystem_changed === false && p.proposals.every((x) => x.executable === false);

  const detail = $derived(outcomes && shown !== 'none' ? outcomes[shown] : null);
  const detailPlan = $derived(detail && 'plan' in detail ? detail.plan : null);
  const firstPlan = $derived(outcomes ? Object.values(outcomes).find((o): o is { plan: PlacementPlan } => 'plan' in o)?.plan ?? null : null);
  const baseline = $derived(firstPlan ? BigInt(firstPlan.baseline_shortfall_bytes) : null);
</script>

<h1>Tiers</h1>
<p class="simbanner" role="note"><strong>Simulation only.</strong> This page shows what a placement plan could look like. Nothing is moved, and Loomward v0.3 cannot move anything.</p>
{#if error}<p class="bad" role="alert">{error}</p>{/if}

<section class="panel" aria-labelledby="h-vol">
  <h2 id="h-vol">Volumes</h2>
  {#if model}
    <div class="vols">
      {#each model.volumes as v (v.volume_id)}
        {@const used = v.free_fraction === null ? null : 1 - v.free_fraction}
        <article class="vol" class:unknown-tier={v.tier.tier === null} class:chosen={v.volume_id === source}>
          <header>
            <h3><VisibleName name={v.display_name} /></h3>
            <span class="tag tier" title={v.tier.note}>{tierLabel(v.tier)}</span>
            {#if !v.online}<span class="tag bad">offline</span>{/if}
          </header>
          <div class="meter" role="img" aria-label={`${v.display_name} free space ${percent(v.free_fraction)}; watch below ${percent(model.policy.watch_free_fraction)}, pressure below ${percent(model.policy.pressure_free_fraction)}`}>
            {#if used !== null}<div class={`used ${v.pressure}`} style={`width:${(used * 100).toFixed(1)}%`}></div>{/if}
            <span class="tick" style={`left:${((1 - model.policy.watch_free_fraction) * 100).toFixed(1)}%`} title="watch watermark"></span>
            <span class="tick hard" style={`left:${((1 - model.policy.pressure_free_fraction) * 100).toFixed(1)}%`} title="pressure watermark"></span>
          </div>
          <p class="num">{formatBytes(v.free_bytes)} free of {formatBytes(v.capacity_bytes)} · {percent(v.free_fraction)} <span class={`tag pr-${v.pressure}`}>{PRESSURE[v.pressure]}</span></p>
          <p class="muted small">{v.tier.note}</p>
          <button class="btn" type="button" aria-pressed={v.volume_id === source} disabled={v.tier.tier === null || !v.online} onclick={() => choose(v.volume_id)}>
            {v.volume_id === source ? 'Source volume' : 'Use as source'}
          </button>
        </article>
      {/each}
    </div>
    <p class="muted small">Watermarks: watch below {percent(model.policy.watch_free_fraction)} free, pressure below {percent(model.policy.pressure_free_fraction)} free. {model.policy.reserve_note}</p>
    <p class="muted small">{model.policy.note}</p>
  {:else}<p class="muted">{loading ? 'Loading' : 'No volumes to show.'}</p>{/if}
</section>

<section class="panel" aria-labelledby="h-cand">
  <h2 id="h-cand">Candidate groups{#if src} on <VisibleName name={src.display_name} />{/if}</h2>
  {#if cands}
    <p class="muted small">{cands.note}</p>
    <div class="tbl-wrap">
      <table class="tbl">
        <caption class="sr-only">Candidate folder groups with what-if assumptions</caption>
        <thead>
          <tr>
            <th scope="col">Group</th><th scope="col" class="r">By entry</th><th scope="col" class="r">Space freed</th><th scope="col" class="r">Needs there</th>
            <th scope="col">Heat</th><th scope="col">Newest change</th><th scope="col">Observed state</th><th scope="col">What if</th>
          </tr>
        </thead>
        <tbody>
          {#each cands.groups as g (g.group_id)}
            {@const a = assume[g.group_id] ?? { heat: '', free: false }}
            <tr>
              <td><VisibleName name={g.name} />{#if g.coverage !== 'complete'} <span class="tag warn">{g.coverage}</span>{/if}</td>
              <td class="r num">{formatBytes(g.source_bytes)}<div class="small"><span class="tag" class:warn={g.estimate_basis !== 'allocated_entries'}>{g.estimate_basis.replaceAll('_', ' ')}</span></div></td>
              <td class="r num">
                {#if g.estimated_relief_bytes === null}<span class="unknown">unknown</span>{:else}{formatBytes(g.estimated_relief_bytes)}{/if}
                <div class="small"><span class="tag" class:warn={g.relief_basis !== 'verified_unique_allocation'}>{g.relief_basis.replaceAll('_', ' ')}</span></div>
              </td>
              <td class="r num">{formatBytes(g.destination_bytes)}<div class="small muted">estimate</div></td>
              <td>{#if g.heat === null}<span class="unknown">unknown</span>{:else}<span class="num">{g.heat.toFixed(2)}</span>{/if} <span class="tag">{g.heat_basis.replaceAll('_', ' ')}</span></td>
              <td class="nowrap">{formatTime(g.newest_modified_at)}</td>
              <td>
                {#each [['pinned', g.pinned], ['active', g.active], ['protected', g.protected]] as [label, v] (label)}
                  <span class="tag" class:warn={v === true}>{label}: {v === null ? 'unknown' : v ? 'yes' : 'no'}</span>
                {/each}
              </td>
              <td class="whatif">
                <select aria-label={`Assumed heat for ${g.name}`} value={a.heat} onchange={(e) => (assume[g.group_id] = { ...a, heat: e.currentTarget.value as Assumption['heat'] })}>
                  <option value="">Heat: as observed</option><option value="0.1">Assume cold (0.1)</option><option value="0.5">Assume warm (0.5)</option><option value="0.9">Assume hot (0.9)</option>
                </select>
                <label class="chk"><input type="checkbox" checked={a.free} onchange={(e) => (assume[g.group_id] = { ...a, free: e.currentTarget.checked })} /> Assume not pinned, active or protected</label>
              </td>
            </tr>
          {:else}
            <tr><td colspan="8" class="muted">No candidate groups on this volume.</td></tr>
          {/each}
        </tbody>
      </table>
    </div>
    <p class="muted small">What-if choices are assumptions. They are sent to the simulation and recorded in its result; they never change a volume, a capacity or a file.</p>
  {:else}<p class="muted">{src ? 'Loading' : 'Choose a source volume with a known tier.'}</p>{/if}
</section>

<section class="panel" aria-labelledby="h-sim">
  <h2 id="h-sim">Simulate</h2>
  <form class="params" onsubmit={(e) => { e.preventDefault(); void simulate(); }}>
    <label class="field">Free space wanted on the source <span class="inline"><input type="number" min="1" max="90" step="0.5" bind:value={pct} /> %</span></label>
    <label class="field">Most it may move <span class="inline"><input type="number" min="0" max="100000" step="10" bind:value={transferGiB} /> GiB</span></label>
    <label class="field">Space freed counted from
      <select bind:value={relief}>
        <option value="verified_only">Verified allocation only</option>
        <option value="entry_allocation_whatif">Entry allocation (what-if)</option>
      </select>
    </label>
    <button class="btn primary" type="submit" disabled={simulating || !cands || !cands.groups.length}>{simulating ? 'Simulating' : 'Simulate'}</button>
  </form>
  {#if formError}<p class="bad" role="alert">{formError}</p>{/if}

  {#if outcomes}
    {@const none = baseline}
    <h3>Alternatives</h3>
    <div class="tbl-wrap">
      <table class="tbl alts">
        <caption class="sr-only">Alternatives, including doing nothing</caption>
        <thead><tr><th scope="col">Choice</th><th scope="col" class="r">Groups</th><th scope="col" class="r">Moves</th><th scope="col" class="r">Still short</th><th scope="col">Target</th><th scope="col"><span class="sr-only">Show</span></th></tr></thead>
        <tbody>
          <tr class:current={shown === 'none'}>
            <th scope="row">Do nothing<div class="muted small">Always a valid outcome. Nothing changes.</div></th>
            <td class="r num">0</td><td class="r num">0 B</td>
            <td class="r num">{none === null ? 'unknown' : formatBytes(none.toString())}</td>
            <td>{none === 0n ? 'Already met' : 'Not met'}</td>
            <td><button class="btn" type="button" aria-pressed={shown === 'none'} onclick={() => (shown = 'none')}>Show</button></td>
          </tr>
          {#each HEAT as h (h.id)}
            {@const o = outcomes[h.id]}
            <tr class:current={shown === h.id}>
              <th scope="row">{h.label}<div class="muted small">{h.note}</div></th>
              {#if 'plan' in o && sound(o.plan)}
                <td class="r num">{o.plan.proposals.length}</td><td class="r num">{formatBytes(o.plan.transfer_bytes)}</td>
                <td class="r num">{formatBytes(o.plan.shortfall_bytes)}</td><td>{o.plan.satisfied ? 'Met' : 'Not met'}</td>
              {:else}
                <td colspan="4" class="bad">{'plan' in o ? 'Not shown: the reply was not a no-change simulation.' : o.error}</td>
              {/if}
              <td><button class="btn" type="button" aria-pressed={shown === h.id} onclick={() => (shown = h.id)}>Show</button></td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>

{#if outcomes && shown === 'none'}
  <section class="panel" aria-labelledby="h-none">
    <h2 id="h-none">Do nothing</h2>
    <p class="simlabel">Simulation · executable: false · filesystem_changed: false</p>
    <p>Leaving everything as it is keeps {src ? vname(src.volume_id) : 'the source'} at {formatBytes(src?.free_bytes)} free. {baseline && baseline > 0n ? `That is ${formatBytes(baseline.toString())} short of the target.` : 'The target is already met.'}</p>
  </section>
{:else if detail && 'error' in detail}
  <section class="panel"><p class="bad" role="alert">{detail.error}</p></section>
{:else if detailPlan && !sound(detailPlan)}
  <section class="panel"><p class="bad" role="alert">The engine's reply was not a simulation that leaves the filesystem unchanged, so it is not shown.</p></section>
{:else if detailPlan}
  {@const p = detailPlan}
  <section class="panel" aria-labelledby="h-plan">
    <h2 id="h-plan">Placement plan</h2>
    <p class="simlabel">Simulation · executable: {String(p.proposals.some((x) => x.executable))} · filesystem_changed: {String(p.filesystem_changed)}</p>
    <dl class="kv">
      <dt>Target free</dt><dd class="num">{formatBytes(p.target_free_bytes)} on {src ? vname(src.volume_id) : 'the source'}</dd>
      <dt>Result</dt><dd><strong>{p.satisfied ? 'Target met in the simulation' : `Still ${formatBytes(p.shortfall_bytes)} short`}</strong>
        <span class="muted"> (was {formatBytes(p.baseline_shortfall_bytes)} short; improvement {formatBytes(p.shortfall_improvement_bytes)})</span></dd>
      <dt>Would move</dt><dd class="num">{formatBytes(p.transfer_bytes)} across {formatCount(p.proposals.length)} groups</dd>
      <dt>Space freed counted from</dt><dd>{p.relief_policy === 'verified_only' ? 'verified allocation only' : 'entry allocation (what-if, not verified)'}</dd>
      <dt>Search</dt><dd>{p.search.complete ? 'Complete' : 'Incomplete'} ({p.search.reason.replaceAll('_', ' ')}); {formatCount(p.search.nodes_visited)} of {formatCount(p.search.node_budget)} steps; {formatCount(p.search.eligible_groups)} eligible groups, {formatCount(p.search.eligible_targets)} eligible targets</dd>
      <dt>Optimality</dt><dd>{p.optimality_claim ? 'Claimed' : 'Not claimed'}: {p.optimality_scope}</dd>
    </dl>

    <h3>Projected free space</h3>
    <ul class="list">
      {#each Object.entries(p.projected_free_bytes) as [id, bytes] (id)}
        <li><VisibleName name={vname(id)} />: <span class="num">{formatBytes(volume(id)?.free_bytes)}</span> now, <span class="num">{formatBytes(bytes)}</span> in this simulation</li>
      {/each}
    </ul>

    <h3>Proposals <span class="muted small">(each needs consent and cannot be executed here)</span></h3>
    {#if p.proposals.length}
      <div class="tbl-wrap">
        <table class="tbl">
          <caption class="sr-only">Proposed placements</caption>
          <thead><tr><th scope="col">Group</th><th scope="col">From</th><th scope="col">To</th><th scope="col" class="r">Frees</th><th scope="col" class="r">Needs there</th><th scope="col" class="r">Transfer</th><th scope="col">Status</th></tr></thead>
          <tbody>
            {#each p.proposals as x (x.group_id)}
              <tr>
                <td><VisibleName name={group(x.group_id)?.name ?? x.group_id} /></td>
                <td><VisibleName name={vname(x.source_id)} /></td><td><VisibleName name={vname(x.target_id)} /></td>
                <td class="r num">{formatBytes(x.source_bytes_relieved)}</td><td class="r num">{formatBytes(x.destination_bytes_required)}</td><td class="r num">{formatBytes(x.transfer_bytes)}</td>
                <td><span class="tag">consent required</span> <span class="tag">not executable</span></td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {:else}<p class="muted">No group qualified under this policy, so the plan does nothing.</p>{/if}

    {#if p.pre_rejected.length || p.rejected.length}
      <h3>Not proposed</h3>
      <ul class="list">
        {#each p.pre_rejected as x (x.group_id)}<li><VisibleName name={group(x.group_id)?.name ?? x.group_id} />: {PRE_REJECTED[x.reason]}</li>{/each}
        {#each p.rejected as x (x.group_id)}<li><VisibleName name={group(x.group_id)?.name ?? x.group_id} />: {REJECTED[x.reason]}</li>{/each}
      </ul>
    {/if}
    {#if p.excluded_volumes.length}
      <h3>Left out of the plan</h3>
      <ul class="list">{#each p.excluded_volumes as x (x.volume_id)}<li><VisibleName name={vname(x.volume_id)} />: {EXCLUDED[x.reason]}</li>{/each}</ul>
    {/if}
    <h3>Assumptions</h3>
    <ul class="list">
      <li>{p.assumption}</li>
      {#each p.assumptions as a (a)}<li>{a}</li>{/each}
    </ul>
  </section>
{/if}

<style>
  .simbanner { border: 1px dashed var(--permission); border-radius: var(--radius); padding: 8px 12px; background: var(--surface); }
  .simlabel { display: inline-block; font-family: var(--font-mono); font-size: 0.85rem; border: 1px solid var(--permission); border-radius: 999px; padding: 2px 12px; margin: 0 0 12px; }
  .bad { color: var(--danger); }
  .small { font-size: 0.8rem; }
  .vols { display: grid; grid-template-columns: repeat(auto-fill, minmax(250px, 1fr)); gap: 12px; margin-bottom: 8px; }
  .vol { border: 1px solid var(--line); border-radius: var(--radius); padding: 12px; background: var(--bg); }
  .vol.chosen { border-color: var(--accent); }
  .vol.unknown-tier { border-style: dashed; }
  .vol header { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; margin-bottom: 8px; }
  .vol h3 { margin: 0; font-size: 1.1rem; }
  .vol p { margin: 6px 0; }
  .tag.tier { color: var(--residency); border: 1px dashed var(--residency); }
  .meter { position: relative; height: 12px; border: 1px solid var(--line); border-radius: 6px; background: var(--raised); overflow: hidden; }
  .used { height: 100%; background: var(--residency); opacity: 0.75; }
  .used.watch { background: var(--warn); }
  .used.pressure { background: repeating-linear-gradient(135deg, var(--danger) 0 4px, color-mix(in srgb, var(--danger) 45%, transparent) 4px 8px); }
  .used.unknown { background: transparent; }
  .tick { position: absolute; top: 0; bottom: 0; width: 0; border-left: 1px dotted var(--text); }
  .tick.hard { border-left-style: solid; }
  .tag.pr-pressure { color: var(--danger); border-color: var(--danger); }
  .tag.pr-watch { color: var(--warn); border-color: var(--warn); }
  .whatif { min-width: 15rem; }
  .chk { display: block; font-size: 0.8rem; color: var(--muted); margin-top: 4px; }
  .params { display: flex; flex-wrap: wrap; gap: 12px 20px; align-items: end; margin-bottom: 12px; }
  .field { display: flex; flex-direction: column; gap: 4px; font-size: 0.9rem; }
  .field .inline { display: inline-flex; align-items: center; gap: 6px; }
  .field input[type='number'] { width: 7rem; font: inherit; color: var(--text); background: var(--bg); border: 1px solid var(--line); border-radius: var(--radius); padding: 6px 10px; }
  .btn.primary { background: var(--accent); color: var(--accent-ink); border-color: var(--accent); }
  .alts tr.current th, .alts tr.current td { background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .alts th[scope='row'] { text-align: left; font-weight: 600; text-transform: none; letter-spacing: 0; font-size: inherit; color: var(--text); }
  .list { margin: 0 0 12px; padding-left: 1.1em; }
  .list li { margin-bottom: 4px; overflow-wrap: anywhere; }
  .kv { display: grid; grid-template-columns: max-content 1fr; gap: 4px 20px; margin: 0 0 12px; }
  .kv dt { color: var(--muted); }
  .kv dd { margin: 0; }
  @media (max-width: 600px) { .kv { grid-template-columns: 1fr; gap: 0; } .kv dd { margin-bottom: 8px; } }
</style>
