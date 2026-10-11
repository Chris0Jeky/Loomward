<script lang="ts">
  import { onMount } from 'svelte';
  import { session } from '../stores/session.svelte';
  import type { Collection, NodeId } from '../contracts.gen';
  import { escapedName } from '../format/names';
  import { Say } from './say.svelte';
  let { nodeId }: { nodeId: NodeId } = $props();
  let collections = $state<Collection[]>([]);
  let selected = $state('');
  let busy = $state(false);
  let error = $state('');
  const result = new Say();
  onMount(() => {
    const ctl = new AbortController();
    void session.client!.call('collections.list', {}, { signal: ctl.signal }).then(({ result }) => {
      if (!ctl.signal.aborted) { collections = result.collections.filter((c) => c.kind === 'human'); selected = collections[0]?.collection_id ?? ''; }
    }).catch((e) => { if (!ctl.signal.aborted) error = session.handle(e); });
    return () => { ctl.abort(); result.clear(); };
  });
  async function add(): Promise<void> {
    if (!selected || !session.client || busy) return;
    const id = nodeId, collection_id = selected;
    busy = true; error = ''; result.clear();
    try {
      const { meta } = await session.client.call('collections.members', { collection_id, limit: 1, cursor: null });
      if (meta.state_rev === null) throw new Error('The collection revision is unknown; refresh before adding.');
      await session.client.call('collections.update_members', { collection_id, add: [id], remove: [] }, { expectedStateRev: meta.state_rev });
      result.say('Added to collection. No file was changed.');
    } catch (e) { error = session.handle(e); }
    finally { busy = false; }
  }
</script>
<div class="controls">
  <label class="ctl"><span class="lbl">Add to collection</span> <select bind:value={selected} disabled={busy || !collections.length}>
    {#each collections as c (c.collection_id)}<option value={c.collection_id}>{escapedName(c.name)}</option>{/each}
  </select></label>
  <button class="btn primary" type="button" disabled={!selected} aria-disabled={busy} onclick={() => void add()}>Add to collection</button>
</div>
{#if !collections.length && !error}<p class="empty">Create a collection in the <a href="#/collections">Collections view</a> first.</p>{/if}
<p class="live muted" class:has={!!result.text} role="status" aria-label="Membership result">{result.text}</p>
<p class="live bad" class:has={!!error} role="alert">{error}</p>
<style>
  select { max-width: 100%; }
</style>
