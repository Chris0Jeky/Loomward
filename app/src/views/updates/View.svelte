<script module lang="ts">
  export const meta = { title: 'Updates', order: 90 };
</script>

<script lang="ts">
  import { session } from '../../lib/stores/session.svelte';
  import { updates, type Channel } from '../../lib/stores/updates.svelte';

  // Bind to the desktop's IPC once; a browser session has no updater. Loading reads settings only.
  $effect(() => {
    const tauri = session.mode === 'tauri';
    let current = true;
    void (async () => {
      const invoke = tauri ? (await import('@tauri-apps/api/core')).invoke : null;
      if (!current) return;
      updates.use(invoke as never);
      if (invoke) await updates.load();
    })();
    return () => { current = false; };
  });

  const s = $derived(updates.status);
  const off = $derived(!s || !!s.unavailable || !!updates.busy);
</script>

<h1>Updates</h1>
<p class="lede">Loomward looks for a new build only when you press Check, or at start if you turn that on below. Nothing is installed without your OK in a Windows confirmation box.</p>

{#if !updates.desktop}
  <p class="empty">Updates are handled by the Loomward desktop app. This browser session has no updater.</p>
{:else if !s}
  <p class="empty">{updates.message || 'Reading the update settings…'}</p>
{:else}
  <section class="panel" aria-labelledby="updates-settings">
    <h2 id="updates-settings">Settings</h2>
    <p>Running <strong class="num">{s.current_version}</strong>.</p>
    {#if s.unavailable}<p class="note gated">{s.unavailable.charAt(0).toUpperCase() + s.unavailable.slice(1)}.</p>{/if}
    <div class="controls">
      <div class="lw-field">
        <label class="lbl" for="updates-channel">Channel</label>
        <select id="updates-channel" value={s.prefs.channel} disabled={!!updates.busy} onchange={(e) => updates.setChannel(e.currentTarget.value as Channel)}>
          <option value="preview">Preview (tested releases)</option>
          <option value="nightly">Nightly (every merge, untested)</option>
        </select>
      </div>
      <label class="lw-field toggle">
        <span class="lbl">At start</span>
        <span><input type="checkbox" checked={s.prefs.check_on_start} disabled={!!updates.busy} onchange={(e) => updates.setCheckOnStart(e.currentTarget.checked)} /> Check for updates when Loomward starts</span>
      </label>
    </div>
    {#if s.prefs.channel === 'nightly'}<p class="note warn">Nightly builds are built from every merge and are not tested by hand. They may break. Your state stays where it is, and a build that changes its schema copies <code>state.db</code> first.</p>{/if}
    <div class="controls">
      <button class="btn primary" disabled={off} onclick={() => updates.check(false)}>Check for updates</button>
      {#if s.prefs.channel === 'nightly'}<button class="btn" disabled={off} onclick={() => updates.check(true)}>Find the previous nightly</button>{/if}
    </div>
    <p class="live" class:has={!!updates.message} role="status" aria-live="polite">{updates.message}</p>
  </section>

  {#if s.offer}
    <section class="panel" aria-labelledby="updates-offer">
      <h2 id="updates-offer">{s.offer.previous ? 'Previous nightly' : 'Available update'}</h2>
      <p>Version <strong class="num">{s.offer.version}</strong> · {s.offer.channel} channel{#if s.offer.date} · published {s.offer.date}{/if}</p>
      {#if s.offer.notes}<pre class="notes">{s.offer.notes}</pre>{/if}
      <div class="controls">
        <button class="btn primary" disabled={!!updates.busy} onclick={() => updates.install()}>{s.offer.previous ? 'Reinstall this nightly…' : 'Install…'}</button>
      </div>
    </section>
  {/if}
{/if}

<style>
  .toggle span:last-child { display: flex; align-items: center; gap: var(--sp-2); }
  .notes { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 16rem; overflow: auto; font: inherit; }
</style>
