<script lang="ts">
  // The alert for a failed slice load, with a way to try again (shared by Atlas and Observatory).
  import type { SliceNav } from './shared.svelte';
  let { nav }: { nav: SliceNav } = $props();
</script>

{#if nav.error}
  <p class="bad" role="alert">
    Could not load this view: {nav.error}
    {nav.trail.length ? 'Nothing is drawn until a load succeeds; the breadcrumb shows where you still are.' : 'Nothing is drawn until a load succeeds, and there is no breadcrumb yet.'}
    <button type="button" class="btn" disabled={nav.busy} onclick={() => void nav.load()}>Try again</button>
  </p>
{/if}

<style>
  .bad { color: var(--danger); }
</style>
