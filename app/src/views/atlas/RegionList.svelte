<script lang="ts">
  // The text equivalent of the canvas: the open node's children as a list, every one reachable by
  // keyboard (open or inspect), with the same honest notes (denied, unmeasured, unknown allocation,
  // not in this slice). Names go through VisibleName, so hidden characters show as badges.
  import { formatBytes, parseBytes } from '../../lib/format/bytes';
  import { formatCount } from '../../lib/format/time';
  import { escapedName } from '../../lib/format/names';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import type { SliceNode, TreeSlice } from '../../lib/contracts.gen';

  interface Props {
    slice: TreeSlice | null;
    onopen: (id: string, name: string) => void;
    oninspect: (id: string) => void;
  }
  let { slice, onopen, oninspect }: Props = $props();

  const KIND: Record<string, string> = { dir: 'folder', file: 'file', root: 'root folder', volume: 'volume', atlas: 'all roots', other: 'folded items' };

  /** Size under the slice's basis, with unknown never shown as 0 B. */
  function sizeText(n: SliceNode, allocated: boolean): string {
    const b = parseBytes(n.size_bytes);
    const denied = n.coverage === 'denied' || n.threads.permission.state === 'denied';
    if (b === null) return 'unknown';
    if (b === 0n && denied) return 'unknown';
    if (allocated && n.size_unknown_files > 0) return b === 0n ? 'unknown' : `at least ${formatBytes(n.size_bytes)}`;
    return formatBytes(n.size_bytes);
  }

  const rows = $derived.by(() => {
    if (!slice) return [];
    const allocated = slice.basis === 'allocated';
    return slice.nodes
      .filter((n) => n.parent === 0)
      .map((n) => {
        const notes: string[] = [KIND[n.kind] ?? n.kind];
        const t = n.threads;
        notes.push(t.residency.tier === null ? 'residency unknown' : `tier ${t.residency.tier}`);
        if (t.meaning.state !== 'none') notes.push(`meaning ${t.meaning.state}${t.meaning.label ? ` ${escapedName(t.meaning.label)}` : ''}`);
        if (t.permission.state !== 'granted' && t.permission.state !== 'denied') notes.push(`permission ${t.permission.state}`);
        if (n.coverage === 'denied' || n.threads.permission.state === 'denied') notes.push('access denied: size and contents unknown');
        else if (n.coverage !== 'complete') notes.push(`coverage ${n.coverage.replaceAll('_', ' ')}`);
        if (n.size_unknown_files > 0) notes.push(`${formatCount(n.size_unknown_files)} with unknown allocation`);
        if (n.kind === 'other') notes.push(n.folded_count === null ? 'some items folded (count unknown)' : `${formatCount(n.folded_count)} items folded`);
        if (n.live) notes.push('provisional');
        const open = n.kind !== 'file' && n.kind !== 'other' && n.coverage !== 'denied' && (n.child_count ?? 0) > 0;
        return { n, notes, open, size: sizeText(n, allocated) };
      });
  });
  const rest = $derived.by(() => {
    const total = parseBytes(slice?.nodes[0]?.size_bytes);
    if (total === null) return null;
    let listed = 0n;
    for (const r of rows) listed += parseBytes(r.n.size_bytes) ?? 0n; // a malformed size degrades to nothing listed, never a throw
    return total > listed ? formatBytes(String(total - listed)) : null;
  });
</script>

<details class="as-text">
  <summary>Regions as text ({formatCount(rows.length)})</summary>
  <ol>
    {#each rows as r (r.n.node_id)}
      <li>
        {#if r.open}
          <button type="button" class="open" onclick={() => onopen(r.n.node_id, r.n.name)}><VisibleName name={r.n.name} /></button>
        {:else}
          <VisibleName name={r.n.name} />
        {/if}
        <span class="num size">{r.size}</span>
        <button type="button" class="inspect" aria-label={`Inspect ${escapedName(r.n.name)}`} onclick={() => oninspect(r.n.node_id)}>Inspect</button>
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
  .inspect { font: inherit; font-size: 0.78rem; color: var(--muted); background: none; border: 1px solid var(--control-line); border-radius: 999px; padding: 0 8px; margin-left: 6px; cursor: pointer; }
  .inspect:hover { color: var(--text); border-color: var(--line-strong); }
  .size { color: var(--muted); margin-left: 8px; }
  .notes { display: block; color: var(--faint); font-size: 0.8rem; }
  .rest { color: var(--faint); list-style: none; }
</style>
