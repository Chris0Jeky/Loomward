<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import type { ApplicationPage, DesktopPage, InventoryText } from '../contracts.gen';
  import { formatBytes } from '../format/bytes';
  import { formatTime } from '../format/time';
  import { session } from '../stores/session.svelte';
  import { Say } from './say.svelte';
  import VisibleName from './VisibleName.svelte';

  let { kind }: { kind: 'applications' | 'desktop' } = $props();
  let apps = $state<ApplicationPage | null>(null);
  let desktop = $state<DesktopPage | null>(null);
  let busy = $state(false);
  let error = $state('');
  let pageNumber = $state(1);
  let next = $state<string | null>(null);
  let ticket = 0;
  let controller: AbortController | undefined;
  const say = new Say();
  const data = $derived(apps ?? desktop);
  const title = $derived(kind === 'applications' ? 'Applications' : 'Desktop');
  const label = (s: string) => s.replaceAll('_', ' ');
  const evidence = (e: InventoryText) => e.value ?? (e.state === 'missing' ? 'not reported' : 'unknown');

  async function load(cursor: string | null = null): Promise<void> {
    const client = session.client;
    if (!client) return;
    controller?.abort();
    controller = new AbortController();
    const mine = ++ticket;
    busy = true; error = ''; apps = null; desktop = null;
    say.say(`Loading ${title.toLowerCase()}.`);
    try {
      const opts = { signal: controller.signal };
      const result = kind === 'applications'
        ? (await client.call('apps.list', { limit: 200, cursor }, opts)).result
        : (await client.call('desktop.contexts', { limit: 200, cursor }, opts)).result;
      if (mine !== ticket) return;
      if ('applications' in result) apps = result;
      else desktop = result;
      next = (apps ?? desktop)?.next_cursor ?? null;
      pageNumber = cursor === null ? 1 : pageNumber + 1;
      say.say(`${title} page ${pageNumber} loaded.`);
    } catch (e) {
      if (mine !== ticket) return;
      apps = null; desktop = null; next = null;
      error = session.handle(e);
      say.say(`${title} unavailable. ${error}`);
    } finally { if (mine === ticket) busy = false; }
  }
  $effect(() => { void session.epoch; untrack(() => void load()); });
  onDestroy(() => { ++ticket; controller?.abort(); say.clear(); });
</script>

<section class="panel inventory">
  <div class="headrow">
    <div>
      <h1>{title}</h1>
      <p class="muted">{kind === 'applications' ? 'Installed application evidence, with each provider’s coverage kept visible.' : 'Personal and public contexts, with redirection and shortcut health kept separate.'}</p>
    </div>
    <button type="button" class="btn" disabled={busy} onclick={() => void load()}>Refresh {title.toLowerCase()}</button>
  </div>
  <p class="sr-only" role="status" aria-live="polite" aria-atomic="true">{say.text}</p>
  {#if error}<p class="bad">{error}</p>{/if}
  {#if busy}<p class="muted">Loading {title.toLowerCase()}…</p>{/if}
  {#if data}
    <p class="muted">Read-only · observed {formatTime(data.observed_at)} · page {pageNumber}</p>
    {#if data.budget_hit}<p class="warn">The read budget was reached. This observation is incomplete.</p>{/if}
    <h2>Provider coverage</h2>
    <dl class="coverage">
      {#each data.providers as provider (provider.provider)}
        <div><dt>{label(provider.provider)}</dt><dd><strong>{label(provider.state)}</strong>{#if provider.reason}<span class="muted"> · {provider.reason}</span>{/if}</dd></div>
      {/each}
    </dl>
  {/if}
  {#if apps}
    <p class="muted">Sizes are provider estimates. Unknown sizes stay unknown; application locations are evidence only.</p>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex (keyboard access to horizontal scrolling) -->
    <div class="tablewrap" tabindex="0" role="region" aria-label="Installed applications">
      <table class="tbl">
        <caption>Applications reported on this page</caption>
        <thead><tr><th scope="col">Application</th><th scope="col">Publisher / version</th><th scope="col">Scope / provenance</th><th scope="col">Estimated size</th><th scope="col">Install location</th></tr></thead>
        <tbody>
          {#each apps.applications as app, i (i)}
            <tr><th scope="row"><VisibleName name={evidence(app.name)} />{#if app.name.expandable}<span class="muted"> · literal expandable text</span>{/if}{#if app.name.reason}<small>{app.name.reason}</small>{/if}</th>
              <td><VisibleName name={evidence(app.publisher)} /><small><VisibleName name={evidence(app.version)} /></small></td>
              <td>{label(app.scope)}<small>{label(app.provider)} · <VisibleName name={app.registry_key} /></small></td>
              <td>{formatBytes(app.estimated_size_bytes)}{#if app.size_reason}<small>{app.size_reason}</small>{/if}</td>
              <td><VisibleName name={evidence(app.install_location)} /></td></tr>
          {/each}
        </tbody>
      </table>
    </div>
    {#if apps.applications.length === 0}<p>No applications reported on this page. See provider coverage above.</p>{/if}
  {/if}
  {#if desktop}
    <h2>Known folders</h2>
    <dl class="contexts">
      {#each desktop.contexts as context (context.folder)}
        <div><dt>{label(context.folder)} <span class="muted">· {label(context.ownership)}</span></dt>
          <dd>{label(context.location)} · {context.redirected === null ? 'redirection unknown' : context.redirected ? 'redirected' : 'default location'}
            {#if context.current_path}<small>Configured: <VisibleName name={context.current_path.text} />{context.current_path.truncated ? ' (truncated)' : ''}</small>{/if}
            {#if context.default_path}<small>Default: <VisibleName name={context.default_path.text} />{context.default_path.truncated ? ' (truncated)' : ''}</small>{/if}
            {#if context.reason}<small>{context.reason}</small>{/if}
          </dd></div>
      {/each}
    </dl>
    <p class="muted">Configured folders do not establish existence or online state. Shortcut health describes recorded metadata at this observation.</p>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex (keyboard access to horizontal scrolling) -->
    <div class="tablewrap" tabindex="0" role="region" aria-label="Desktop shortcuts">
      <table class="tbl">
        <caption>Shortcut evidence on this page</caption>
        <thead><tr><th scope="col">Shortcut / desktop</th><th scope="col">Target health</th><th scope="col">Recorded target</th><th scope="col">Arguments / working directory</th></tr></thead>
        <tbody>{#each desktop.shortcuts as shortcut, i (i)}
          <tr><th scope="row"><VisibleName name={shortcut.name} /><small>{label(shortcut.desktop)}</small></th>
            <td>{label(shortcut.health)}{#if shortcut.reason}<small>{shortcut.reason}</small>{/if}</td>
            <td><VisibleName name={evidence(shortcut.target)} /></td>
            <td><VisibleName name={evidence(shortcut.arguments)} /><small><VisibleName name={evidence(shortcut.working_directory)} /></small></td></tr>
        {/each}</tbody>
      </table>
    </div>
    {#if desktop.shortcuts.length === 0}<p>No shortcut entries reported. See provider coverage above.</p>{/if}
  {/if}
  <div class="paging"><button type="button" class="btn" disabled={busy || !next} onclick={() => void load(next)}>Next page</button></div>
</section>

<style>
  .headrow { display: flex; flex-wrap: wrap; justify-content: space-between; align-items: start; gap: 12px; }
  .headrow > div { flex: 1 1 280px; }
  h2 { margin-top: 24px; }
  dl > div { border-top: 1px solid var(--line); padding: 10px 0; }
  dt { font-weight: 600; }
  dd { margin: 3px 0 0; overflow-wrap: anywhere; }
  small { display: block; color: var(--muted); margin-top: 4px; font-weight: normal; overflow-wrap: anywhere; }
  .tablewrap { overflow-x: auto; }
  .tbl { min-width: 680px; }
  caption { text-align: left; margin: 12px 0 8px; color: var(--muted); }
  th, td { overflow-wrap: anywhere; max-width: 320px; }
  .paging { margin-top: 16px; }
</style>
