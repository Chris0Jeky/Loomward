<script lang="ts">
  import type { EntryRow, Root } from '../../lib/contracts.gen';
  import { formatBytes, parseBytes } from '../../lib/format/bytes';
  import { formatCount, formatTime } from '../../lib/format/time';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import type { Footprint } from './footprint';

  let { data, root, generation }: { data: Footprint; root: Root | undefined; generation: string } = $props();
  const measured = (row: EntryRow) => row.coverage === 'complete' || row.coverage === 'partial';
  const bytes = (value: string | null) => parseBytes(value) === null ? 'unknown' : `${formatBytes(value)} (${value} bytes)`;
  let row = $derived(data.detail.row);
  let known = $derived(measured(row));
  let partial = $derived(row.coverage !== 'complete' || data.detail.subtree?.complete === false || !data.families.complete || data.children.budget_hit);
</script>

<section class="footprint" aria-label="Recorded project footprint">
  <h4>Recorded storage footprint</h4>
  <p>Coverage: <strong>{row.coverage}</strong>. Recorded scan finished: {formatTime(root?.scan.generation === generation ? root.scan.finished_at : null)}.</p>
  {#if partial}<p class="note warn">Incomplete observations. Recorded totals may omit files; an empty listing does not prove an empty folder.</p>{/if}
  <dl>
    <dt>Logical size</dt><dd>{bytes(known ? row.logical_bytes : null)}</dd>
    <dt>Allocated size</dt><dd>{bytes(known ? row.allocated_bytes : null)}</dd>
    <dt>Files</dt><dd>{formatCount(known ? row.files : null)}</dd>
    <dt>Folders</dt><dd>{formatCount(known ? row.dirs : null)}</dd>
  </dl>
  <p class="muted">Logical size is recorded file length; allocated size is recorded disk allocation. Totals count names, so hard links can count the same object again. Alternate streams and filesystem metadata are not included.</p>

  {#if known}
    <h4>Recorded file families</h4>
    <p class="muted">By logical bytes, largest recorded families first. These are file-type groups, not project purpose or deletion advice.</p>
    {#if !data.families.complete}<p class="note warn">Family coverage is incomplete; these groups may omit files.</p>{/if}
    <ul class="facts">
      {#each data.families.buckets as bucket (bucket.key)}
        <li><span><VisibleName name={bucket.label} /> · {formatCount(bucket.files)} files</span><span class="num">{bytes(bucket.bytes)}</span></li>
      {/each}
      {#if data.families.other.files > 0}<li><span>Other recorded families · {formatCount(data.families.other.files)} files</span><span class="num">{bytes(data.families.other.bytes)}</span></li>{/if}
      {#if data.families.unknown.files > 0}<li><span>Unknown family · {formatCount(data.families.unknown.files)} files</span><span class="num">{bytes(data.families.unknown.bytes)}</span></li>{/if}
    </ul>
    {#if !data.families.buckets.length && !data.families.other.files && !data.families.unknown.files}<p>No family groups returned{data.families.complete ? '.' : ' from the available observations.'}</p>{/if}

    <h4>Recorded direct children</h4>
    <p class="muted">Up to 12 entries in catalogue order by recorded logical size, descending. Unknown or stale sizes do not establish a global ranking.</p>
    <ol class="facts">
      {#each data.children.items.slice(0, 12) as child (child.node_id)}
        <li><span><VisibleName name={child.name} /> <small>({child.kind === 'dir' ? 'folder' : 'file'} · {child.coverage})</small></span><span class="num">{bytes(measured(child) ? child.logical_bytes : null)}</span></li>
      {/each}
    </ol>
    {#if data.children.next_cursor || data.children.budget_hit}<p class="note">More entries may exist; this brief shows only the returned first page. Inspect in Explorer to continue.</p>{/if}
    {#if !data.children.items.length}<p>No direct children returned{partial ? ' from the available observations; this is not proof of an empty folder.' : '.'}</p>{/if}
  {:else}<p class="note">Sizes, counts, families and child listings are unknown for this coverage state. Find projects again after the catalogue becomes available.</p>{/if}

  <p class="muted">Git cleanliness, recent human use, regeneration cost and safe deletion: unknown. Manifest names are clues only. This reads recorded metadata; it does not inspect live files.</p>
</section>
<style>
  .footprint { margin-block: var(--sp-3); }
  h4 { margin-block: var(--sp-3) var(--sp-1); }
  dl { display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: var(--sp-1) var(--sp-3); }
  dd { margin: 0; overflow-wrap: anywhere; font-variant-numeric: tabular-nums; }
  .facts { padding-inline-start: 1.5rem; }
  .facts li { padding-block: var(--sp-1); overflow-wrap: anywhere; }
  .facts li > span { display: block; }
  @media (min-width: 48rem) { .facts li > span { display: inline; } .facts .num { margin-inline-start: var(--sp-3); } }
</style>
