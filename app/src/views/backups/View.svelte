<script module lang="ts">
  export const meta = { title: 'Backups', order: 65 };
</script>
<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { session } from '../../lib/stores/session.svelte';
  import { formatTime } from '../../lib/format/time';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import type { BackupEvidence, Volume } from '../../lib/contracts.gen';
  import { conditions, coverage, formatAge, IMPORT_REFUSAL_WORDS, LABEL, observations, parseRequired, SNAPSHOT, topology } from './model';

  const PROOF: Record<string, string> = {
    repository_and_snapshot_binding: 'the restore is bound to this repository and snapshot',
    independent_content_sha256: 'each restored file’s SHA-256 is measured independently',
    required_metadata_comparison: 'the listed metadata is compared file by file',
    complete_manifest: 'every listed file is restored',
    fresh_observation: 'the observation is fresh',
  };
  const METADATA: Record<string, string> = {
    acl: 'permissions (ACL)', alternate_streams: 'alternate streams', hard_links: 'hard links', timestamps: 'timestamps',
    sparse: 'sparse ranges', compression: 'compression', efs: 'encryption (EFS)',
  };

  let evidence = $state<BackupEvidence[]>([]);
  let volumes = $state<Volume[]>([]);
  let loading = $state(false);
  let error = $state('');
  let notice = $state('');
  let label = $state('');
  let snapshot = $state('');
  let source = $state('');
  let target = $state('');
  let wanted = $state('');
  let integrity = $state(true);
  let importing = $state(false);
  let forgetting = $state('');
  // The import field the shown form error belongs to: it is marked invalid and described by the message (WCAG 3.3.1).
  let invalidField = $state<'' | 'label' | 'snapshot' | 'receipt'>('');
  let heading = $state<HTMLElement>();
  let generation = 0;

  const desktop = $derived(session.info?.adapter === 'tauri');
  const personal = $derived(session.datasetClass === 'personal');
  const parsed = $derived(parseRequired(wanted));
  const formError = $derived(
    !LABEL.test(label) ? 'Use a label of 1–64 letters, digits, dots, dashes or underscores.'
      : !SNAPSHOT.test(snapshot) ? 'Paste the 64-digit snapshot ID from restic.'
      : parsed.errors[0] ?? '',
  );

  async function load(): Promise<void> {
    if (!session.client) return;
    const mine = ++generation;
    loading = true; error = ''; invalidField = '';
    try {
      const { result } = await session.client.call('backups.list', {});
      if (mine !== generation) return;
      evidence = result.evidence;
    } catch (e) {
      if (mine !== generation) return;
      error = session.handle(e); evidence = [];
    } finally { if (mine === generation) loading = false; }
    if (desktop && personal) {
      try {
        volumes = (await session.client.call('volumes.list', {})).result.volumes;
      } catch (e) {
        if (mine !== generation) return;
        error = session.handle(e);
        volumes = [];
      }
    }
  }

  async function importReceipt(): Promise<void> {
    if (!session.client || importing) return;
    notice = ''; error = ''; invalidField = '';
    if (formError) {
      error = formError;
      invalidField = !LABEL.test(label) ? 'label' : !SNAPSHOT.test(snapshot) ? 'snapshot' : 'receipt';
      return;
    }
    importing = true;
    try {
      // The dialog waits on the owner: Rust opens it, reads the file and keeps the path.
      const { result } = await session.client.call('backups.import', {
        repository_label: label, snapshot_id: snapshot, source_volume_id: source || null, target_volume_id: target || null,
        required: parsed.required, integrity_check: integrity,
      }, { deadlineMs: 60000 });
      if (result.outcome === 'cancelled_by_user') notice = 'Import cancelled. Nothing was stored.';
      else if (result.outcome === 'refused') error = result.refusal ? IMPORT_REFUSAL_WORDS[result.refusal] : 'The import was refused.';
      else { notice = 'Evidence imported. Only the normalised evidence was stored; the file was read, not kept.'; await load(); }
    } catch (e) { error = session.handle(e); }
    finally { importing = false; }
  }

  async function forget(e: BackupEvidence): Promise<void> {
    if (!session.client || forgetting) return;
    forgetting = e.evidence_id; notice = ''; error = ''; invalidField = '';
    try {
      await session.client.call('backups.forget', { evidence_id: e.evidence_id });
      await load();
      notice = `Forgot the evidence for ${e.repository_label}. No backup or file was changed.`;
      await tick(); heading?.focus();
    } catch (err) { error = session.handle(err); }
    finally { forgetting = ''; }
  }

  $effect(() => {
    session.epoch;
    untrack(() => void load());
    return () => { generation++; };
  });
</script>

<h1 tabindex="-1" bind:this={heading}>Backups</h1>
<p class="lede">Recovery evidence from restic receipts you import.</p>
<p class="note"><strong>A green backup job is not verified recovery.</strong> A snapshot, an integrity check and a restore drill are separate observations, and only a restore drill of the exact versions you need shows recovery. Loomward never runs a backup tool, opens a repository or restores anything.</p>
<p class="live muted" class:has={!!notice} role="status" aria-label="Backup result">{notice}</p>
<p id="backup-error" class="live bad" class:has={!!error} role="alert">{error}</p>

<section class="panel" aria-labelledby="backup-import">
  <h2 id="backup-import">Import a receipt</h2>
  {#if !desktop || !personal}
    <p class="muted">Importing needs the desktop app in a personal session: only it can open a file dialog that keeps the path in Rust. This session can read and forget stored evidence.</p>
  {:else}
    <p class="muted">Run <code>restic snapshots &lt;id&gt; --json</code> (and optionally <code>restic check --json</code>) yourself, save the output, then pick the files. Loomward reads at most 256 KiB of each, read only, and keeps only the normalised evidence.</p>
    <form class="controls form" onsubmit={(e) => { e.preventDefault(); void importReceipt(); }}>
      <label class="lw-field"><span class="lbl">Repository label</span> <input type="text" bind:value={label} maxlength="64" autocomplete="off" aria-invalid={invalidField === 'label' ? 'true' : undefined} aria-describedby={invalidField === 'label' ? 'backup-error' : undefined} /></label>
      <label class="lw-field wide"><span class="lbl">Snapshot ID</span> <input type="text" bind:value={snapshot} maxlength="64" autocomplete="off" spellcheck="false" aria-invalid={invalidField === 'snapshot' ? 'true' : undefined} aria-describedby={invalidField === 'snapshot' ? 'backup-error' : undefined} /></label>
      <label class="lw-field"><span class="lbl">Source volume</span> <select bind:value={source}>
        <option value="">Not declared</option>
        {#each volumes as v (v.volume_id)}<option value={v.volume_id}>{v.display_name}</option>{/each}
      </select></label>
      <label class="lw-field"><span class="lbl">Backup volume</span> <select bind:value={target}>
        <option value="">Not declared (remote or unknown)</option>
        {#each volumes as v (v.volume_id)}<option value={v.volume_id}>{v.display_name}</option>{/each}
      </select></label>
      <label class="lw-field wide"><span class="lbl">Files you need back: one per line, relative name then SHA-256</span>
        <textarea bind:value={wanted} rows="3" spellcheck="false" aria-invalid={invalidField === 'receipt' ? 'true' : undefined} aria-describedby={invalidField === 'receipt' ? 'backup-error' : undefined}></textarea></label>
      <label class="check"><input type="checkbox" bind:checked={integrity} /> Also pick a <code>check --json</code> output</label>
      <button class="btn primary" type="submit" aria-disabled={importing} onclick={(e) => { if (importing) e.preventDefault(); }}>{importing ? 'Waiting for the file dialog' : 'Choose receipt file'}</button>
    </form>
  {/if}
</section>

{#each evidence as e (e.evidence_id)}
  {@const topo = topology(e)}
  <section class="panel" aria-labelledby={`repo-${e.evidence_id}`}>
    <div class="head">
      <h2 id={`repo-${e.evidence_id}`}>{e.repository_label}</h2>
      <span class="tag" class:ok={e.reported_recovery_evidence} class:warn={!e.reported_recovery_evidence}>{e.reported_recovery_evidence ? 'Reported evidence complete, not native proof' : 'Recovery not shown'}</span>
    </div>
    <p class="muted">Snapshot <code>{e.snapshot_id.slice(0, 12)}…</code> · imported {formatTime(e.imported_at)}</p>
    <div class="tbl-wrap">
      <table class="tbl">
        <caption class="sr-only">Recovery evidence for {e.repository_label}</caption>
        <thead><tr><th scope="col">Observation</th><th scope="col">Status</th><th scope="col">Observed</th><th scope="col">Why it does not show recovery</th></tr></thead>
        <tbody>
          {#each observations(e) as o (o.name)}
            <tr><th scope="row">{o.name}</th>
              <td>{#if o.good}{o.status}{:else}<span class="tag warn">{o.status}</span>{/if}</td>
              <td>{#if o.name === 'Snapshot taken'}{formatTime(e.snapshot_observed_at)}, {formatAge(e.snapshot_age_seconds)} ago
                {:else if o.name === 'Integrity checked'}{#if e.integrity_observed_at}{formatTime(e.integrity_observed_at)}, {formatAge(e.integrity_age_seconds)} ago{:else}<span class="unknown">never</span>{/if}
                {:else}<span class="unknown">never</span>{/if}</td>
              <td>{#if o.why.length}<ul class="why">{#each o.why as w (w)}<li>{w}</li>{/each}</ul>{:else}<span class="muted">None</span>{/if}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    <dl class="facts">
      <div><dt>Restore coverage</dt><dd>{coverage(e)}</dd></div>
      <div><dt>Topology</dt><dd>{topo.label}. <span class="muted">{topo.basis}</span></dd></div>
      <div><dt>Freshness window</dt><dd>{formatAge(e.max_age_seconds)}</dd></div>
      {#if conditions(e).length}<div><dt>Other conditions</dt><dd><ul class="why">{#each conditions(e) as c (c)}<li>{c}</li>{/each}</ul></dd></div>{/if}
    </dl>
    <details class="drill">
      <summary>Proposed restore drill (data only)</summary>
      <p class="muted">Restore these versions to a new disposable folder, never over the source, then compare {e.proposed_drill.compare_metadata.map((m) => METADATA[m] ?? m).join(' and ') || 'content'}. Loomward does not run this drill.</p>
      <ul>{#each e.proposed_drill.files as f (f.relative_name)}<li><VisibleName name={f.relative_name} /> <code class="small">{f.version.slice(7, 19)}…</code></li>{/each}</ul>
      <p class="muted">It shows recovery only when {e.proposed_drill.proofs.map((p) => PROOF[p] ?? p).join('; ')}.</p>
    </details>
    <div class="foot">
      <button class="btn" type="button" aria-label={`Forget the evidence for ${e.repository_label}`} aria-disabled={!!forgetting} onclick={() => void forget(e)}>Forget record</button>
      <span class="muted">Removes Loomward’s record only.</span>
    </div>
  </section>
{:else}
  <section class="panel"><p class="empty">{loading ? 'Loading evidence' : error ? 'Evidence unavailable.' : 'No backup evidence imported yet.'}</p></section>
{/each}

<style>
  .form { flex-wrap: wrap; align-items: end; }
  .wide { flex: 1 1 22rem; }
  input, select, textarea { max-width: 100%; }
  textarea { width: 100%; font-family: var(--font-mono); }
  .check { display: inline-flex; gap: var(--sp-2); align-items: center; min-height: 24px; }
  .head { display: flex; flex-wrap: wrap; gap: var(--sp-3); align-items: baseline; justify-content: space-between; }
  .why { margin: 0; padding-left: 1.1em; }
  .facts { display: grid; grid-template-columns: repeat(auto-fit, minmax(14rem, 1fr)); gap: var(--sp-3); margin: var(--sp-3) 0; }
  .facts dt { color: var(--muted); font-size: 0.85em; }
  .facts dd { margin: 0; }
  .drill summary { cursor: pointer; min-height: 24px; }
  .foot { display: flex; flex-wrap: wrap; align-items: center; gap: var(--sp-3); margin-top: var(--sp-3); }
  /* the heading takes focus after Forget record: keyboard users get the focus ring (2.4.7) */
  h1:focus:not(:focus-visible) { outline: none; }
</style>
