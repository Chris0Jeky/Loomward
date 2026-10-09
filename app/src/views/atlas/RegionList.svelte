<script lang="ts">
  // The text equivalent of the canvas: the open node's children as a list, every one reachable by
  // keyboard, with the same honest notes (denied, unmeasured, unknown allocation, not in this slice).
  import { formatBytes } from '../../lib/format/bytes';
  import { formatCount } from '../../lib/format/time';
  import type { TreeSlice } from '../../lib/types';

  interface Props { slice: TreeSlice | null; onopen: (id: string, name: string) => void }
  let { slice, onopen }: Props = $props();

  const rows = $derived.by(() => {
    if (!slice) return [];
    return slice.nodes
      .filter((n) => n.parent === 0)
      .map((n) => {
        const notes: string[] = [];
        if (n.coverage === 'denied' || n.threads.permission.state === 'denied') notes.push('access denied: size and contents unknown');
        else if (n.coverage !== 'complete') notes.push(`coverage ${n.coverage.replaceAll('_', ' ')}`);
        if (n.size_unknown_files > 0) notes.push(`${formatCount(n.size_unknown_files)} with unknown allocation`);
        if (n.kind === 'other') notes.push(`${formatCount(n.folded_count)} items folded`);
        if (n.live) notes.push('provisional');
        const open = n.kind !== 'file' && n.kind !== 'other' && n.coverage !== 'denied' && (n.child_count ?? 0) > 0;
        return { n, notes, open, size: n.coverage === 'denied' && n.size_bytes === '0' ? 'unknown' : formatBytes(n.size_bytes) };
      });
  });
  const rest = $derived.by(() => {
    if (!slice?.nodes[0]) return null;
    const listed = rows.reduce((s, r) => s + BigInt(r.n.size_bytes), 0n);
    const total = BigInt(slice.nodes[0].size_bytes);
    return total > listed ? formatBytes(String(total - listed)) : null;
  });
</script>

<details class="as-text">
  <summary>Regions as text ({formatCount(rows.length)})</summary>
  <ol>
    {#each rows as r (r.n.node_id)}
      <li>
        {#if r.open}
          <button type="button" class="open" onclick={() => onopen(r.n.node_id, r.n.name)}><bdi>{r.n.name}</bdi></button>
        {:else}
          <bdi>{r.n.name}</bdi>
        {/if}
        <span class="num size">{r.size}</span>
        {#if r.notes.length}<span class="notes">{r.notes.join(' · ')}</span>{/if}
      </li>
    {/each}
    {#if rest}<li class="rest">Not in this slice <span class="num size">{rest}</span></li>{/if}
  </ol>
</details>

<style>
  .as-text { margin-top: 10px; font-size: 0.85rem; }
  summary { cursor: pointer; color: var(--muted); }
  ol { margin: 8px 0 0; padding: 0 0 0 1.4em; max-height: 280px; overflow: auto; }
  li { padding: 2px 0; overflow-wrap: anywhere; }
  .open { font: inherit; color: var(--accent); background: none; border: 0; padding: 0; cursor: pointer; text-align: left; }
  .size { color: var(--muted); margin-left: 8px; }
  .notes { display: block; color: var(--faint); font-size: 0.8rem; }
  .rest { color: var(--faint); list-style: none; }
</style>
