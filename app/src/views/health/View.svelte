<script module lang="ts">
  export const meta = { title: 'Grants & health', order: 70 };
</script>

<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import { formatBytes } from '../../lib/format/bytes';
  import { escapedName } from '../../lib/format/names';
  import { formatCount, formatDuration, formatTime } from '../../lib/format/time';
  import { grantPicker as picker } from '../../lib/stores/grant.svelte';
  import { scans } from '../../lib/stores/scan.svelte';
  import { forget } from '../../lib/stores/forget.svelte';
  import { session } from '../../lib/stores/session.svelte';
  import { Say } from '../../lib/ui/say.svelte';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import Diagnostics from './Diagnostics.svelte';
  import type { GrantList, Health, Root, RootList } from '../../lib/contracts.gen';
  import type { ScanView } from '../../lib/scan/model';
  import type { Grant } from '../../lib/derived';

  untrack(() => { picker.resume(); forget.resume(); });

  /** A revocation (or forgetting) the owner has asked for and not yet confirmed. */
  type Pending = { kind: 'root'; id: string; label: string } | { kind: 'grant'; id: string; label: string } | { kind: 'forget'; id: string; label: string };

  /** The last folder of a display path, for the confirmation's question. */
  const leaf = (path: string) => path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path;

  const OBSERVATION: Record<string, string> = {
    duplicate_content_read: 'Content read: duplicates only, on request',
    metadata_scan: 'Metadata scan', process_observation: 'Process observation', gpu_observation: 'GPU observation',
    disk_io_observation: 'Disk I/O observation', teacher_disclosure: 'Teacher disclosure',
  };

  let roots = $state<RootList | null>(null);
  let grants = $state<GrantList | null>(null);
  let health = $state<Health | null>(null);
  let loadError = $state('');
  const error = $derived(forget.problem || loadError);
  let revocation = $state<Pending | null>(null);
  const pending = $derived(forget.pending ?? revocation);
  let purge = $state(false);
  let revoking = $state(false);
  const working = $derived(revoking || forget.working);
  let revoked = $state('');
  const done = $derived(forget.done || revoked);
  let confirmEl = $state<HTMLElement>();
  let doneEl = $state<HTMLElement>();
  let opener: HTMLElement | null = null; // the Revoke… button that opened the confirmation: focus goes back to it
  let ticket = 0;
  let alive = true;
  let now = $state(Date.now()); // ticks once a second while a scan is active: the elapsed time
  const scanSay = new Say();

  async function load(): Promise<void> {
    const c = session.client;
    if (!c || !alive) return;
    const epoch = session.connectionEpoch;
    const mine = ++ticket;
    const [r, g, h, s] = await Promise.allSettled([c.call('roots.list', {}), c.call('grants.list', {}), c.call('health.get', {}), scans.sync()]);
    if (!alive || mine !== ticket || c !== session.client || epoch !== session.connectionEpoch) return;
    roots = r.status === 'fulfilled' ? r.value.result : null;
    grants = g.status === 'fulfilled' ? g.value.result : null;
    health = h.status === 'fulfilled' ? h.value.result : null;
    loadError = [r, g, h, s].filter(result => result.status === 'rejected').map(result => session.handle(result.reason)).join(' ');
  }

  onMount(() => () => { alive = false; ticket++; picker.dismiss(); forget.dismiss(); }); // requests in stores can finish offscreen
  $effect(() => { const r = roots?.roots; if (r) untrack(() => picker.sync(r)); }); // only a new roots read speaks, never a change of the picker's own state

  // `scans.settled` moves when a scan ends: the roots are read again for their totals and coverage.
  // A stream generation change can leave Health mounted. Resume before loading, so a forget on
  // the old connection exposes its unknown outcome immediately; ordinary root refreshes keep it.
  $effect(() => {
    void session.epoch;
    void session.info;
    void scans.settled;
    untrack(() => { forget.resume(); void load(); });
  });
  $effect(() => {
    if (!scans.anyActive) return;
    now = Date.now();
    const t = setInterval(() => { now = Date.now(); }, 1000);
    return () => clearInterval(t);
  });
  // Every scan state change is said once, in a region that stays in the page. A change made before this
  // view mounted (the store outlives the view) is not repeated.
  let heard = scans.change?.n ?? 0;
  $effect(() => {
    const c = scans.change;
    if (!c || c.n <= heard) return;
    heard = c.n;
    untrack(() => scanSay.say(`${escapedName(rootLabel(c.rootId))}: ${c.text}`));
  });

  const rootLabel = (id: string) => roots?.roots.find((r) => r.root_id === id)?.display_path.text ?? id;

  async function ask(p: Pending) {
    if (working || forget.unknown) return;
    opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (p.kind === 'forget') { revocation = null; forget.ask(p); }
    else { forget.cancel(); revocation = p; forget.done = ''; forget.problem = ''; forget.stopped = false; }
    purge = false;
    revoked = '';
    await tick();
    confirmEl?.focus();
  }

  /** Keep it: the confirmation closes and focus returns to the button that opened it. */
  async function cancel() {
    if (working || forget.unknown) return;
    forget.cancel();
    revocation = null;
    await tick();
    if (opener?.isConnected) opener.focus();
    opener = null;
  }

  async function stopWaiting() {
    forget.stopWaiting();
    await tick();
    doneEl?.focus();
  }

  async function confirm() {
    const c = session.client;
    const p = pending;
    if (!c || !p || working) return;
    if (p.kind === 'forget') {
      await forget.confirm();
      if (alive && !forget.pending) { await load(); await tick(); doneEl?.focus(); }
      return;
    }
    revoking = true;
    try {
      if (p.kind === 'root') {
        const { result } = await c.call('roots.revoke', { root_id: p.id, purge_catalog: purge });
        revoked = `Revoked ${escapedName(p.label)} at ${formatTime(result.revoked_at)}. ${result.purged ? "Loomward's own catalogue rows for it were deleted." : 'Its catalogue rows were kept.'} No scanned file was touched.`;
      } else {
        const { result } = await c.call('grants.revoke', { grant_id: p.id });
        revoked = `Revoked ${escapedName(p.label)} at ${formatTime(result.revoked_at)}.`;
      }
      revocation = null;
      await load();
      // the confirmation (and, for a root, its now disabled button) is gone: focus the result, which is announced
      await tick();
      doneEl?.focus();
    } catch (e) {
      loadError = session.handle(e);
    } finally {
      revoking = false;
    }
  }

  const rootActive = (r: Root) => r.grant_state === 'active';

  let addBtn = $state<HTMLButtonElement>();
  /** The native picker exists only where the service says so; elsewhere the page says where folders are added. */
  const canPick = $derived(session.info?.features.grant_picker === true);
  const pickNote = $derived(
    !session.info || canPick ? ''
    : session.info.adapter === 'http' ? 'Folders are added in the desktop app.'
    : session.info.dataset_class === 'synthetic' ? 'A synthetic session scans registered lab folders only; it cannot add a folder.'
    : 'This session has no folder picker.');

  /**
   * One native dialog at a time. The roots are read again afterwards. The button stays in the page (aria-disabled, never
   * disabled) so it keeps focus while the dialog is open; if focus was lost meanwhile it comes back, but it is never
   * pulled away from wherever the owner moved it.
   */
  async function addFolder() {
    if (picker.phase === 'waiting' || !roots) return;
    await picker.ask(roots.roots.map((r) => r.root_id));
    await load();
    await tick();
    const a = document.activeElement;
    if (!a || a === document.body || !a.isConnected) addBtn?.focus();
  }

  /** Scan or Cancel: one button that keeps its place, so keyboard focus is never lost while a scan starts, runs and ends. */
  async function primary(r: Root, v: ScanView | null) {
    if (v?.activity === 'cancelling') return;
    if (v?.active) await scans.cancel(r.root_id);
    else await scans.start(r.root_id, 'full');
    await keepFocus(r.root_id);
  }
  async function refresh(r: Root) {
    await scans.start(r.root_id, 'refresh');
    await keepFocus(r.root_id);
  }
  /** Refresh goes away once a scan starts: if focus was on it, it moves to the root's Scan/Cancel button. */
  async function keepFocus(id: string) {
    await tick();
    const a = document.activeElement;
    if (!a || a === document.body || !a.isConnected) document.querySelector<HTMLElement>(`[data-scan-primary="${CSS.escape(id)}"]`)?.focus();
  }
  const scanNote = $derived.by(() => {
    const info = session.info;
    if (!info || info.capabilities.observation.metadata_scan) return '';
    return 'This build reports that it cannot scan here. The service declines a scan and says why.';
  });
  const grantActive = (g: Grant) => g.revoked_at === null;
  const effects = $derived(Object.entries(session.info?.capabilities.effects ?? {}).filter(([name]) => name !== 'content_read'));
  const observation = $derived(Object.entries(session.info?.capabilities.observation ?? {}));
  const queueShare = $derived(health && health.catalog.writer_queue_capacity > 0 ? Math.min(100, (health.catalog.writer_queue_depth / health.catalog.writer_queue_capacity) * 100) : null);
</script>

<h1>Grants &amp; health</h1>
<p class="lede">What Loomward may read, and how it is doing. Grants come from the desktop folder picker or the server's command line. This page can add a folder where a picker exists, and can end a grant.</p>
<!-- live regions stay in the page and only their text changes: one inserted already filled is often not announced -->
<p class="bad live" class:has={!!error} role="alert">{error}</p>
<p class="live" class:ok={!forget.stopped} class:has={!!done} role="status" tabindex="-1" bind:this={doneEl}>{done}</p>
<p class="live" class:has={pending?.kind === 'forget' && (working || forget.unknown)} role="status">{pending?.kind === 'forget' ? forget.unknown ? 'Forget outcome pending. It may already have completed. Check status to retrieve the result.' : working ? 'Forgetting. Large roots can take a while; no scanned file is touched.' : '' : ''}</p>

{#if pending}
  <section class="panel confirm" role="group" aria-labelledby="h-confirm" bind:this={confirmEl} tabindex="-1">
    <h2 id="h-confirm">{#if pending.kind === 'forget'}Forget {escapedName(leaf(pending.label))}?{:else}Revoke {pending.kind === 'root' ? 'root access' : 'grant'}?{/if}</h2>
    <p><VisibleName name={pending.label} /></p>
    {#if pending.kind === 'forget'}
      <p class="muted">Loomward removes its name and catalogue from its database and its own backup copies, and discards the start-up backup copies (they hold every root's state); traces may remain in unused disk space until it is overwritten. Your files are not touched. A dated record that a root was granted, revoked and forgotten stays in the grants list, without the name.</p>
    {:else if pending.kind === 'root'}
      <p class="muted">Loomward stops reading this root. The files on disk are never touched.</p>
      <label class="chk"><input type="checkbox" bind:checked={purge} /> Also delete Loomward's own catalogue rows for this root (the scanned files stay as they are)</label>
    {:else}
      <p class="muted">The grant stops authorising anything further. Anything already sent under it cannot be recalled.</p>
    {/if}
    <div class="row">
      <button class="btn danger" type="button" disabled={working} onclick={() => void confirm()}>{pending.kind === 'forget' ? (working ? 'Forgetting' : forget.unknown ? 'Check status' : 'Forget') : working ? 'Revoking' : 'Revoke'}</button>
      <button class="btn" type="button" disabled={working || forget.unknown} onclick={() => void cancel()}>Keep it</button>
      {#if pending.kind === 'forget' && forget.unknown}
        <button class="btn" type="button" disabled={working} onclick={() => void stopWaiting()}>Stop waiting</button>
      {/if}
    </div>
  </section>
{/if}

<section class="panel" aria-labelledby="h-health">
  <div class="head">
    <h2 id="h-health">Loomward's own health</h2>
    <button class="btn" type="button" onclick={() => void load()}>Refresh</button>
  </div>
  {#if health}
    <div class="cols">
      <dl class="kv">
        <dt>Private commit</dt><dd class="num">{formatBytes(health.engine.private_commit_bytes)}</dd>
        <dt>Working set</dt><dd class="num">{formatBytes(health.engine.working_set_bytes)}</dd>
        <dt>CPU time</dt><dd class="num">{typeof health.engine.cpu_seconds === 'number' && Number.isFinite(health.engine.cpu_seconds) ? `${health.engine.cpu_seconds.toFixed(1)} s` : 'unknown'}</dd>
        <dt>Threads</dt><dd class="num">{formatCount(health.engine.threads)}</dd>
        <dt>Jobs running</dt><dd class="num">{formatCount(health.jobs_running)}</dd>
        <dt>Observed</dt><dd>{formatTime(health.observed_at)}</dd>
      </dl>
      <dl class="kv">
        <dt>Catalogue</dt><dd class="num">{formatCount(health.catalog.files)} files, {formatCount(health.catalog.dirs)} folders, schema v{health.catalog.schema_version}</dd>
        <dt>Database</dt><dd class="num">{formatBytes(health.catalog.db_bytes)}</dd>
        <dt>Write-ahead log</dt><dd class="num">{formatBytes(health.catalog.wal_bytes)}</dd>
        <dt>Writer queue</dt>
        <dd class="num">{health.catalog.writer_queue_depth} / {health.catalog.writer_queue_capacity}
          {#if queueShare !== null}<span class="meter" role="img" aria-label={`Writer queue ${queueShare.toFixed(0)}% full`}><i style={`width:${queueShare}%`}></i></span>{/if}</dd>
        <dt>Last error</dt><dd>{#if health.last_error}<span class="tag bad">{health.last_error.code.replaceAll('_', ' ')}</span> {health.last_error.message}{:else}none{/if}</dd>
      </dl>
    </div>
    {#if health.warnings.length}
      <ul class="list">
        {#each health.warnings as w (w.code + w.at)}
          <li><span class="tag warn">{w.code.replaceAll('_', ' ')}</span> {w.message} <span class="muted small">{formatTime(w.at)}</span></li>
        {/each}
      </ul>
    {:else}<p class="muted">No warnings.</p>{/if}
    {#if session.info}
      <p class="muted small">Engine {session.info.engine_version} · protocol {session.info.protocol} · {session.info.adapter} adapter · {session.info.dataset_class} dataset · enumeration {session.info.enumeration_strategy.replaceAll('_', ' ')} · session started {formatTime(session.info.session_started_at)}</p>
    {/if}
  {:else}<p class="muted">{error ? 'Health unknown: refresh failed.' : 'Loading'}</p>{/if}
</section>

<Diagnostics />

<section class="panel" aria-labelledby="h-roots">
  <h2 id="h-roots">Roots</h2>
  {#if canPick}
    <div class="add">
      <button class="btn primary" type="button" bind:this={addBtn} aria-disabled={picker.phase === 'waiting' || !roots ? 'true' : undefined} onclick={() => void addFolder()}>Add a folder…</button>
      <p class="live" class:has={!!picker.text} role="status">{picker.text}</p>
    </div>
  {:else if pickNote}<p class="note">{pickNote}</p>{/if}
  {#if roots}
    <div class="tbl-wrap">
      <table class="tbl">
        <thead>
          <tr>
            <th scope="col">Display path</th><th scope="col">Volume</th><th scope="col">Granted via</th><th scope="col">State</th><th scope="col">Scan</th>
            <th scope="col" class="r">Files</th><th scope="col" class="r">Logical</th><th scope="col" class="r">Allocated</th><th scope="col"><span class="sr-only">Action</span></th>
          </tr>
        </thead>
        <tbody>
          {#each roots.roots as r (r.root_id)}
            <tr>
              <th scope="row"><VisibleName name={r.display_path.text} />{#if r.display_path.truncated} <span class="tag warn">truncated</span>{/if}<div class="muted small">{r.origin.replaceAll('_', ' ')} · {r.dataset_class}</div></th>
              <td class="num">{r.volume_id ?? 'unknown'}</td>
              <td>{r.granted_via.replaceAll('_', ' ')}</td>
              <td><span class="tag" class:warn={!rootActive(r)}>{r.grant_state.replaceAll('_', ' ')}</span></td>
              <td>{r.scan.state.replaceAll('_', ' ')} <span class="tag">{r.scan.coverage}</span></td>
              <td class="r num">{formatCount(r.totals?.files)}</td>
              <td class="r num">{formatBytes(r.totals?.logical_bytes)}</td>
              <td class="r num">{formatBytes(r.totals?.allocated_bytes)}</td>
              <td><button class="btn" type="button" disabled={r.grant_state !== 'active' && r.grant_state !== 'identity_changed'} aria-label={`Revoke… ${escapedName(r.display_path.text)}`} onclick={() => void ask({ kind: 'root', id: r.root_id, label: r.display_path.text })}>Revoke…</button>
                {#if r.grant_state === 'revoked'}
                  <button class="btn small" type="button" aria-label={`Forget… ${escapedName(r.display_path.text)}`} onclick={() => void ask({ kind: 'forget', id: r.root_id, label: r.display_path.text })}>Forget…</button>
                {/if}</td>
            </tr>
          {:else}
            <tr><td colspan="9" class="muted">No roots granted.</td></tr>
          {/each}
        </tbody>
      </table>
    </div>

    <h3 id="h-scans">Scans</h3>
    <p class="muted">Scan reads names, sizes and dates only. It never opens the contents of a file and changes no file. Refresh re-reads a root that was scanned before.</p>
    <p class="muted">Watching runs only during a scan. While it runs, renaming a parent folder may be blocked; the watch closes when the scan ends. Completed coverage describes that scan; changes since it ended are unknown.</p>
    {#if scanNote}<p class="muted">{scanNote}</p>{/if}
    <p class="live scanlive" class:has={!!scanSay.text} role="status">{scanSay.text}</p>
    <ul class="scans" aria-labelledby="h-scans">
      {#each roots.roots.filter(rootActive) as r (r.root_id)}
        {@const v = scans.view(r.root_id, now)}
        {@const problem = scans.problems[r.root_id]}
        {@const label = escapedName(r.display_path.text)}
        <li class="scan">
          <div class="who"><VisibleName name={r.display_path.text} /> <span class="muted small">{r.origin.replaceAll('_', ' ')}</span>{#if r.root_id === picker.rootId} <span class="tag ok">just added</span>{/if}</div>
          <div class="acts">
            <button class="btn" type="button" data-scan-primary={r.root_id} aria-disabled={v?.activity === 'cancelling' ? 'true' : undefined}
              aria-label={v?.activity === 'cancelling' ? `Cancelling scan of ${label}` : v?.active ? `Cancel scan of ${label}` : `Scan ${label}`}
              onclick={() => void primary(r, v)}>{v?.activity === 'cancelling' ? 'Cancelling' : v?.active ? 'Cancel' : 'Scan'}</button>
            {#if !v?.active && r.scan.state !== 'never_scanned'}
              <button class="btn" type="button" aria-label={`Refresh scan of ${label}`} onclick={() => void refresh(r)}>Refresh</button>
            {/if}
          </div>
          {#if v}
            <div class="job" data-activity={v.activity}>
              {#if v.active}<span class="scanbar" aria-hidden="true"><i></i></span>{/if}
              <strong>{v.headline}</strong>{#if v.elapsedMs !== null}{' '}<span class="num">· {v.active ? 'elapsed' : v.activity === 'completed' || v.activity === 'partial' ? 'took' : 'after'} {formatDuration(v.elapsedMs)}</span>{/if}
              {#if v.activity === 'running'}<div class="muted small">No progress is reported until the scan ends.</div>{/if}
              {#if v.detail}<div class="small">{v.detail}</div>{/if}
            </div>
          {/if}
          {#if problem}<div class="problem small">{problem}</div>{/if}
        </li>
      {/each}
    </ul>
  {:else}<p class="muted">{error ? 'Roots unknown: refresh failed.' : 'Loading'}</p>{/if}
</section>

<section class="panel" aria-labelledby="h-grants">
  <h2 id="h-grants">Grants</h2>
  {#if grants}
    <ul class="list">
      {#each grants.grants as g (g.grant_id)}
        <li>
          {#if g.kind === 'metadata_root'}
            <strong>Metadata root</strong> {#if g.forgotten_at}<span class="muted">(name forgotten)</span>{:else}<VisibleName name={rootLabel(g.root_id)} />{/if} via {g.granted_via.replaceAll('_', ' ')}, granted {formatTime(g.granted_at)}
            {#if grantActive(g)}<span class="muted small"> · ends with the root's revocation above</span>{/if}
          {:else}
            <strong>Teacher disclosure</strong> to <code>{g.recipient}</code>: {g.item_count} items, fields {g.fields.join(', ')}; {g.used ? 'used' : 'unused'}, confirmed by {g.confirmed_via.replaceAll('_', ' ')}, expires {formatTime(g.expires_at)}
          {/if}
          <span class="tag" class:warn={!grantActive(g)}>{g.revoked_at ? `revoked ${formatTime(g.revoked_at)}` : 'active'}{g.kind === 'metadata_root' && g.forgotten_at ? ` · forgotten ${formatTime(g.forgotten_at)}` : ''}</span>
          {#if g.kind === 'teacher_disclosure' && grantActive(g)}
            <button class="btn small" type="button" aria-label={`Revoke… teacher disclosure to ${escapedName(g.recipient)}`} onclick={() => void ask({ kind: 'grant', id: g.grant_id, label: `Teacher disclosure to ${g.recipient}` })}>Revoke…</button>
          {/if}
        </li>
      {:else}
        <li class="muted">No grants.</li>
      {/each}
    </ul>
  {:else}<p class="muted">{error ? 'Grants unknown: refresh failed.' : 'Loading'}</p>{/if}
</section>

<section class="panel" aria-labelledby="h-caps">
  <h2 id="h-caps">What this build can do</h2>
  <p class="muted">Every file and process effect below is off. Duplicate inspection reads file contents locally, read-only, within its budgets and only for a folder you granted when you press Start inspection. The one exception is a diagnostic bundle you save yourself (above), written only to a file you pick and confirm.</p>
  <h3>Effects</h3>
  <ul class="caps">
    {#each effects as [name, on] (name)}
      <li><span class="tag" class:bad={on !== false}>{on === false ? 'off' : 'ON'}</span> {name.replaceAll('_', ' ')}</li>
    {/each}
  </ul>
  <h3>Observation</h3>
  <ul class="caps">
    {#each observation as [name, on] (name)}
      <li><span class="tag" class:ok-tag={on}>{on ? 'available' : 'not available'}</span> {OBSERVATION[name] ?? name.replaceAll('_', ' ')}</li>
    {/each}
  </ul>
</section>

<style>
  .bad { color: var(--danger); }
  .ok { color: var(--ok); }
  .live:focus-visible { outline-offset: 4px; }
  .small { font-size: 0.8rem; }
  .add { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 16px; margin-bottom: 12px; }
  .add .live { margin: 0; flex: 1 1 18rem; }
  .head { display: flex; justify-content: space-between; align-items: center; gap: 12px; }
  .cols { display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 8px 32px; }
  .list { margin: 0 0 8px; padding-left: 1.1em; }
  .list li { margin-bottom: 8px; overflow-wrap: anywhere; }
  .kv { display: grid; grid-template-columns: max-content 1fr; gap: 4px 20px; margin: 0 0 12px; align-content: start; }
  .kv dt { color: var(--muted); }
  .kv dd { margin: 0; }
  .caps { list-style: none; margin: 0 0 12px; padding: 0; display: grid; grid-template-columns: repeat(auto-fill, minmax(210px, 1fr)); gap: 6px; }
  .tag.ok-tag { color: var(--ok); border-color: var(--ok); }
  .meter { display: inline-block; width: 90px; height: 8px; margin-left: 10px; border: 1px solid var(--control-line); border-radius: 4px; vertical-align: middle; overflow: hidden; }
  .meter i { display: block; height: 100%; background: var(--residency); }
  .scans { list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 22rem), 1fr)); gap: 10px; }
  .scan { border: 1px solid var(--line); border-radius: var(--radius); padding: 10px 12px; display: grid; gap: 8px; align-content: start; }
  .who { overflow-wrap: anywhere; }
  .acts { display: flex; flex-wrap: wrap; gap: 6px; }
  .job { min-height: 1.5em; }
  .job[data-activity='partial'] strong, .job[data-activity='cancelled'] strong { color: var(--warn); }
  .job[data-activity='failed'] strong, .problem { color: var(--danger); }
  .problem { margin-top: 4px; }
  /* indeterminate: the service reports no progress while a job runs, so nothing here moves in proportion to anything */
  .scanbar { display: inline-block; position: relative; width: 56px; height: 6px; margin-right: 8px; border: 1px solid var(--control-line); border-radius: 3px; vertical-align: middle; overflow: hidden; }
  .scanbar i { position: absolute; top: 0; bottom: 0; left: 0; width: 40%; background: var(--residency); animation: sweep 1.3s ease-in-out infinite alternate; }
  @keyframes sweep { to { left: 60%; } }
  @media (forced-colors: active) { .scanbar { border-color: CanvasText; } .scanbar i { background: Highlight; } }
  .confirm { border-color: var(--permission); border-style: dashed; margin-bottom: 16px; outline: none; }
  .confirm:focus-visible { outline: 2px solid var(--focus); }
  .chk { display: block; margin: 8px 0; }
  .row { display: flex; gap: 8px; margin-top: 12px; }
  .btn.danger { border-color: var(--danger); color: var(--danger); }
  .btn.small { padding: 1px 8px; font-size: 0.85rem; margin-left: 6px; }
  @media (max-width: 600px) { .kv { grid-template-columns: 1fr; gap: 0; } .kv dd { margin-bottom: 8px; } }
</style>
