<script module lang="ts">
  export const meta = { title: 'Collections', order: 35 };
</script>
<script lang="ts">
  import { onDestroy, tick, untrack } from 'svelte';
  import { session } from '../../lib/stores/session.svelte';
  import { LoomwardError } from '../../lib/transport/client';
  import { formatBytes } from '../../lib/format/bytes';
  import { escapedName } from '../../lib/format/names';
  import { formatCount } from '../../lib/format/time';
  import { Say } from '../../lib/ui/say.svelte';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import ProjectDiscovery from './ProjectDiscovery.svelte';
  import type { Collection, CollectionMember, Cursor } from '../../lib/contracts.gen';

  let collections = $state<Collection[]>([]);
  let selected = $state('');
  let name = $state('');
  let members = $state<CollectionMember[]>([]);
  let selectedMembers = $state<string[]>([]);
  let next = $state<Cursor | null>(null);
  let total = $state<number | null>(null);
  let loading = $state(false);
  let creating = $state(false);
  let removing = $state(false);
  let stateRev = $state<string | null>(null);
  let notice = $state('');
  let error = $state('');
  const result = new Say();
  let needsReread = $state(false);
  let uncertain = '';
  let pendingName = '';
  let sending = false;
  let alive = true;
  let summary = $state<HTMLElement>();
  let more = $state<HTMLButtonElement>();
  let generation = 0;
  let abort: AbortController | null = null;
  const current = $derived(collections.find((c) => c.collection_id === selected));

  function invalidate(): void {
    generation++; abort?.abort(); abort = null;
    if (sending) uncertain = `An earlier removal from “${pendingName}” may already have been saved. Refresh and check its members before trying again.`;
    sending = false; removing = false; loading = false;
    selectedMembers = []; stateRev = null; needsReread = true;
    if (uncertain) result.say(uncertain);
    else result.clear();
  }

  async function load(append = false, restarted = false): Promise<boolean> {
    if (append && (loading || removing || !next)) return false;
    if (!append) invalidate();
    const client = session.client;
    if (!client || session.state !== 'connected') return false;
    abort?.abort();
    const mine = ++generation;
    const ctl = abort = new AbortController();
    const epoch = session.epoch, connection = session.connectionEpoch;
    let collection_id = selected;
    const current = () => alive && mine === generation && !ctl.signal.aborted && client === session.client &&
      session.state === 'connected' && epoch === session.epoch && connection === session.connectionEpoch && collection_id === selected;
    const hadFocus = append && document.activeElement === more;
    loading = true; error = '';
    if (!restarted) notice = '';
    try {
      if (!append) {
        members = []; next = null; total = null;
        const { result } = await client.call('collections.list', {}, { signal: ctl.signal });
        if (!current()) return false;
        collections = result.collections;
        if (!collections.some((c) => c.collection_id === selected)) selected = collection_id = collections[0]?.collection_id ?? '';
      }
      if (!collection_id) return false;
      const { result, meta } = await client.call('collections.members', { collection_id, limit: 50, cursor: append ? next : null }, { signal: ctl.signal });
      if (!current()) return false;
      members = append ? [...members, ...result.items] : result.items;
      next = result.next_cursor; total = result.total;
      stateRev = meta.state_rev;
      needsReread = stateRev === null;
      if (result.budget_hit) notice = 'The work budget was reached; more members may exist.';
      if (hadFocus) { await tick(); if (current() && (document.activeElement === more || document.activeElement === document.body)) (next ? more : summary)?.focus(); }
      return current();
    } catch (e) {
      if (!current()) return false;
      if (e instanceof LoomwardError && e.code === 'stale_generation' && !restarted) {
        notice = 'The collection changed, so its members start again from the top.';
        return await load(false, true);
      }
      error = session.handle(e); members = []; next = null; total = null;
      selectedMembers = []; stateRev = null; needsReread = true;
      return false;
    } finally { if (current()) loading = false; }
  }
  async function create(): Promise<void> {
    if (!session.client || creating || !name.length) return;
    creating = true; error = ''; notice = '';
    try {
      const { result } = await session.client.call('collections.create', { name });
      selected = result.collection_id; name = '';
      await load(); notice = 'Collection created. No file was changed.';
    } catch (e) { error = session.handle(e); }
    finally { creating = false; }
  }
  async function remove(member?: CollectionMember): Promise<void> {
    const client = session.client;
    const ids = [...new Set(member ? [member.member_id] : selectedMembers)];
    if (!client || session.state !== 'connected' || current?.kind !== 'human' || removing || loading || needsReread || stateRev === null || !ids.length || ids.length > 500) return;
    const collection_id = selected, revision = stateRev, name = escapedName(current.name);
    const epoch = session.epoch, connection = session.connectionEpoch, mine = ++generation;
    abort?.abort();
    const ctl = abort = new AbortController();
    const sameContext = () => alive && client === session.client && session.state === 'connected' &&
      epoch === session.epoch && connection === session.connectionEpoch && collection_id === selected;
    const currentRequest = () => sameContext() && mine === generation && !ctl.signal.aborted;
    removing = true; sending = true; pendingName = name; uncertain = ''; error = ''; notice = ''; result.clear();
    try {
      await client.call('collections.update_members', { collection_id, add: [], remove: [], remove_members: ids }, { expectedStateRev: revision, signal: ctl.signal });
      if (!currentRequest()) { if (mine === generation) invalidate(); return; }
      sending = false; removing = false;
      const refresh = load(), refreshedGeneration = generation;
      const loaded = await refresh;
      const currentRefresh = () => sameContext() && generation === refreshedGeneration;
      if (!currentRefresh()) return;
      result.say(member ? 'Member removed. No file was changed.' : `Removal saved for the selection of ${formatCount(ids.length)} ${ids.length === 1 ? 'member' : 'members'} from “${name}”. No file was changed.`);
      if (loaded) { await tick(); if (currentRefresh()) summary?.focus(); }
    } catch (e) {
      if (!currentRequest()) { if (mine === generation) invalidate(); return; }
      if (!(e instanceof LoomwardError) || e.code === 'deadline_exceeded') {
        uncertain = `Removal from “${name}” may already have been saved. Refresh and check its members before trying again.`;
        result.say(uncertain);
      }
      sending = false; selectedMembers = []; stateRev = null; needsReread = true;
      error = session.handle(e);
    } finally { if (mine === generation) { removing = false; sending = false; } }
  }
  $effect(() => {
    session.epoch; session.connectionEpoch; session.client; session.state;
    untrack(() => void load());
    return invalidate;
  });
  onDestroy(() => { alive = false; invalidate(); result.clear(); });
</script>

<h1>Collections</h1>
<p class="lede">Your virtual groupings.</p>
<p class="note">Membership is a preference, never permission to act. Files stay where they are.</p>
<p class="live muted" class:has={!!result.text || !!notice} role="status" aria-label="Collection result">{result.text || notice}</p>
<p class="live bad" class:has={!!error} role="alert" aria-label="Collection error">{error}</p>

<section class="panel" aria-labelledby="collection-make">
  <h2 id="collection-make">Make a collection</h2>
  <form class="controls" onsubmit={(e) => { e.preventDefault(); void create(); }}>
    <label class="lw-field"><span class="lbl">Collection name</span> <input type="text" bind:value={name} maxlength="64" required /></label>
    <button class="btn primary" type="submit" aria-disabled={creating || !name.length} onclick={(e) => { if (creating || !name.length) e.preventDefault(); }}>Create collection</button>
  </form>
</section>

<section class="panel">
  <div class="controls pick">
    <label class="lw-field"><span class="lbl">Collection</span> <select bind:value={selected} onchange={(e) => { selected = e.currentTarget.value; void load(); }} disabled={!collections.length}>
      {#each collections as c (c.collection_id)}<option value={c.collection_id}>{escapedName(c.name)}</option>{/each}
    </select></label>
    <button class="btn" type="button" aria-disabled={loading} onclick={() => { if (!loading) void load(); }}>Refresh collections</button>
  </div>
  {#if current}
    <h2><VisibleName name={current.name} /></h2>
    <p class="muted">{current.member_count} members · virtual size {formatBytes(current.logical_bytes)}. Overlapping entries may count the same bytes.</p>
  {:else if !loading && !error}<p class="empty">No collections yet. Create one, then add a node from Explorer’s inspector.</p>{/if}
  {#if current?.kind === 'human'}
    <div class="controls">
      <p class="muted" role="status" aria-label="Selected collection members">{formatCount(selectedMembers.length)} selected · only checked members will be removed.</p>
      <button class="btn" type="button" disabled={!selectedMembers.length || loading || removing || needsReread || stateRev === null} onclick={() => void remove()}>Remove selected members</button>
      <button class="btn" type="button" disabled={!selectedMembers.length || removing} onclick={() => { selectedMembers = []; }}>Clear member selection</button>
    </div>
    <p class="note">Select up to 500 members across loaded pages. Removing membership leaves files and other collections unchanged.</p>
    {#if selectedMembers.length >= 500}<p class="muted">The 500-member limit is reached. Clear a checkbox to choose another member.</p>{/if}
  {/if}
  <div class="tbl-wrap" aria-busy={loading}>
    <table class="tbl">
      <caption class="sr-only">Collection members</caption>
      <thead><tr><th scope="col">Member</th><th scope="col">Status</th><th scope="col" class="r">Logical size</th><th scope="col">Location</th><th scope="col">Membership</th></tr></thead>
      <tbody>
        {#each members as member (member.member_id)}
          <tr><th scope="row">{#if member.entry}<VisibleName name={member.entry.name} />{:else}Unresolved member <span class="muted">{member.member_id}</span>{/if}</th>
            <td>{#if member.status === 'resolved'}Resolved{:else}<span class="tag warn">{member.status === 'root_revoked' ? 'Root revoked' : member.status === 'root_unavailable' ? 'Root unavailable' : 'Object unavailable'}</span>{/if}</td>
            <td class="r num">{formatBytes(member.entry?.logical_bytes ?? null)}</td>
            <td>{#if member.entry?.location_hint}<VisibleName name={member.entry.location_hint.text} />{:else}<span class="unknown">unknown</span>{/if}</td>
            <td>{#if current?.kind === 'human'}
              <label><input type="checkbox" value={member.member_id} bind:group={selectedMembers}
                aria-label={`Select member ${member.member_id}${member.entry ? `: ${escapedName(member.entry.name)}` : ''}`}
                disabled={loading || removing || needsReread || stateRev === null || (selectedMembers.length >= 500 && !selectedMembers.includes(member.member_id))} /> Select</label>
              <button class="btn" type="button" aria-label={`Remove member ${member.member_id}`} aria-disabled={loading || removing || needsReread || stateRev === null} onclick={() => void remove(member)}>Remove</button>
            {/if}</td>
          </tr>
        {:else}<tr><td colspan="5" class="muted">{loading ? 'Loading members' : error ? 'Members unavailable.' : 'No members.'}</td></tr>{/each}
      </tbody>
    </table>
  </div>
  <div class="foot">
    <span class="muted" tabindex="-1" bind:this={summary}>Showing {members.length} of {total ?? 'unknown'} members</span>
    {#if next}<button class="btn" type="button" bind:this={more} aria-disabled={loading || removing} onclick={() => { if (!loading && !removing) void load(true); }}>Load more members</button>{/if}
  </div>
</section>
<ProjectDiscovery collectionListKey={collections.map((c) => c.collection_id).join(',')} onAdded={() => void load()} />
<style>
  .pick { margin-top: 0; }
  input, select { max-width: 100%; }
  .foot { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: var(--sp-3); margin-top: var(--sp-3); }
</style>
