<script module lang="ts">
  export const meta = { title: 'Collections', order: 35 };
</script>
<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { session } from '../../lib/stores/session.svelte';
  import { LoomwardError } from '../../lib/transport/client';
  import { formatBytes } from '../../lib/format/bytes';
  import { escapedName } from '../../lib/format/names';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import type { Collection, CollectionMember, Cursor } from '../../lib/contracts.gen';

  let collections = $state<Collection[]>([]);
  let selected = $state('');
  let name = $state('');
  let members = $state<CollectionMember[]>([]);
  let next = $state<Cursor | null>(null);
  let total = $state<number | null>(null);
  let loading = $state(false);
  let creating = $state(false);
  let removing = $state(false);
  let stateRev = $state<string | null>(null);
  let notice = $state('');
  let error = $state('');
  let summary = $state<HTMLElement>();
  let more = $state<HTMLButtonElement>();
  let generation = 0;
  let abort: AbortController | null = null;
  const current = $derived(collections.find((c) => c.collection_id === selected));

  async function load(append = false, restarted = false): Promise<void> {
    if (!session.client) return;
    abort?.abort();
    const mine = ++generation;
    const ctl = abort = new AbortController();
    const hadFocus = append && document.activeElement === more;
    loading = true; error = '';
    if (!restarted) notice = '';
    try {
      if (!append) {
        members = []; next = null; total = null;
        const { result } = await session.client.call('collections.list', {}, { signal: ctl.signal });
        if (mine !== generation) return;
        collections = result.collections;
        if (!collections.some((c) => c.collection_id === selected)) selected = collections[0]?.collection_id ?? '';
      }
      if (!selected) return;
      const { result, meta } = await session.client.call('collections.members', { collection_id: selected, limit: 50, cursor: append ? next : null }, { signal: ctl.signal });
      if (mine !== generation) return;
      members = append ? [...members, ...result.items] : result.items;
      next = result.next_cursor; total = result.total;
      stateRev = meta.state_rev;
      if (result.budget_hit) notice = 'The work budget was reached; more members may exist.';
      if (hadFocus) { await tick(); if (document.activeElement === more || document.activeElement === document.body) (next ? more : summary)?.focus(); }
    } catch (e) {
      if (mine !== generation) return;
      if (e instanceof LoomwardError && e.code === 'stale_generation' && !restarted) {
        notice = 'The collection changed, so its members start again from the top.';
        return await load(false, true);
      }
      error = session.handle(e); members = []; next = null; total = null;
    } finally { if (mine === generation) loading = false; }
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
  async function remove(member: CollectionMember): Promise<void> {
    if (!session.client || removing || loading || stateRev === null) return;
    removing = true; error = ''; notice = '';
    try {
      await session.client.call('collections.update_members', { collection_id: selected, add: [], remove: [], remove_members: [member.member_id] }, { expectedStateRev: stateRev });
      await load(); notice = 'Member removed. No file was changed.';
      await tick(); summary?.focus();
    } catch (e) {
      if (e instanceof LoomwardError && e.code === 'stale_generation') await load();
      error = session.handle(e);
    } finally { removing = false; }
  }
  $effect(() => {
    session.epoch;
    untrack(() => void load());
    return () => { generation++; abort?.abort(); };
  });
</script>

<h1>Collections</h1>
<p class="muted">Your virtual groupings. Membership is a preference, never permission to act. Files stay where they are.</p>
<section class="panel">
  <h2>Make a collection</h2>
  <form class="bar" onsubmit={(e) => { e.preventDefault(); void create(); }}>
    <label>Collection name <input type="text" bind:value={name} maxlength="64" required /></label>
    <button class="btn" type="submit" aria-disabled={creating || !name.length} onclick={(e) => { if (creating || !name.length) e.preventDefault(); }}>Create collection</button>
  </form>
  <p class="live muted" role="status" aria-label="Collection result">{notice}</p>
  <p class="live bad" role="alert">{error}</p>
  <div class="bar">
    <label>Collection <select bind:value={selected} onchange={() => void load()} disabled={!collections.length}>
      {#each collections as c (c.collection_id)}<option value={c.collection_id}>{escapedName(c.name)}</option>{/each}
    </select></label>
    <button class="btn" type="button" aria-disabled={loading} onclick={() => { if (!loading) void load(); }}>Refresh collections</button>
  </div>
  {#if current}
    <h2><VisibleName name={current.name} /></h2>
    <p class="muted">{current.member_count} members · virtual size {formatBytes(current.logical_bytes)}. Overlapping entries may count the same bytes.</p>
  {:else if !loading && !error}<p class="muted">No collections yet. Create one, then add a node from Explorer’s inspector.</p>{/if}
  <div class="tbl-wrap" aria-busy={loading}>
    <table class="tbl">
      <caption class="sr-only">Collection members</caption>
      <thead><tr><th scope="col">Member</th><th scope="col">Status</th><th scope="col">Logical size</th><th scope="col">Location</th><th scope="col">Membership</th></tr></thead>
      <tbody>
        {#each members as member (member.member_id)}
          <tr><th scope="row">{#if member.entry}<VisibleName name={member.entry.name} />{:else}Unresolved member <span class="muted">{member.member_id}</span>{/if}</th>
            <td>{member.status === 'resolved' ? 'Resolved' : member.status === 'root_revoked' ? 'Root revoked' : member.status === 'root_unavailable' ? 'Root unavailable' : 'Object unavailable'}</td>
            <td class="num">{formatBytes(member.entry?.logical_bytes ?? null)}</td>
            <td>{#if member.entry?.location_hint}<VisibleName name={member.entry.location_hint.text} />{:else}<span class="unknown">unknown</span>{/if}</td>
            <td>{#if current?.kind === 'human'}<button class="btn" type="button" aria-label={`Remove member ${member.member_id}`} aria-disabled={loading || removing || stateRev === null} onclick={() => void remove(member)}>Remove</button>{/if}</td>
          </tr>
        {:else}<tr><td colspan="5" class="muted">{loading ? 'Loading members' : error ? 'Members unavailable.' : 'No members.'}</td></tr>{/each}
      </tbody>
    </table>
  </div>
  <div class="bar foot">
    <span class="muted" tabindex="-1" bind:this={summary}>Showing {members.length} of {total ?? 'unknown'} members</span>
    {#if next}<button class="btn" type="button" bind:this={more} aria-disabled={loading} onclick={() => { if (!loading) void load(true); }}>Load more members</button>{/if}
  </div>
</section>
<style>
  .bar { display: flex; flex-wrap: wrap; align-items: center; gap: 12px; }
  label { display: inline-flex; flex-wrap: wrap; align-items: center; gap: 8px; }
  input, select { max-width: 100%; }
  .foot { justify-content: space-between; margin-top: 12px; }
  .bad { color: var(--danger); }
</style>
