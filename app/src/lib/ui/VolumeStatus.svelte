<script lang="ts">
  import { volumes } from '../stores/volumes.svelte';
  import { volumeMarkers } from '../format/volume';

  let { id, showOffline = true }: { id: string | null; showOffline?: boolean } = $props();
  const volume = $derived(volumes.items?.find((v) => v.volume_id === id));
</script>

{#each volumeMarkers(volume).filter((m) => showOffline || m !== 'Offline') as marker (marker)}
  <span class="tag">{marker}</span>
{/each}
{#if id && volumes.error}<span class="muted small">Volume details unavailable: {volumes.error}</span>{/if}
