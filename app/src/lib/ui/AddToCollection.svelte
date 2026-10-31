<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { session } from '../stores/session.svelte';
  import type { Collection, NodeId } from '../contracts.gen';
  import { escapedName } from '../format/names';
  import { Say } from './say.svelte';
  type Props = { onAdded?: () => void; contextKey?: number } & (
    { nodeId: NodeId; expectedGeneration?: string; nodeIds?: never } |
    { nodeIds: NodeId[]; nodeId?: never; expectedGeneration?: never }
  );
  let { nodeId, nodeIds, expectedGeneration, onAdded, contextKey = 0 }: Props = $props();
  const fieldId = $props.id();
  const ids = $derived([...new Set(nodeIds ?? (nodeId ? [nodeId] : []))]);
  const selectionKey = $derived(ids.join('\0'));
  const batch = $derived(nodeIds !== undefined);
  let collections = $state<Collection[]>([]);
  let selected = $state('');
  let loading = $state(true);
  let busy = $state(false);
  let error = $state('');
  const result = new Say();
  let alive = true;
  let generation = 0;
  let request: AbortController | null = null;
  let sending = false;
  let unknownOutcome = false;

  function invalidate() {
    generation++; request?.abort(); request = null;
    unknownOutcome ||= sending;
    busy = false; sending = false; error = '';
    if (unknownOutcome) result.say('An earlier collection update may already have been saved. Check Collections before trying again.');
    else result.clear();
  }

  $effect(() => {
    selectionKey; batch; selected; expectedGeneration; contextKey;
    session.epoch; session.connectionEpoch; session.client; session.state;
    untrack(invalidate);
  });
  onMount(() => {
    const ctl = new AbortController();
    const client = session.client, connection = session.connectionEpoch;
    const current = () => alive && !ctl.signal.aborted && client === session.client && connection === session.connectionEpoch;
    void client!.call('collections.list', {}, { signal: ctl.signal }).then(({ result }) => {
      if (current()) { collections = result.collections.filter((c) => c.kind === 'human'); selected = collections[0]?.collection_id ?? ''; }
    }).catch((e) => { if (current()) error = session.handle(e); })
      .finally(() => { if (current()) loading = false; });
    return () => { alive = false; ctl.abort(); invalidate(); result.clear(); };
  });
  async function add(): Promise<void> {
    const client = session.client;
    if (!selected || !client || session.state !== 'connected' || busy || !ids.length || ids.length > 500) return;
    const add = [...ids], collection_id = selected, observed = expectedGeneration, key = selectionKey, asBatch = batch;
    const name = escapedName(collections.find(c => c.collection_id === collection_id)?.name ?? 'the collection');
    const epoch = session.epoch, connection = session.connectionEpoch, context = contextKey, mine = ++generation;
    const ctl = request = new AbortController();
    const current = () => alive && mine === generation && !ctl.signal.aborted && client === session.client &&
      session.state === 'connected' && epoch === session.epoch && connection === session.connectionEpoch &&
      context === contextKey && key === selectionKey && asBatch === batch && collection_id === selected && observed === expectedGeneration;
    busy = true; error = ''; unknownOutcome = false; result.clear();
    try {
      const { meta } = await client.call('collections.members', { collection_id, limit: 1, cursor: null }, { signal: ctl.signal });
      if (!current()) { if (mine === generation) invalidate(); return; }
      if (meta.state_rev === null) throw new Error('The collection revision is unknown; refresh before adding.');
      sending = true;
      await client.call('collections.update_members', { collection_id, add, remove: [] }, { expectedStateRev: meta.state_rev, expectedGeneration: asBatch ? undefined : observed, signal: ctl.signal });
      if (!current()) { if (mine === generation) invalidate(); return; }
      sending = false;
      result.say(asBatch ? `Saved selection of ${add.length} ${add.length === 1 ? 'row' : 'rows'} to “${name}”. No file was changed.` : 'Added to collection. No file was changed.');
      onAdded?.();
    } catch (e) { if (current()) error = session.handle(e); }
    finally { if (mine === generation) { busy = false; sending = false; request = null; } }
  }
</script>
<div class="controls">
  <div class="lw-field"><label class="lbl" for={fieldId}>{batch ? 'Collection for selected rows' : 'Add to collection'}</label> <select id={fieldId} bind:value={selected} disabled={busy || loading || !collections.length}>
    {#each collections as c (c.collection_id)}<option value={c.collection_id}>{escapedName(c.name)}</option>{/each}
  </select></div>
  <button class="btn primary" type="button" disabled={!selected || !ids.length || ids.length > 500} aria-disabled={busy} onclick={() => void add()}>{batch ? 'Add selected to collection' : 'Add to collection'}</button>
</div>
{#if loading}<p class="muted">Loading collections…</p>
{:else if !collections.length && !error}<p class="empty">Create a collection in the <a href="#/collections">Collections view</a> first.</p>{/if}
{#if batch}<p class="muted small">{ids.length} selected {ids.length === 1 ? 'row' : 'rows'} · virtual membership only; files stay in place.</p>{/if}
{#if ids.length > 500}<p class="bad" role="alert">Choose at most 500 rows for one collection update.</p>{/if}
<p class="live muted" class:has={!!result.text} role="status" aria-label="Membership result">{result.text}</p>
<p class="live bad" class:has={!!error} role="alert">{error}</p>
<style>
  select { max-width: 100%; }
</style>
