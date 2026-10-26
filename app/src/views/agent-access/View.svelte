<script module lang="ts">
  export const meta = { title: 'Agent access', order: 72 };
</script>
<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { session } from '../../lib/stores/session.svelte';
  import { formatTime } from '../../lib/format/time';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import VolumeStatus from '../../lib/ui/VolumeStatus.svelte';
  import type { ClientField } from '../../lib/contracts.gen';
  import { accessState, fieldOptions, refreshAccess, grantAccess, revokeAccess } from './model';

  const access = $state(accessState());
  let name = $state('');
  let selectedRoots = $state<string[]>([]);
  let selectedFields = $state<ClientField[]>([]);
  let expiry = $state('');
  let confirmation = $state<HTMLElement>();
  let heading = $state<HTMLElement>();
  const activeRoots = $derived(access.roots?.filter(r => r.grant_state === 'active') ?? []);
  const options = $derived(fieldOptions(access.list?.available_fields ?? []));
  const connect = $derived(`claude mcp add loomward -- "%LOCALAPPDATA%\\Loomward\\loomward-mcp.exe" --client ${/^[A-Za-z0-9._-]{1,64}$/.test(name) ? name : '<name>'}`);
  const showError = (e: unknown) => session.handle(e);

  $effect(() => {
    void session.epoch;
    const c = session.client;
    if (c) untrack(() => {
      selectedRoots = []; selectedFields = [];
      void refreshAccess(access, c, showError);
    });
  });

  async function confirm(client: string) {
    access.pendingRevoke = client;
    await tick(); confirmation?.focus();
  }
  async function revoke() {
    if (!session.client) return;
    await revokeAccess(access, session.client, showError);
    await tick(); heading?.focus();
  }
</script>

<header class="view-head">
  <div><h1 tabindex="-1" bind:this={heading}>Agent access</h1>
    <p class="muted">Choose what each connected agent may read. Revoke access at any time.</p></div>
  <button class="btn" disabled={access.busy} onclick={() => session.client && refreshAccess(access, session.client, showError)}>Refresh access</button>
</header>
<p class="live bad" class:has={!!access.error} role="alert">{access.error}</p>
<p class="live muted" class:has={!!access.notice} role="status">{access.notice}</p>

<section class="panel" aria-labelledby="access-grants">
  <h2 id="access-grants">Client grants</h2>
  {#if access.list === null}
    <p class="muted">{access.busy ? 'Loading client grants…' : 'Client grants unavailable. Refresh to try again.'}</p>
  {:else if !access.list.grants.length}
    <p class="muted">No MCP client grants have been created.</p>
  {:else}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex (keyboard access to horizontal scrolling) -->
    <div class="tbl-wrap" tabindex="0" role="region" aria-label="Scrollable MCP client grants"><table class="tbl" aria-label="MCP client grants">
      <thead><tr><th scope="col">Client</th><th scope="col">Folders and details</th><th scope="col">Dates</th><th scope="col">Access</th></tr></thead>
      <tbody>{#each access.list.grants as grant (grant.grant_id)}
        <tr>
          <th scope="row"><VisibleName name={grant.client} /><div class="muted small">{grant.grant_id}</div></th>
          <td>{#if grant.roots.length}<ul class="list">{#each grant.roots as id (id)}
              {@const root = access.roots?.find(r => r.root_id === id)}
              <li><VisibleName name={root?.display_path.text ?? id} /> <VolumeStatus id={root?.volume_id ?? null} /></li>
            {/each}</ul>{:else}<span class="muted">Original folders no longer available</span>{/if}
            <div class="small">{grant.fields.map(f => options.find(o => o.field === f)?.explanation ?? f).join('; ') || 'No optional fields'}</div></td>
          <td class="small">Created {formatTime(grant.created_at)}<br />Expires {grant.expires_at ? formatTime(grant.expires_at) : 'never'}
            {#if grant.revoked_at}<br />Revoked {formatTime(grant.revoked_at)}{/if}</td>
          <td>{#if grant.revoked_at}<span class="tag">Revoked</span>
            {:else}<span class="tag">{grant.expires_at && Date.parse(grant.expires_at) <= Date.now() ? 'Expired' : 'Not revoked'}</span>
              <button class="btn" disabled={access.busy} onclick={() => confirm(grant.client)} aria-label={`Revoke ${grant.client}`}>Revoke</button>{/if}</td>
        </tr>
      {/each}</tbody>
    </table></div>
  {/if}
  {#if access.pendingRevoke}
    <div class="confirmation" tabindex="-1" bind:this={confirmation} role="group" aria-label="Confirm revocation">
      <p>Revoke <strong>{access.pendingRevoke}</strong>? The agent will be refused from its next call. Its grant history stays visible.</p>
      <div class="controls"><button class="btn" disabled={access.busy} onclick={() => access.pendingRevoke = null}>Keep access</button>
        <button class="btn" disabled={access.busy} onclick={revoke}>Confirm revoke</button></div>
    </div>
  {/if}
</section>

<section class="panel" aria-labelledby="access-create">
  <h2 id="access-create">Create or replace a grant</h2>
  <p class="muted">A new grant replaces this client's earlier grant. It gives the agent read access within your chosen folders; it cannot move or delete files through Loomward.</p>
  {#if access.list && access.roots}
    <form onsubmit={(e) => { e.preventDefault(); if (session.client) void grantAccess(access, session.client, { name, roots: selectedRoots, fields: selectedFields, expiry }, showError); }}>
      <fieldset disabled={access.busy}>
        <legend>Agent scope</legend>
        <label class="lw-field"><span class="lbl">Client name</span><input type="text" bind:value={name} required pattern={'[A-Za-z0-9._\\-]{1,64}'} maxlength="64" autocomplete="off" spellcheck="false" aria-describedby="client-name-help" /></label>
        <p id="client-name-help" class="muted small">1–64 letters, digits, dots, underscores or hyphens. The principal is mcp:{name || '<name>'}.</p>
        <fieldset><legend>Folders</legend>
          {#each activeRoots as root (root.root_id)}<label class="check"><input type="checkbox" value={root.root_id} bind:group={selectedRoots} /> <VisibleName name={root.display_path.text} /> <VolumeStatus id={root.volume_id} /></label>{/each}
          {#if !activeRoots.length}<p class="muted">Add a folder in Grants &amp; health first.</p>{/if}
        </fieldset>
        <fieldset><legend>Details the agent may read</legend>
          {#each options as option (option.field)}<label class="check"><input type="checkbox" value={option.field} bind:group={selectedFields} /> {option.explanation}</label>{/each}
        </fieldset>
        <p class="disclosure">The agent always sees the names and kinds of files and folders in the chosen folders. Its host may send what it reads to its model provider; Loomward cannot see or control that. The client name is a label, not a password: any program on this PC can use it.</p>
        <label class="lw-field"><span class="lbl">Expiry (optional, local time)</span><input type="datetime-local" bind:value={expiry} /></label>
        <button class="btn primary" type="submit" disabled={!selectedRoots.length}>Create grant</button>
      </fieldset>
    </form>
  {:else}<p class="muted">Folder and grant details must be available before creating access.</p>{/if}
</section>
<section class="panel" aria-labelledby="access-connect">
  <h2 id="access-connect">Connect an agent</h2>
  <p class="muted">Use the same client name in your agent host. For Claude Code:</p>
  <pre class="connect"><code>{connect}</code></pre>
</section>

<style>
  .view-head { display: flex; align-items: start; justify-content: space-between; gap: var(--sp-3); flex-wrap: wrap; }
  fieldset { min-width: 0; margin: var(--sp-3) 0; }
  .lw-field { max-width: 28rem; }
  input[type='datetime-local'] { font: inherit; color: var(--text); background: var(--bg); border: 1px solid var(--control-line); border-radius: var(--radius); padding: 6px 10px; max-width: 100%; }
  .check { display: flex; align-items: baseline; gap: var(--sp-2); margin-block: var(--sp-2); }
  .disclosure, .confirmation { max-width: var(--measure); padding: var(--sp-3); background: var(--bg); border: 1px solid var(--line); border-radius: var(--radius); }
  .connect { max-width: 100%; white-space: pre-wrap; overflow-wrap: anywhere; }
  td { vertical-align: top; }
  .tbl { min-width: 45rem; }
</style>
