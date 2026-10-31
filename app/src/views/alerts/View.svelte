<script module lang="ts">
  export const meta = { title: 'Alerts', order: 66 };
</script>
<script lang="ts">
  import { untrack } from 'svelte';
  import { session } from '../../lib/stores/session.svelte';
  import { formatTime } from '../../lib/format/time';
  import { formatBytes } from '../../lib/format/bytes';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import VolumeStatus from '../../lib/ui/VolumeStatus.svelte';
  import { Say } from '../../lib/ui/say.svelte';
  import type { AlertHistory, AlertReport } from '../../lib/contracts.gen';
  import { COVERAGE_WORDS, formFrom, measured, settingsFrom, STATUS, threshold, type SettingsForm } from './model';

  let report = $state<AlertReport | null>(null);
  let form = $state<SettingsForm | null>(null);
  let loading = $state(false);
  let saving = $state(false);
  let error = $state('');
  let notice = $state('');
  // The field the shown form error belongs to: marked invalid and described by the message (WCAG 3.3.1).
  let invalidField = $state<'' | keyof SettingsForm>('');
  let generation = 0;
  let request: AbortController | null = null;
  const refreshResult = new Say();
  let historyRoot = $state<string | null>(null);
  let history = $state<AlertHistory | null>(null);
  let historyLoading = $state(false);
  let historyError = $state('');
  let historyGeneration = 0;
  let historyRequest: AbortController | null = null;

  function closeHistory(restoreFocus = false): void {
    const root = historyRoot;
    historyGeneration++; historyRequest?.abort(); historyRequest = null;
    historyRoot = null; history = null; historyLoading = false; historyError = '';
    if (restoreFocus && root) document.getElementById(`alert-history-${root}`)?.focus();
  }

  async function showHistory(root: string | null): Promise<void> {
    const client = session.client;
    if (!root || !client || session.state !== 'connected' || loading || saving ||
      (historyLoading && historyRoot === root)) return;
    closeHistory();
    historyRoot = root; historyLoading = true;
    const ctl = historyRequest = new AbortController();
    const mine = historyGeneration, epoch = session.epoch, connection = session.connectionEpoch;
    const current = () => mine === historyGeneration && !ctl.signal.aborted && client === session.client &&
      epoch === session.epoch && connection === session.connectionEpoch && session.state === 'connected';
    try {
      const { result } = await client.call('alerts.history', { root_id: root }, { signal: ctl.signal });
      if (current()) history = result;
    } catch (e) { if (current()) historyError = session.handle(e); }
    finally { if (current()) { historyLoading = false; historyRequest = null; } }
  }

  const active = $derived(report?.items.filter((i) => i.status === 'alert') ?? []);
  const unevaluated = $derived(report?.items.filter((i) => i.status !== 'alert' && i.status !== 'ok').length ?? 0);

  async function load(manual = false): Promise<void> {
    const client = session.client;
    if (!client || session.state !== 'connected' || (manual && (loading || saving))) return;
    closeHistory();
    request?.abort();
    const ctl = request = new AbortController();
    const mine = ++generation, epoch = session.epoch, connection = session.connectionEpoch;
    const current = () => mine === generation && !ctl.signal.aborted && client === session.client &&
      epoch === session.epoch && connection === session.connectionEpoch && session.state === 'connected';
    loading = true; error = '';
    refreshResult.clear();
    if (manual) { notice = ''; invalidField = ''; }
    try {
      const { result } = await client.call('alerts.list', {}, { signal: ctl.signal });
      if (!current()) return;
      report = result;
      // Refresh observations without replacing thresholds the owner is editing.
      form ??= formFrom(result.settings);
      if (manual) refreshResult.say('Checks refreshed from recorded observations. Unsaved threshold edits were kept.');
    } catch (e) {
      if (!current()) return;
      error = session.handle(e); report = null;
    } finally { if (current()) { loading = false; request = null; } }
  }

  function refresh(): Promise<void> {
    return load(true);
  }

  async function save(): Promise<void> {
    if (!session.client || !form || saving || loading) return;
    notice = ''; error = ''; invalidField = '';
    const parsed = settingsFrom(form);
    if ('error' in parsed) { error = parsed.error; invalidField = parsed.field; return; }
    closeHistory();
    // Shares load()'s generation: a reply superseded by a newer load or save is dropped, so it cannot show older thresholds.
    const mine = ++generation;
    saving = true;
    try {
      const { result } = await session.client.call('alerts.configure', parsed.settings);
      if (mine !== generation) return;
      report = result;
      form = formFrom(result.settings);
      notice = 'Thresholds saved. This changes what is flagged here, nothing else.';
    } catch (e) { if (mine === generation) error = session.handle(e); }
    finally { saving = false; }
  }

  function reset(): void {
    if (report) { form = formFrom(report.defaults); invalidField = ''; error = ''; notice = 'Defaults filled in. Save to keep them.'; }
  }

  $effect(() => {
    session.epoch; session.connectionEpoch; session.state; session.client;
    untrack(() => void load());
    return () => {
      generation++; request?.abort(); request = null; loading = false; refreshResult.clear();
      closeHistory();
    };
  });
</script>

<h1>Alerts</h1>
<p class="lede">Low free space on volumes that hold a granted folder, and folders that grew fast between complete scans.</p>
<p class="note">Alerts are advice, computed on this machine from what Loomward already observed. They never move, delete or change anything. <strong>Unknown is not zero:</strong> a volume whose space was not observed, a folder without enough scan history, or a partial scan is listed as such and raises no alert.</p>
<div class="controls">
  <button class="btn" type="button" aria-disabled={loading || saving} aria-describedby="alerts-refresh-hint" onclick={() => void refresh()}>Refresh checks</button>
  <span id="alerts-refresh-hint" class="muted small">Reevaluate recorded observations without starting a scan. Unsaved threshold edits are kept.</span>
</div>
<p class="live muted" class:has={loading || !!refreshResult.text} role="status" aria-label="Alert refresh result">{loading ? 'Refreshing checks…' : refreshResult.text}</p>
<p class="live muted" class:has={!!notice} role="status" aria-label="Alert settings result">{notice}</p>
<p id="alerts-error" class="live bad" class:has={!!error} role="alert">{error}</p>

<section class="panel" aria-labelledby="alerts-active">
  <h2 id="alerts-active">Active alerts{report ? ` (${active.length})` : ''}</h2>
  {#if !report}
    <p class="empty">{loading ? 'Loading alerts' : error ? 'Alerts unavailable.' : 'No report yet.'}</p>
  {:else}
    {#each active as i, n (n)}
      <p class="alert"><span class="tag bad">Alert</span> <VisibleName name={i.label} /> <VolumeStatus id={i.volume_id} /> · {measured(i)}, {threshold(i)}. <span class="muted">{i.note}</span></p>
    {:else}
      <p>No alert is active.{unevaluated ? ` ${unevaluated} ${unevaluated === 1 ? 'check was' : 'checks were'} not evaluated; see below.` : ''}</p>
    {/each}
    <p class="muted small">Evaluated {formatTime(report.evaluated_at)}. {report.notifications_note}</p>
  {/if}
</section>

{#if report && report.items.length}
  <section class="panel" aria-labelledby="alerts-checks">
    <h2 id="alerts-checks">Every check</h2>
    <div class="tbl-wrap">
      <table class="tbl">
        <caption class="sr-only">Low-space and growth checks with their coverage</caption>
        <thead><tr><th scope="col">Subject</th><th scope="col">Check</th><th scope="col">Status</th><th scope="col">Measured</th><th scope="col">Threshold</th><th scope="col">Coverage</th></tr></thead>
        <tbody>
          {#each report.items as i, n (n)}
            {@const s = STATUS[i.status]}
            <tr>
              <th scope="row"><VisibleName name={i.label} /> <VolumeStatus id={i.volume_id} />
                {#if i.kind === 'fast_growth' && i.root_id}
                  <button id={`alert-history-${i.root_id}`} class="btn history-open" type="button" aria-expanded={historyRoot === i.root_id} aria-controls="alert-history" aria-disabled={loading || saving || (historyLoading && historyRoot === i.root_id)} onclick={() => void showHistory(i.root_id)}>View scan history</button>
                {/if}
              </th>
              <td>{i.kind === 'low_space' ? 'Free space' : 'Growth'}</td>
              <td><span class="tag" class:bad={s.tone === 'bad'} class:ok={s.tone === 'ok'} class:warn={s.tone === 'warn'} class:unk={s.tone === 'unk'}>{s.label}</span><small>{i.note}</small></td>
              <td class:unknown={i.measured_bytes === null}>{measured(i)}{#if i.window_start && i.window_end}<small>{formatTime(i.window_start)} to {formatTime(i.window_end)}</small>{/if}</td>
              <td>{threshold(i)}</td>
              <td>{COVERAGE_WORDS[i.coverage] ?? i.coverage}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  </section>
{:else if report}
  <section class="panel"><p class="empty">No granted folder yet, so there is nothing to check. <a href="#/health">Add a folder in Grants &amp; health</a>.</p></section>
{/if}

<section id="alert-history" class="panel" aria-labelledby="alert-history-title" hidden={!historyRoot}>
  <div class="controls">
    <h2 id="alert-history-title">Recorded scan history</h2>
    <button class="btn" type="button" onclick={() => closeHistory(true)}>Close history</button>
  </div>
  {#if historyRoot}
    <p><VisibleName name={report?.items.find(i => i.root_id === historyRoot)?.label ?? 'Selected folder'} /></p>
  {/if}
  <p class="live muted" class:has={historyLoading || !!history} role="status" aria-label="Scan history result">{historyLoading ? 'Loading recorded scans…' : history ? `${history.samples.length} recorded scans shown, oldest first.` : ''}</p>
  <p class="live bad" class:has={!!historyError} role="alert">{historyError}</p>
  {#if historyError}<button class="btn" type="button" onclick={() => void showHistory(historyRoot)}>Retry history</button>{/if}
  {#if history}
    <p>Saved check: <strong>{STATUS[history.check.status].label}</strong>. {history.check.note}</p>
    <p class="muted small">Evaluated {formatTime(history.evaluated_at)} using the saved {history.growth_window_days}-day window and growth threshold of {formatBytes(history.check.threshold_bytes)}. Unsaved threshold edits do not apply here.</p>
    <p class="note">Recorded logical sizes, not current disk contents or a forecast. Only complete scans inside the saved window are comparable; unknown is not zero. Retained history may not cover the entire window.</p>
    {#if history.truncated}<p class="note">Older records omitted: showing the newest 64 completed scans.</p>{/if}
    {#if history.samples.length}
      <div class="tbl-wrap"><table class="tbl">
        <caption class="sr-only">Recorded folder sizes and scan coverage, oldest first</caption>
        <thead><tr><th scope="col">Scan finished</th><th scope="col">Logical size</th><th scope="col">Coverage</th></tr></thead>
        <tbody>{#each history.samples as sample, n (n)}<tr>
          <th scope="row">{formatTime(sample.finished_at)}</th>
          <td class:unknown={sample.logical_bytes === null}>{sample.logical_bytes === null ? 'Unknown' : formatBytes(sample.logical_bytes)}{#if sample.logical_bytes !== null}<small>{sample.logical_bytes} bytes</small>{/if}</td>
          <td>{COVERAGE_WORDS[sample.coverage] ?? sample.coverage}</td>
        </tr>{/each}</tbody>
      </table></div>
    {:else}<p class="empty">No completed scans were recorded for this folder.</p>{/if}
  {/if}
</section>

{#if form && report}
  <section class="panel" aria-labelledby="alerts-settings">
    <h2 id="alerts-settings">Thresholds</h2>
    <p class="muted">Free space is low when it falls below the larger of the percentage and the size. A folder is growing fast when its net size between the oldest and newest complete scans inside the window reaches the growth size.</p>
    <form class="controls form" onsubmit={(e) => { e.preventDefault(); void save(); }}>
      <label class="lw-field"><span class="lbl">Low space: percent of capacity</span>
        <input type="text" inputmode="numeric" bind:value={form.percent} maxlength="3" autocomplete="off" aria-invalid={invalidField === 'percent' ? 'true' : undefined} aria-describedby={invalidField === 'percent' ? 'alerts-error' : undefined} /></label>
      <label class="lw-field"><span class="lbl">Low space: at least this many GiB free</span>
        <input type="text" inputmode="decimal" bind:value={form.freeGib} maxlength="12" autocomplete="off" aria-invalid={invalidField === 'freeGib' ? 'true' : undefined} aria-describedby={invalidField === 'freeGib' ? 'alerts-error' : undefined} /></label>
      <label class="lw-field"><span class="lbl">Growth: window in days</span>
        <input type="text" inputmode="numeric" bind:value={form.days} maxlength="3" autocomplete="off" aria-invalid={invalidField === 'days' ? 'true' : undefined} aria-describedby={invalidField === 'days' ? 'alerts-error' : undefined} /></label>
      <label class="lw-field"><span class="lbl">Growth: GiB added within the window</span>
        <input type="text" inputmode="decimal" bind:value={form.growthGib} maxlength="12" autocomplete="off" aria-invalid={invalidField === 'growthGib' ? 'true' : undefined} aria-describedby={invalidField === 'growthGib' ? 'alerts-error' : undefined} /></label>
      <label class="check"><input type="checkbox" bind:checked={form.notifications} /> Notify me when notifications arrive (stored now; no notification is sent yet)</label>
      <button class="btn primary" type="submit" aria-disabled={saving || loading} onclick={(e) => { if (saving || loading) e.preventDefault(); }}>{saving ? 'Saving' : 'Save thresholds'}</button>
      <button class="btn" type="button" onclick={reset}>Fill in defaults</button>
    </form>
  </section>
{/if}

<style>
  .form { flex-wrap: wrap; align-items: end; }
  .check { display: inline-flex; gap: var(--sp-2); align-items: center; min-height: 24px; }
  .alert { margin: 0 0 var(--sp-2); }
  .history-open { display: block; margin-top: var(--sp-2); }
</style>
