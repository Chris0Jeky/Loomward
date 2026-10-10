<script module lang="ts">
  export const meta = { title: 'Grants & health', order: 70 };
</script>

<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { formatBytes } from '../../lib/format/bytes';
  import { formatCount, formatTime } from '../../lib/format/time';
  import { session } from '../../lib/stores/session.svelte';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import type { Grant, GrantList, Health, Root, RootList } from '../../lib/types';

  /** A revocation the owner has asked for and not yet confirmed. */
  type Pending = { kind: 'root'; id: string; label: string } | { kind: 'grant'; id: string; label: string };

  const OBSERVATION: Record<string, string> = {
    metadata_scan: 'Metadata scan', process_observation: 'Process observation', gpu_observation: 'GPU observation',
    disk_io_observation: 'Disk I/O observation', teacher_disclosure: 'Teacher disclosure',
  };

  let roots = $state<RootList | null>(null);
  let grants = $state<GrantList | null>(null);
  let health = $state<Health | null>(null);
  let error = $state('');
  let pending = $state<Pending | null>(null);
  let purge = $state(false);
  let working = $state(false);
  let done = $state('');
  let confirmEl = $state<HTMLElement>();
  let ticket = 0;

  async function load(): Promise<void> {
    const c = session.client;
    if (!c) return;
    const mine = ++ticket;
    try {
      const [r, g, h] = await Promise.all([c.call('roots.list', {}), c.call('grants.list', {}), c.call('health.get', {})]);
      if (mine !== ticket) return;
      roots = r.result;
      grants = g.result;
      health = h.result;
      error = '';
    } catch (e) {
      if (mine === ticket) error = session.handle(e);
    }
  }

  $effect(() => {
    void session.epoch;
    untrack(() => void load());
  });

  const rootLabel = (id: string) => roots?.roots.find((r) => r.root_id === id)?.display_path.text ?? id;

  async function ask(p: Pending) {
    pending = p;
    purge = false;
    done = '';
    await tick();
    confirmEl?.focus();
  }

  async function confirm() {
    const c = session.client;
    const p = pending;
    if (!c || !p) return;
    working = true;
    try {
      if (p.kind === 'root') {
        const { result } = await c.call('roots.revoke', { root_id: p.id, purge_catalog: purge });
        done = `Revoked ${p.label} at ${formatTime(result.revoked_at)}. ${result.purged ? "Loomward's own catalogue rows for it were deleted." : 'Its catalogue rows were kept.'} No scanned file was touched.`;
      } else {
        const { result } = await c.call('grants.revoke', { grant_id: p.id });
        done = `Revoked ${p.label} at ${formatTime(result.revoked_at)}.`;
      }
      pending = null;
      await load();
    } catch (e) {
      error = session.handle(e);
    } finally {
      working = false;
    }
  }

  const rootActive = (r: Root) => r.grant_state === 'active';
  const grantActive = (g: Grant) => g.revoked_at === null;
  const effects = $derived(Object.entries(session.info?.capabilities.effects ?? {}));
  const observation = $derived(Object.entries(session.info?.capabilities.observation ?? {}));
  const queueShare = $derived(health && health.catalog.writer_queue_capacity > 0 ? Math.min(100, (health.catalog.writer_queue_depth / health.catalog.writer_queue_capacity) * 100) : null);
</script>

<h1>Grants &amp; health</h1>
<p class="muted">What Loomward may read, and how it is doing. Grants come from the desktop folder picker or the server's command line; this page can only ask to end one.</p>
{#if error}<p class="bad" role="alert">{error}</p>{/if}
{#if done}<p class="ok" role="status">{done}</p>{/if}

{#if pending}
  <section class="panel confirm" role="group" aria-labelledby="h-confirm" bind:this={confirmEl} tabindex="-1">
    <h2 id="h-confirm">Revoke {pending.kind === 'root' ? 'root access' : 'grant'}?</h2>
    <p><VisibleName name={pending.label} /></p>
    {#if pending.kind === 'root'}
      <p class="muted">Loomward stops reading this root. The files on disk are never touched.</p>
      <label class="chk"><input type="checkbox" bind:checked={purge} /> Also delete Loomward's own catalogue rows for this root (the scanned files stay as they are)</label>
    {:else}
      <p class="muted">The grant stops authorising anything further. Anything already sent under it cannot be recalled.</p>
    {/if}
    <div class="row">
      <button class="btn danger" type="button" disabled={working} onclick={() => void confirm()}>{working ? 'Revoking' : 'Revoke'}</button>
      <button class="btn" type="button" disabled={working} onclick={() => (pending = null)}>Keep it</button>
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
        <dt>CPU time</dt><dd class="num">{health.engine.cpu_seconds === null ? 'unknown' : `${health.engine.cpu_seconds.toFixed(1)} s`}</dd>
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
  {:else}<p class="muted">Loading</p>{/if}
</section>

<section class="panel" aria-labelledby="h-roots">
  <h2 id="h-roots">Roots</h2>
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
              <td><VisibleName name={r.display_path.text} />{#if r.display_path.truncated} <span class="tag warn">truncated</span>{/if}<div class="muted small">{r.origin.replaceAll('_', ' ')} · {r.dataset_class}</div></td>
              <td class="num">{r.volume_id ?? 'unknown'}</td>
              <td>{r.granted_via.replaceAll('_', ' ')}</td>
              <td><span class="tag" class:warn={!rootActive(r)}>{r.grant_state.replaceAll('_', ' ')}</span></td>
              <td>{r.scan.state.replaceAll('_', ' ')} <span class="tag">{r.scan.coverage}</span></td>
              <td class="r num">{formatCount(r.totals?.files)}</td>
              <td class="r num">{formatBytes(r.totals?.logical_bytes)}</td>
              <td class="r num">{formatBytes(r.totals?.allocated_bytes)}</td>
              <td><button class="btn" type="button" disabled={!rootActive(r)} onclick={() => void ask({ kind: 'root', id: r.root_id, label: r.display_path.text })}>Revoke…</button></td>
            </tr>
          {:else}
            <tr><td colspan="9" class="muted">No roots granted.</td></tr>
          {/each}
        </tbody>
      </table>
    </div>
  {:else}<p class="muted">Loading</p>{/if}
</section>

<section class="panel" aria-labelledby="h-grants">
  <h2 id="h-grants">Grants</h2>
  {#if grants}
    <ul class="list">
      {#each grants.grants as g (g.grant_id)}
        <li>
          {#if g.kind === 'metadata_root'}
            <strong>Metadata root</strong> <VisibleName name={rootLabel(g.root_id)} /> via {g.granted_via.replaceAll('_', ' ')}, granted {formatTime(g.granted_at)}
            {#if grantActive(g)}<span class="muted small"> · ends with the root's revocation above</span>{/if}
          {:else}
            <strong>Teacher disclosure</strong> to <code>{g.recipient}</code>: {g.item_count} items, fields {g.fields.join(', ')}; {g.used ? 'used' : 'unused'}, confirmed by {g.confirmed_via.replaceAll('_', ' ')}, expires {formatTime(g.expires_at)}
          {/if}
          <span class="tag" class:warn={!grantActive(g)}>{g.revoked_at ? `revoked ${formatTime(g.revoked_at)}` : 'active'}</span>
          {#if g.kind === 'teacher_disclosure' && grantActive(g)}
            <button class="btn small" type="button" onclick={() => void ask({ kind: 'grant', id: g.grant_id, label: `Teacher disclosure to ${g.recipient}` })}>Revoke…</button>
          {/if}
        </li>
      {:else}
        <li class="muted">No grants.</li>
      {/each}
    </ul>
  {:else}<p class="muted">Loading</p>{/if}
</section>

<section class="panel" aria-labelledby="h-caps">
  <h2 id="h-caps">What this build can do</h2>
  <p class="muted">Every effect is off, fixed by the contract: Loomward v0.3 observes and simulates only.</p>
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
  .small { font-size: 0.8rem; }
  .head { display: flex; justify-content: space-between; align-items: center; gap: 12px; }
  .cols { display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 8px 32px; }
  .list { margin: 0 0 8px; padding-left: 1.1em; }
  .list li { margin-bottom: 8px; overflow-wrap: anywhere; }
  .kv { display: grid; grid-template-columns: max-content 1fr; gap: 4px 20px; margin: 0 0 12px; align-content: start; }
  .kv dt { color: var(--muted); }
  .kv dd { margin: 0; }
  .caps { list-style: none; margin: 0 0 12px; padding: 0; display: grid; grid-template-columns: repeat(auto-fill, minmax(210px, 1fr)); gap: 6px; }
  .tag.ok-tag { color: var(--ok); border-color: var(--ok); }
  .meter { display: inline-block; width: 90px; height: 8px; margin-left: 10px; border: 1px solid var(--line); border-radius: 4px; vertical-align: middle; overflow: hidden; }
  .meter i { display: block; height: 100%; background: var(--residency); }
  .confirm { border-color: var(--permission); border-style: dashed; margin-bottom: 16px; outline: none; }
  .confirm:focus-visible { outline: 2px solid var(--focus); }
  .chk { display: block; margin: 8px 0; }
  .row { display: flex; gap: 8px; margin-top: 12px; }
  .btn.danger { border-color: var(--danger); color: var(--danger); }
  .btn.small { padding: 1px 8px; font-size: 0.85rem; margin-left: 6px; }
  @media (max-width: 600px) { .kv { grid-template-columns: 1fr; gap: 0; } .kv dd { margin-bottom: 8px; } }
</style>
