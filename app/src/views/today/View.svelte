<script module lang="ts">
  export const meta = { title: 'Today', order: 10 };
</script>

<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { scans } from '../../lib/stores/scan.svelte';
  import { session } from '../../lib/stores/session.svelte';
  import { volumes } from '../../lib/stores/volumes.svelte';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import type { AlertReport, Job, ReclaimPlan, Root } from '../../lib/contracts.gen';
  import { linkName, missingSources, moreViews, needsEmptyText, needsYou, reclaimTargets, SECTION_TITLES, unevaluatedAlerts, unevaluatedLine, unplacedRoots, volumeRows, type Sources } from './model';

  const JOBS_LIMIT = 100;
  const SHOWN = 5; // per section; the rest is a link to each view that lists them
  const REFRESH_MS = 250;

  let roots = $state<Root[] | null>(null);
  let alerts = $state<AlertReport | null>(null);
  let jobs = $state<Job[] | null>(null);
  let reclaim = $state(new Map<string, ReclaimPlan | null>());
  let failedReclaim = $state(0);
  let loadProblems = $state<string[]>([]);
  let reclaimProblems = $state<string[]>([]);
  let loaded = $state(false);
  let refreshed = $state(0); // bumped when a load ends, so the reclaim plans are read again with it
  let now = $state(Date.now());
  let ticket = 0;
  let reclaimTicket = 0;
  let alive = true;
  let started = false;
  let timer: ReturnType<typeof setTimeout> | undefined;

  async function load(): Promise<void> {
    const c = session.client;
    if (!c || !alive) return;
    const epoch = session.connectionEpoch;
    const mine = ++ticket;
    const [r, a, j] = await Promise.allSettled([c.call('roots.list', {}), c.call('alerts.list', {}), c.call('jobs.list', { limit: JOBS_LIMIT })]);
    if (!alive || mine !== ticket || c !== session.client || epoch !== session.connectionEpoch) return;
    roots = r.status === 'fulfilled' ? r.value.result.roots : null;
    alerts = a.status === 'fulfilled' ? a.value.result : null;
    jobs = j.status === 'fulfilled' ? j.value.result.jobs : null;
    loadProblems = [r, a, j].flatMap((x) => (x.status === 'rejected' ? [session.handle(x.reason)] : []));
    loaded = true;
    refreshed++;
  }

  async function loadReclaim(ids: string[]): Promise<void> {
    const c = session.client;
    const epoch = session.connectionEpoch; // the client object is constant across reconnects, the epoch is not
    const mine = ++reclaimTicket;
    reclaimProblems = []; // each round replaces the last round's errors, they do not pile up
    if (!c || !ids.length) { reclaim = new Map(); failedReclaim = 0; return; }
    const settled = await Promise.allSettled(ids.map((id) => c.call('reclaim.plan', { volume_id: id, limit: 20 })));
    if (!alive || mine !== reclaimTicket || c !== session.client || epoch !== session.connectionEpoch) return;
    reclaim = new Map(settled.map((s, i) => [ids[i]!, s.status === 'fulfilled' ? s.value.result : null] as const));
    failedReclaim = settled.filter((s) => s.status === 'rejected').length;
    reclaimProblems = settled.flatMap((s) => (s.status === 'rejected' ? [session.handle(s.reason)] : []));
  }

  // The first read is immediate. Later triggers (a scan end fires both scans.settled and job.state, a running scan fires
  // job.state about every 100 ms) share one pending timer: a throttle, so a long burst still reads once per window.
  // A trigger inside the window is covered because the pending read starts after it.
  function refresh(): void {
    if (!started) { started = true; void load(); return; }
    if (timer !== undefined) return;
    timer = setTimeout(() => { timer = undefined; void load(); }, REFRESH_MS);
  }
  function onEvent(ev: { event: string }): void { if (ev.event === 'job.state') refresh(); }

  onMount(() => () => { alive = false; ticket++; reclaimTicket++; clearTimeout(timer); });
  $effect(() => {
    void session.epoch;
    void scans.settled;
    untrack(refresh);
  });
  $effect(() => session.listen(onEvent));
  $effect(() => {
    const t = setInterval(() => { now = Date.now(); }, 30_000); // ages ("5 min ago") move without a reload
    return () => clearInterval(t);
  });

  // Reclaim plans are read only for nearly full volumes that hold a granted folder, and only when that set or a load changes.
  const targetKey = $derived(volumes.items && roots && alerts ? reclaimTargets(volumes.items, roots, alerts).join('\n') : '');
  $effect(() => {
    void refreshed;
    const ids = targetKey ? targetKey.split('\n') : [];
    untrack(() => void loadReclaim(ids));
  });

  const problems = $derived([...loadProblems, ...reclaimProblems]);
  const settled = $derived(loaded && (volumes.items !== null || volumes.error !== ''));
  const src = $derived<Sources>({ volumes: volumes.items, roots, alerts, jobs, reclaim });
  const rows = $derived(volumes.items ? volumeRows(volumes.items, roots, now) : []);
  const needs = $derived(needsYou(src, now));
  const unevaluated = $derived(unevaluatedAlerts(alerts));
  const unevaluatedNote = $derived(unevaluatedLine(unevaluated));
  const missing = $derived(settled ? missingSources(src, failedReclaim) : []);
  const nothingRead = $derived(settled && !volumes.items && !roots && !alerts && !jobs);
  const noGrant = $derived(roots !== null && !roots.some((r) => r.grant_state === 'active'));
  const unplaced = $derived(volumes.items && roots ? unplacedRoots(volumes.items, roots) : 0);
  const jobsCut = $derived(jobs !== null && jobs.length >= JOBS_LIMIT);
  const percent = (f: number) => `${(f * 100).toFixed(0)}%`;
</script>

<h1>Today</h1>
<p class="lede">How your disks are, and what needs you. Everything here is read from what Loomward already observed.</p>
<p class="note">Read-only: nothing on this page moves, deletes or changes anything. <strong>Unknown is not zero:</strong> a figure that was not observed, or a source that could not be read, is named below.</p>

{#if !settled}
  <p class="empty" role="status">Reading volumes, granted folders and alerts.</p>
{:else if nothingRead}
  <section class="panel" role="alert" aria-labelledby="today-down">
    <h2 id="today-down">Today could not read anything</h2>
    <p>The engine answered, but not to any of the questions this page asks, so it shows nothing rather than guess.</p>
    {#each problems as p, n (n)}<p class="muted small">{p}</p>{/each}
  </section>
{:else}
  {#if missing.length}
    <div class="note warn" role="status">
      <strong>Partial.</strong> Not read: {missing.join(', ')}. What follows is only what arrived; nothing is assumed about the rest.
      {#each problems as p, n (n)}<span class="small"> {p}</span>{/each}
    </div>
  {/if}

  <section class="panel" aria-labelledby="today-vols">
    <h2 id="today-vols">Volumes</h2>
    {#if volumes.items === null}
      <p class="empty">Volumes could not be read{volumes.error ? `: ${volumes.error}` : '.'}</p>
    {:else if !rows.length}
      <p class="empty">No volume was observed.</p>
    {:else}
      <div class="tbl-wrap">
        <table class="tbl">
          <caption class="sr-only">One row per volume: free space, tier, and when its granted folders were last scanned</caption>
          <thead><tr><th scope="col">Volume</th><th scope="col">Space</th><th scope="col">Tier</th><th scope="col">Last scan</th></tr></thead>
          <tbody>
            {#each rows as v (v.id)}
              <tr class:offline={!v.online}>
                <th scope="row">
                  <VisibleName name={v.name} />
                  {#if !v.online}<span class="tag bad">Offline</span>{/if}
                  {#each v.unknowns as u (u)}<small>{u}</small>{/each}
                  <small class="muted" class:unknown={v.healthUnknown}>{v.health}</small>
                </th>
                <td>
                  <div class="meter" class:blank={v.used === null} role="img"
                    aria-label={!v.online ? 'Offline: no current fill level' : v.used === null ? 'Fill level unknown' : `${percent(v.used)} full`}>
                    {#if v.used !== null}<div class="used" class:tight={v.used > 0.9} style={`width:${(v.used * 100).toFixed(1)}%`}></div>{/if}
                  </div>
                  <span class="num" class:unknown={v.used === null}>{v.space}</span>
                </td>
                <td><span class="tag" class:unk={v.tier === 'Tier unknown'}>{v.tier}</span></td>
                <td class:unknown={v.scan.startsWith('at an unknown time') || v.scan === 'unknown' || v.scan === 'Never scanned' || v.scan === 'No granted folder'}>{v.scan}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      {#if unplaced}
        <p class="muted small">{unplaced} granted {unplaced === 1 ? 'folder sits' : 'folders sit'} on no volume listed here, so {unplaced === 1 ? 'its' : 'their'} space is not counted above.</p>
      {/if}
    {/if}
  </section>

  {#if noGrant}
    <section class="empty" aria-labelledby="today-empty">
      <h2 id="today-empty">No granted folder yet</h2>
      <p>Loomward reads nothing until you grant a folder, so there is no scan age to show and nothing to rescan.</p>
      <p><a class="btn primary" href="#/health">Add a folder in Grants &amp; health</a></p>
    </section>
  {/if}

  <section class="panel" aria-labelledby="today-needs">
    <h2 id="today-needs">Needs you</h2>
    {#if needs.length === 0}
      <p>{needsEmptyText(missing.length, unevaluated)}</p>
    {:else}
      {#each needs as s (s.section)}
        <h3>{SECTION_TITLES[s.section]}</h3>
        <ul class="needs">
          {#each s.items.slice(0, SHOWN) as n (n.key)}
            <li>
              {#if n.name}<strong><VisibleName name={n.name} /></strong>{/if}
              {n.text}
              <a href={`#/${n.view}`}>{n.linkText}</a>
            </li>
          {/each}
        </ul>
        {#if s.items.length > SHOWN}
          <p class="muted small">and {s.items.length - SHOWN} more: see all in
            {#each moreViews(s.items, SHOWN) as view, n (view)}{n ? ', ' : ''}<a href={`#/${view}`}>{linkName(view)}</a>{/each}</p>
        {/if}
      {/each}
    {/if}
    {#if unevaluatedNote}<p><a href="#/alerts">{unevaluatedNote}</a></p>{/if}
    {#if jobsCut}<p class="muted small">Only the {JOBS_LIMIT} most recent jobs were read.</p>{/if}
  </section>
{/if}

<style>
  h3 { margin: var(--sp-4) 0 var(--sp-2); font-size: 1rem; }
  .needs { list-style: none; margin: 0; padding: 0; display: grid; gap: var(--sp-2); }
  .needs li { max-width: var(--measure); }
  .needs a { margin-left: var(--sp-2); white-space: nowrap; }
  .meter { height: 10px; min-width: 9rem; border: 1px solid var(--control-line); border-radius: 5px; background: var(--raised); overflow: hidden; margin-bottom: var(--sp-1); }
  .meter.blank { border-style: dotted; border-color: var(--unknown); background: transparent; }
  .used { height: 100%; background: var(--residency); opacity: 0.75; }
  .used.tight { background: repeating-linear-gradient(135deg, var(--danger) 0 4px, color-mix(in srgb, var(--danger) 45%, transparent) 4px 8px); opacity: 1; }
  tr.offline > * { color: var(--muted); }
  section.empty h2 { margin-top: 0; }
</style>
