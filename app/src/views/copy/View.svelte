<script module lang="ts">
  export const meta = { title: 'Copy & verify', order: 35 };
</script>

<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { copy, refusalReason, type CopyItem } from '../../lib/stores/copy.svelte';
  import { session } from '../../lib/stores/session.svelte';
  import { TransportError } from '../../lib/transport/client';
  import { formatBytes } from '../../lib/format/bytes';
  import { formatCount, formatTime } from '../../lib/format/time';
  import { escapedName } from '../../lib/format/names';
  import VisibleName from '../../lib/ui/VisibleName.svelte';

  let heading = $state<HTMLHeadingElement>();
  let selectionHeading = $state<HTMLHeadingElement>();
  let destinationHeading = $state<HTMLHeadingElement>();
  let roots = $state<CopyItem[]>([]);
  let trail = $state<CopyItem[]>([]);
  let folders = $state<CopyItem[]>([]);
  let cursor = $state<string | null>(null);
  let browsing = $state(false);
  let browseProblem = $state('');
  let budgetHit = $state(false);
  let now = $state(Date.now());
  let polling = $state(false);
  let browseGeneration = 0;
  const here = $derived(trail.at(-1));
  const nativeConsent = $derived(session.datasetClass === 'synthetic' || session.mode === 'tauri');
  const expired = $derived(!!copy.approval && Date.parse(copy.approval.expires_at) <= now);
  const resuming = $derived(!!copy.status && ['blocked', 'needs_attention'].includes(copy.status.state));
  const phase = $derived(copy.uncertain ? 'Outcome unknown' : copy.status?.state.replaceAll('_', ' ') ?? (copy.gated ? 'Copying is not enabled yet' : copy.approval ? 'Approval ready' : copy.preview ? 'Review preview' : 'Select destination'));
  const nodeName = (id: string | null) => id === null ? 'Whole operation' : copy.items.find(i => i.node_id === id)?.name ?? 'An entry inside the selection';

  async function deselect(nodeId: string): Promise<void> {
    copy.select(copy.items.filter(i => i.node_id !== nodeId));
    await tick(); selectionHeading?.focus();
  }

  async function act(action: 'review' | 'approve' | 'start' | 'refresh' | 'cancel' | 'abandon', focus = true): Promise<void> {
    if (!session.client || (action === 'approve' && !nativeConsent)) return;
    if (!focus) polling = true;
    await copy[action](session.client);
    polling = false;
    if (copy.error instanceof TransportError) session.handle(copy.error);
    if (focus) { await tick(); heading?.focus(); }
  }
  async function browse(nextTrail: CopyItem[], append = false): Promise<void> {
    const current = nextTrail.at(-1);
    if (!current || !session.client) return;
    const mine = ++browseGeneration;
    browsing = true; browseProblem = '';
    if (!append) { trail = nextTrail; folders = []; cursor = null; budgetHit = false; }
    try {
      const { result } = await session.client.call('tree.children', { node_id: current.node_id, sort: 'size_desc', basis: 'logical', limit: 100, cursor: append ? cursor : null });
      if (mine !== browseGeneration) return;
      const dirs = result.items.filter(i => i.kind === 'dir').map(i => ({ node_id: i.node_id, name: i.name }));
      folders = append ? [...folders, ...dirs] : dirs;
      cursor = result.next_cursor; budgetHit = result.budget_hit;
      if (!append) { await tick(); destinationHeading?.focus(); }
    } catch (e) { if (mine === browseGeneration) browseProblem = session.handle(e); }
    finally { if (mine === browseGeneration) browsing = false; }
  }
  onMount(() => {
    let disposed = false;
    void session.client!.call('tree.slice', { anchor: { kind: 'atlas' }, depth: 2, max_nodes: 256, min_share: 0, basis: 'logical', include_files: false }).then(({ result }) => {
      if (!disposed) roots = result.nodes.filter(n => n.kind === 'root').map(n => ({ node_id: n.node_id, name: n.name }));
    }).catch(e => { if (!disposed) browseProblem = session.handle(e); });
    // Status is read-only. An interrupted or uncertain operation never starts or resumes here.
    const timer = setInterval(() => {
      now = Date.now();
      if (copy.status && ['preparing', 'copying', 'staged', 'publishing', 'cancelling'].includes(copy.status.state) && !copy.busy && !copy.problem) void act('refresh', false);
    }, 1500);
    return () => { disposed = true; browseGeneration++; clearInterval(timer); };
  });
  $effect(() => {
    copy.observe(session.epoch);
  });
</script>

<h1>Copy &amp; verify</h1>
<p class="lede">One new folder. Verified bytes. Your source stays where it is.</p>
{#if !session.info?.capabilities.effects.file_write}
  <section class="panel" aria-label="Copy capability">
    <h2>Copying is not enabled yet</h2>
    <p>The copy effect safety gate is closed. Personal sessions and the mock transport refuse every copy command with <code>capability_unavailable</code> / <code>copy_effect_gated</code>. Selecting items grants no permission to write.</p>
  </section>
{/if}
<section class="panel" aria-label="Copy selection">
  <h2 tabindex="-1" bind:this={selectionHeading}>Selected items <span class="muted num">{formatCount(copy.items.length)}</span></h2>
  {#if copy.items.length}
    <ul class="selection">
      {#each copy.items as item (item.node_id)}
        <li><VisibleName name={item.name} /><button class="btn" type="button" disabled={copy.locked || copy.busy}
          aria-label={`Deselect ${escapedName(item.name)}`} onclick={() => void deselect(item.node_id)}>Deselect</button></li>
      {/each}
    </ul>
  {:else}<p>Select items in <a href="#/explorer">Explorer</a> or select a file or folder in <a href="#/atlas">Atlas</a>, then choose Copy to…</p>{/if}
  <p>No space is freed on the source. This is not a move, sync or backup.</p>
</section>

{#if !copy.locked}
  <section class="panel" aria-label="Destination parent">
    <h2 tabindex="-1" bind:this={destinationHeading}>Destination parent</h2>
    <p class="muted">Choose a catalogued folder. A native folder picker is not available in the current copy contract; no path is sent from this page. The engine must validate the destination.</p>
    {#if copy.parent}<p>Selected parent: <strong><VisibleName name={copy.parent.name} /></strong></p>{:else}<p class="unknown">No destination selected.</p>{/if}
    <div class="actions" role="group" aria-label="Destination roots">
      {#each roots as root (root.node_id)}<button class="btn" type="button" disabled={browsing || copy.busy} aria-label={`Browse destination root ${escapedName(root.name)}`} onclick={() => void browse([root])}><VisibleName name={root.name} /></button>{/each}
    </div>
    {#if !roots.length && !browseProblem}<p class="muted">No destination roots are shown. Only catalogued folders can be selected here.</p>{/if}
    {#if here}
      <nav class="actions" aria-label="Destination trail">{#each trail as part, i (part.node_id)}<button class="btn" type="button" disabled={browsing || copy.busy} aria-current={i === trail.length - 1 ? 'location' : undefined} onclick={() => void browse(trail.slice(0, i + 1))}><VisibleName name={part.name} /></button>{/each}</nav>
      <button class="btn primary" type="button" disabled={copy.busy || browsing || !!browseProblem} onclick={() => copy.destination(here!)}>Use this folder as parent</button>
      <ul class="folders" aria-label="Destination folders" aria-busy={browsing}>
        {#each folders as folder (folder.node_id)}<li><button class="btn" type="button" disabled={browsing || copy.busy} aria-label={`Browse destination folder ${escapedName(folder.name)}`} onclick={() => void browse([...trail, folder])}><VisibleName name={folder.name} /></button></li>{/each}
      </ul>
      {#if cursor}<button class="btn" type="button" disabled={browsing || copy.busy} onclick={() => void browse(trail, true)}>Load more destination entries</button>{/if}
      {#if budgetHit}<p class="unknown">The listing reached its budget; more folders may exist.</p>{/if}
    {/if}
    {#if browseProblem}<p class="bad" role="alert">{browseProblem}</p>{/if}
    <div class="actions"><button class="btn primary" type="button" disabled={copy.busy || !copy.items.length || !copy.parent} onclick={() => void act('review')}>Build preview</button></div>
  </section>
{/if}

<section class="panel" aria-label="Copy operation" aria-busy={copy.busy && !polling}>
  <h2 tabindex="-1" bind:this={heading}>{phase}</h2>
  <p role="status" aria-live="polite">{copy.busy && !polling ? 'Waiting for the engine…' : copy.notice}</p>
  {#if copy.problem}<p class="bad" role="alert">{copy.problem}</p>{/if}
  {#if copy.uncertain}<p class="unknown">The operation outcome is unknown. Refresh status before taking another action; it will not be started again automatically.</p>{/if}
  {#if copy.preview}
    {@const p = copy.preview}
    <dl class="facts">
      <div><dt>Preview state</dt><dd>{p.state.replaceAll('_', ' ')}</dd></div>
      <div><dt>Manifest</dt><dd>{formatCount(p.files)} files · {formatCount(p.folders)} folders · {formatBytes(p.bytes)}</dd></div>
      <div><dt>Destination parent</dt><dd><VisibleName name={copy.parent?.name ?? 'unknown'} /></dd></div>
      <div><dt>One new folder</dt><dd><VisibleName name={p.folder_name} /> · no existing entry replaced</dd></div>
      <div><dt>Disk</dt><dd>{p.same_volume ? 'Same disk: not protection against disk failure' : 'Different disk'}</dd></div>
      <div><dt>Space needed / reserve</dt><dd>{formatBytes(p.space_needed)} / {formatBytes(p.space_reserve)}</dd></div>
      <div><dt>Free space before / after</dt><dd class="unknown">unknown / unknown — not supplied by the copy contract</dd></div>
      <div><dt>Digest</dt><dd class="digest">{p.digest} (confirmation suffix: {p.digest.slice(-8)})</dd></div>
    </dl>
    <h3>Metadata policy</h3>
    <p>Preserve and verify: default and supported named data streams, size, creation and last-write times; read-only, hidden, archive and not-content-indexed attributes; folder times and attributes.</p>
    <p>Permissions come from the destination; {formatCount(p.explicit_acl_items)} items had their own. Owner becomes the current user. {formatCount(p.hard_link_names)} hard-link names become independent files.</p>
    <p class="muted">Last-access time is best effort, not verified. Change time, folder case sensitivity, object IDs and short names are not preserved. System, temporary, compressed, sparse, encrypted and integrity-stream items, extended attributes, reparse points and unsupported named streams are refused.</p>
    <h3>Refusals <span class="num">{formatCount(p.refusal_count)}</span></h3>
    {#each p.refusals as refusal, i (i)}<p class="bad"><VisibleName name={nodeName(refusal.node_id)} /> · <code>{refusal.code}</code>: {refusalReason(refusal.code)}</p>{/each}
    {#if p.refusal_count > p.refusals.length}<p class="unknown">Additional refusal details were not supplied. Approval is blocked.</p>{:else if !p.refusal_count && !p.refusals.length}<p>No refusals reported in this preview.</p>{/if}
    {#if copy.canApprove}
      <p>{resuming ? 'Resolve the reported cause outside Loomward before confirming Resume.' : 'Review this manifest, then explicitly approve this digest.'} {session.datasetClass === 'synthetic' ? 'Synthetic session: approval is requested over the session transport.' : 'Personal sessions require the Rust-owned confirmation, rendered from the stored manifest.'}</p>
      {#if !nativeConsent}<p class="unknown">Personal approval is unavailable over HTTP. Open the desktop app for native confirmation when the copy gate opens.</p>{/if}
      <button class="btn primary" type="button" disabled={!nativeConsent} onclick={() => void act('approve')}>{resuming ? 'Confirm resume' : 'Approve this preview'}</button>
    {/if}
    {#if copy.approval}
      <p role="status">{expired ? 'Approval expired. Build a new preview.' : `Approval expires ${formatTime(copy.approval.expires_at)}. Changing the preview invalidates it.`}</p>
      <button class="btn primary" type="button" disabled={!copy.canStart || expired} onclick={() => void act('start')}>{resuming ? 'Resume copying' : 'Start copying'}</button>
    {/if}
    {#if copy.status}
      <h3>{copy.status.state === 'committed' ? 'Receipt: completed and verified' : copy.status.state === 'cancelled' ? 'Receipt: cancelled' : 'Operation status'}</h3>
      <p role="status">{copy.status.state.replaceAll('_', ' ')} · {formatCount(copy.status.items_verified)} of {formatCount(copy.status.items_total)} items verified</p>
      {#if copy.status.refusal}<p class="bad"><VisibleName name={nodeName(copy.status.refusal.node_id)} /> · <code>{copy.status.refusal.code}</code>: {refusalReason(copy.status.refusal.code)}</p>{/if}
      {#if copy.status.state === 'committed'}<p>No space was freed on the source. Read-back does not prove persistence beyond the device’s own cache.</p>
      {:else if copy.status.state === 'cancelled'}<p>The source was not changed. No space was freed on the source.</p>
      {:else if ['blocked', 'needs_attention'].includes(copy.status.state)}<p>Not completed and verified. Sources stay untouched; staged or published entries may remain. Nothing resumes automatically.</p>{/if}
      {#if copy.status.staging_folder}
        <p class="unknown">The staging folder <VisibleName name={copy.status.staging_folder} /> was left in place in <VisibleName name={copy.parent?.name ?? 'the destination parent'} />; remove it in Explorer if you no longer need it (Loomward does not delete it until you allow it).</p>
      {/if}
      {#if copy.status.leftover_staging_folders?.length}
        <p class="unknown">Loomward intended to create these staging folders but never recorded them, so they may be empty leftovers. Loomward does not adopt or remove them; look for them in Explorer under the destination parent.</p>
        <ul class="folders" aria-label="Leftover staging folder names">{#each (copy.status.leftover_staging_folders ?? []) as name, i (i)}<li><VisibleName name={name} /></li>{/each}</ul>
      {/if}
      <p class="muted">Byte progress and transfer rate: unknown — the status contract reports verified item counts only.</p>
    {/if}
    <div class="actions">
      <button class="btn" type="button" aria-disabled={copy.busy} onclick={() => { if (!copy.busy) void act('refresh'); }}>Refresh copy status</button>
      {#if copy.canCancel}<button class="btn" type="button" aria-disabled={copy.busy} onclick={() => { if (!copy.busy) void act('cancel'); }}>Cancel copy</button>{/if}
      {#if copy.canAbandon}<button class="btn" type="button" onclick={() => void act('abandon')}>Abandon operation</button>{/if}
    </div>
  {/if}
  <p class="muted">This tab retains its current operation while navigating views. After a reload, interrupted-operation discovery is unavailable: the current contract has no operation-list command.</p>
</section>

<style>
  .selection { padding-left: 20px; }
  .selection li { padding-block: 4px; overflow-wrap: anywhere; }
  .selection .btn { margin-left: 12px; }
  .actions { display: flex; flex-wrap: wrap; gap: 8px; margin-block: 12px; }
  .folders { padding-left: 20px; }
  .folders li { margin-block: 8px; }
  .facts { display: grid; gap: 8px; }
  .facts > div { display: grid; grid-template-columns: minmax(140px, 1fr) 3fr; gap: 12px; }
  dt { color: var(--muted); }
  dd { margin: 0; overflow-wrap: anywhere; }
  .digest { font-family: var(--num-font); }
  @media (max-width: 600px) { .facts > div { grid-template-columns: 1fr; gap: 2px; } }
</style>
