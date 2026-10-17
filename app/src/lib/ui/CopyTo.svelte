<script lang="ts">
  import { copy, type CopyItem } from '../stores/copy.svelte';
  let { items }: { items: CopyItem[] } = $props();
</script>

<button class="btn" type="button" disabled={!items.length || copy.locked || copy.busy}
  onclick={() => { if (copy.select(items)) location.hash = '#/copy'; }}>Copy to…</button>
{#if copy.locked}<a href="#/copy">View current copy operation</a>{/if}
