<script module lang="ts">
  export const meta = { title: 'Alerts', order: 66 };
</script>
<script lang="ts">
  import { untrack } from 'svelte';
  import { session } from '../../lib/stores/session.svelte';
  import { formatTime } from '../../lib/format/time';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import type { AlertReport } from '../../lib/contracts.gen';
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

  const active = $derived(report?.items.filter((i) => i.status === 'alert') ?? []);
  const unevaluated = $derived(report?.items.filter((i) => i.status !== 'alert' && i.status !== 'ok').length ?? 0);

  async function load(): Promise<void> {
    if (!session.client) return;
    const mine = ++generation;
    loading = true; error = '';
    try {
      const { result } = await session.client.call('alerts.list', {});
      if (mine !== generation) return;
      report = result;
      form = formFrom(result.settings);
    } catch (e) {
      if (mine !== generation) return;
      error = session.handle(e); report = null; form = null;
    } finally { if (mine === generation) loading = false; }
  }

  async function save(): Promise<void> {
    if (!session.client || !form || saving) return;
    notice = ''; error = ''; invalidField = '';
    const parsed = settingsFrom(form);
    if ('error' in parsed) { error = parsed.error; invalidField = parsed.field; return; }
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
    session.epoch;
    untrack(() => void load());
    return () => { generation++; };
  });
</script>

<h1>Alerts</h1>
<p class="lede">Low free space on volumes that hold a granted folder, and folders that grew fast between complete scans.</p>
<p class="note">Alerts are advice, computed on this machine from what Loomward already observed. They never move, delete or change anything. <strong>Unknown is not zero:</strong> a volume whose space was not observed, a folder without enough scan history, or a partial scan is listed as such and raises no alert.</p>
<p class="live muted" class:has={!!notice} role="status" aria-label="Alert settings result">{notice}</p>
<p id="alerts-error" class="live bad" class:has={!!error} role="alert">{error}</p>

<section class="panel" aria-labelledby="alerts-active">
  <h2 id="alerts-active">Active alerts{report ? ` (${active.length})` : ''}</h2>
  {#if !report}
    <p class="empty">{loading ? 'Loading alerts' : error ? 'Alerts unavailable.' : 'No report yet.'}</p>
  {:else}
    {#each active as i, n (n)}
      <p class="alert"><span class="tag bad">Alert</span> <VisibleName name={i.label} /> · {measured(i)}, {threshold(i)}. <span class="muted">{i.note}</span></p>
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
              <th scope="row"><VisibleName name={i.label} /></th>
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
  <section class="panel"><p class="empty">No granted folder yet, so there is nothing to check.</p></section>
{/if}

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
      <button class="btn primary" type="submit" aria-disabled={saving} onclick={(e) => { if (saving) e.preventDefault(); }}>{saving ? 'Saving' : 'Save thresholds'}</button>
      <button class="btn" type="button" onclick={reset}>Fill in defaults</button>
    </form>
  </section>
{/if}

<style>
  .form { flex-wrap: wrap; align-items: end; }
  .check { display: inline-flex; gap: var(--sp-2); align-items: center; min-height: 24px; }
  .alert { margin: 0 0 var(--sp-2); }
</style>
