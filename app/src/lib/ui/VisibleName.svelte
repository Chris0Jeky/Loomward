<script lang="ts">
  import { nameSegments } from '../format/names';

  let { name }: { name: string } = $props();
  const segments = $derived(nameSegments(name));
</script>

<bdi class="vname">{#each segments as s, i (i)}{#if s.code}<span class="ctl" title="Hidden character in this name">{s.text}</span>{:else}{s.text}{/if}{/each}</bdi>

<style>
  .vname { overflow-wrap: anywhere; }
  /* assistive technology hears "hidden character" before the code; the visible badge is unchanged */
  .ctl::before {
    content: 'hidden character '; position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap;
  }
  .ctl {
    display: inline-block; margin: 0 1px; padding: 0 5px; font: 0.72em/1.5 var(--font-mono);
    color: var(--warn); border: 1px solid var(--warn); border-radius: 4px; white-space: nowrap; vertical-align: 0.1em;
  }
</style>
