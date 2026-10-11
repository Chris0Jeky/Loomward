<script lang="ts">
  import { untrack } from 'svelte';
  import { Meaning } from '../stores/meaning.svelte';
  import { session } from '../stores/session.svelte';
  import MeaningThread from './MeaningThread.svelte';
  import type { MeaningThread as Thread } from '../contracts.gen';
  let { nodeId, meaning }: { nodeId: string; meaning?: Thread } = $props();
  const own = new Meaning();
  $effect(() => {
    const id = nodeId, supplied = meaning, client = session.client;
    void session.epoch;
    if (!supplied && client) untrack(() => void own.load(client, [id]));
    return () => own.clear();
  });
</script>

{#if own.busy}<p class="muted" role="status">Reading meaning…</p>{/if}
{#if own.message}<p class="muted" role="status">{own.message}</p>{/if}
<MeaningThread meaning={meaning ?? own.items[nodeId]} />
