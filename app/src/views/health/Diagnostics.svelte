<script lang="ts">
  import { untrack } from 'svelte';
  import { session } from '../../lib/stores/session.svelte';
  import { formatCount } from '../../lib/format/time';
  import { diagnosticsPanel as p } from './panel.svelte';

  /** The save dialog exists only where the service says so: the desktop app. */
  const canSave = $derived(session.info?.features.diagnostics_save === true);
  const saveNote = $derived(!session.info || canSave ? '' : session.info.adapter === 'http' ? 'Saving needs the desktop app, which opens its own save dialog.' : 'This session has no save dialog.');

  const reload = () => p.refresh(session.client, (e) => session.handle(e));

  // What the panel shows now has been shown: the next reload may clear it. (Declared before the reload effect.)
  $effect(() => { if (p.notice || p.problem) p.seen = true; });
  // Every scan invalidation bumps the epoch; the panel coalesces them into one running call plus one more.
  $effect(() => {
    void session.epoch;
    untrack(() => void reload());
  });
</script>

<section class="panel" aria-labelledby="h-diag">
  <div class="head">
    <h2 id="h-diag">Diagnostic bundle</h2>
    <button class="btn" type="button" aria-disabled={p.loading || p.saves.frozen ? 'true' : undefined} onclick={() => void reload()}>Preview again</button>
  </div>
  <p class="muted">A redacted summary for a bug report: versions, capabilities, health, counts, and each root by its opaque ID with its scan state and filesystem. It never holds a path, a file or folder name, a volume label, a process, a label or a collection name. Anything Loomward could not read says unknown.</p>
  <p class="promise">Nothing is uploaded. Saving writes exactly what is shown to a file you choose.</p>
  <p class="live muted" class:has={!!p.notice} role="status" aria-label="Diagnostic bundle result">{p.notice}</p>
  <p class="live fail" class:has={!!p.problem} role="alert">{p.problem}</p>
  {#if p.preview}
    <p class="muted small num">{formatCount(Number(p.preview.bytes))} bytes · <code class="digest">{p.preview.digest}</code>
      · {p.verified === null ? 'checking' : p.verified ? 'the text below is exactly these bytes' : 'the text below does not match these bytes'}</p>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex (keyboard access to scrolling) -->
    <pre class="bundle" tabindex="0" role="region" aria-label="Diagnostic bundle preview">{p.text}</pre>
    {#if canSave}
      <button class="btn primary" type="button" aria-disabled={!p.verified || p.saves.running ? 'true' : undefined} onclick={() => void p.save(session.client, (e) => session.handle(e))}>{p.saves.running ? 'Saving' : 'Save…'}</button>
      {#if p.verified === false}<p class="fail small">Saving is off because the preview could not be checked. Preview again.</p>{/if}
    {:else if saveNote}<p class="muted small">{saveNote}</p>{/if}
  {:else if p.loading}<p class="muted">Loading</p>{/if}
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
