<script module lang="ts">
  export const meta = { title: 'Grants & health', order: 70 };
</script>

<script lang="ts">
  // PLACEHOLDER (lane L3). Lane L13 takes this over: revocation and the full self-health card.
  import { formatBytes } from '../../lib/format/bytes';
  import { formatCount, formatTime } from '../../lib/format/time';
  import { session } from '../../lib/stores/session.svelte';
  import type { GrantList, Health, RootList } from '../../lib/types';

  let roots = $state<RootList | null>(null);
  let grants = $state<GrantList | null>(null);
  let health = $state<Health | null>(null);
  let error = $state('');
  let ticket = 0;

  $effect(() => {
    void session.epoch;
    const c = session.client;
    if (!c) return;
    const mine = ++ticket;
    void (async () => {
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
    })();
  });

  const effects = $derived(Object.entries(session.info?.capabilities.effects ?? {}));
</script>

<h1>Grants &amp; health</h1>
<p class="muted">Placeholder cards. Revoking a grant arrives with lane L13; nothing here changes any state.</p>
{#if error}<p class="bad" role="alert">{error}</p>{/if}

<section class="panel" aria-labelledby="h-roots">
  <h2 id="h-roots">Roots</h2>
  {#if roots}
    <div class="tbl-wrap">
      <table class="tbl">
        <thead><tr><th scope="col">Display path</th><th scope="col">Origin</th><th scope="col">Granted via</th><th scope="col">State</th><th scope="col">Scan</th><th scope="col" class="r">Files</th><th scope="col" class="r">Logical</th></tr></thead>
        <tbody>
          {#each roots.roots as r (r.root_id)}
            <tr>
              <td><bdi>{r.display_path.text}</bdi>{#if r.display_path.truncated} <span class="tag warn">truncated</span>{/if}</td>
              <td>{r.origin.replaceAll('_', ' ')}</td>
              <td>{r.granted_via.replaceAll('_', ' ')}</td>
              <td>{r.grant_state.replaceAll('_', ' ')}</td>
              <td>{r.scan.state.replaceAll('_', ' ')} <span class="tag">{r.scan.coverage}</span></td>
              <td class="r num">{formatCount(r.totals?.files)}</td>
              <td class="r num">{formatBytes(r.totals?.logical_bytes)}</td>
            </tr>
          {:else}
            <tr><td colspan="7" class="muted">No roots granted.</td></tr>
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
            <strong>Metadata root</strong> <code>{g.root_id}</code> via {g.granted_via.replaceAll('_', ' ')}
          {:else}
            <strong>Teacher disclosure</strong> to <code>{g.recipient}</code>, {g.item_count} items, fields {g.fields.join(', ')}; {g.used ? 'used' : 'unused'}, confirmed by {g.confirmed_via.replaceAll('_', ' ')}
          {/if}
          <span class="tag">{g.revoked_at ? `revoked ${formatTime(g.revoked_at)}` : 'active'}</span>
        </li>
      {:else}
        <li class="muted">No grants.</li>
      {/each}
    </ul>
  {:else}<p class="muted">Loading</p>{/if}
</section>

<section class="panel" aria-labelledby="h-health">
  <h2 id="h-health">Loomward's own health</h2>
  {#if health}
    <dl class="kv">
      <dt>Private memory</dt><dd class="num">{formatBytes(health.engine.private_bytes)}</dd>
      <dt>Working set</dt><dd class="num">{formatBytes(health.engine.working_set_bytes)}</dd>
      <dt>Threads</dt><dd class="num">{formatCount(health.engine.threads)}</dd>
      <dt>Catalogue</dt><dd class="num">{formatCount(health.catalog.files)} files, {formatCount(health.catalog.dirs)} folders, schema v{health.catalog.schema_version}</dd>
      <dt>Writer queue</dt><dd class="num">{health.catalog.writer_queue_depth} / {health.catalog.writer_queue_capacity}</dd>
      <dt>Jobs running</dt><dd class="num">{health.jobs_running}</dd>
      <dt>Observed</dt><dd>{formatTime(health.observed_at)}</dd>
    </dl>
    {#each health.warnings as w (w.code + w.at)}
      <p class="tag warn">{w.code.replaceAll('_', ' ')}: {w.message}</p>
    {/each}
  {:else}<p class="muted">Loading</p>{/if}
</section>

<section class="panel" aria-labelledby="h-caps">
  <h2 id="h-caps">What this build can change</h2>
  <p class="muted">Every effect capability is fixed off by the contract. Loomward v0.3 observes and simulates only.</p>
  <ul class="caps">
    {#each effects as [name, on] (name)}
      <li><span class="tag" class:bad={on !== false}>{on === false ? 'off' : 'ON'}</span> {name.replaceAll('_', ' ')}</li>
    {/each}
  </ul>
</section>

<style>
  .bad { color: var(--danger); }
  .list { margin: 0; padding-left: 1.1em; }
  .list li { margin-bottom: 6px; overflow-wrap: anywhere; }
  .kv { display: grid; grid-template-columns: max-content 1fr; gap: 4px 20px; margin: 0 0 12px; }
  .kv dt { color: var(--muted); }
  .kv dd { margin: 0; }
  .caps { list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); gap: 6px; }
</style>
