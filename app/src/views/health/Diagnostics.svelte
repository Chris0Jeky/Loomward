<script lang="ts">
  import { untrack } from 'svelte';
  import { session } from '../../lib/stores/session.svelte';
  import { formatCount } from '../../lib/format/time';
  import type { DiagnosticsPreviewResult } from '../../lib/contracts.gen';
  import { canonicalText, matches } from './diagnostics';
  import { SaveTracker } from './save.svelte';

  let preview = $state<DiagnosticsPreviewResult | null>(null);
  let text = $state('');
  /** null while checking; false when the shown text is not exactly the bytes the service would write. */
  let verified = $state<boolean | null>(null);
  let loading = $state(false);
  const saves = new SaveTracker();
  let notice = $state('');
  let problem = $state('');
  let generation = 0;
  /** A reload asked for while a save is running or pending waits for its outcome: the text shown stays the text being saved. */
  let reloadAfterSave = false;

  /** The save dialog exists only where the service says so: the desktop app. */
  const canSave = $derived(session.info?.features.diagnostics_save === true);
  const saveNote = $derived(!session.info || canSave ? '' : session.info.adapter === 'http' ? 'Saving needs the desktop app, which opens its own save dialog.' : 'This session has no save dialog.');

  async function load(): Promise<void> {
    const c = session.client;
    if (!c) return;
    if (saves.frozen) { reloadAfterSave = true; return; }
    const mine = ++generation;
    loading = true; problem = ''; verified = null;
    try {
      const { result } = await c.call('diagnostics.preview', {});
      if (mine !== generation) return;
      const shown = canonicalText(result.bundle);
      const same = await matches(shown, result);
      if (mine !== generation) return;
      if (preview && result.digest !== preview.digest) notice = ''; // an earlier save's notice described other text
      preview = result; text = shown; verified = same;
    } catch (e) {
      if (mine === generation) { problem = session.handle(e); preview = null; }
    } finally { if (mine === generation) loading = false; }
  }

  async function save(): Promise<void> {
    const c = session.client;
    if (!c || !preview || !verified || saves.running) return;
    notice = ''; problem = '';
    const report = await saves.save(c, preview.digest, (e) => session.handle(e));
    if (report?.kind === 'notice') notice = report.text;
    else if (report) problem = report.text;
    if (!saves.frozen && reloadAfterSave) { reloadAfterSave = false; void load(); }
  }

  $effect(() => {
    void session.epoch;
    untrack(() => void load());
    return () => { generation++; };
  });
</script>

<section class="panel" aria-labelledby="h-diag">
  <div class="head">
    <h2 id="h-diag">Diagnostic bundle</h2>
    <button class="btn" type="button" aria-disabled={loading || saves.frozen ? 'true' : undefined} onclick={() => void load()}>Preview again</button>
  </div>
  <p class="muted">A redacted summary for a bug report: versions, capabilities, health, counts, and each root by its opaque ID with its scan state and filesystem. It never holds a path, a file or folder name, a volume label, a process, a label or a collection name. Anything Loomward could not read says unknown.</p>
  <p class="promise">Nothing is uploaded. Saving writes exactly what is shown to a file you choose.</p>
  <p class="live muted" class:has={!!notice} role="status" aria-label="Diagnostic bundle result">{notice}</p>
  <p class="live fail" class:has={!!problem} role="alert">{problem}</p>
  {#if preview}
    <p class="muted small num">{formatCount(Number(preview.bytes))} bytes · <code class="digest">{preview.digest}</code>
      · {verified === null ? 'checking' : verified ? 'the text below is exactly these bytes' : 'the text below does not match these bytes'}</p>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex (keyboard access to scrolling) -->
    <pre class="bundle" tabindex="0" role="region" aria-label="Diagnostic bundle preview">{text}</pre>
    {#if canSave}
      <button class="btn primary" type="button" aria-disabled={!verified || saves.running ? 'true' : undefined} onclick={() => void save()}>{saves.running ? 'Saving' : 'Save…'}</button>
      {#if verified === false}<p class="fail small">Saving is off because the preview could not be checked. Preview again.</p>{/if}
    {:else if saveNote}<p class="muted small">{saveNote}</p>{/if}
  {:else if loading}<p class="muted">Loading</p>{/if}
</section>

<style>
  .head { display: flex; justify-content: space-between; align-items: center; gap: 12px; }
  .small { font-size: 0.8rem; }
  /* the .note look, under its own name: a view keeps one .note, its page-level promise */
  .promise { max-width: var(--measure); margin: 0 0 var(--sp-4); padding: var(--sp-2) var(--sp-3); border: 1px dashed var(--permission); border-radius: var(--radius); background: var(--bg); }
  .fail { color: var(--danger); }
  .digest { overflow-wrap: anywhere; }
  .bundle { max-height: 22rem; overflow: auto; margin: 0 0 12px; padding: var(--sp-2) var(--sp-3); border: 1px solid var(--line); border-radius: var(--radius); background: var(--bg); font-size: 0.8rem; line-height: 1.4; white-space: pre; }
  .bundle:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
</style>
