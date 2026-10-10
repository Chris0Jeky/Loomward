<script lang="ts">
  import { session } from './lib/stores/session.svelte';
  import { route } from './lib/stores/route.svelte';
  import { theme, THEMES, type Theme } from './lib/stores/theme.svelte';
  import { views } from './views';

  const themeLabel: Record<Theme, string> = { 'woven-atlas': 'Woven atlas', observatory: 'Observatory' };
  const modeLabel = { mock: 'Mock transport (invented data)', http: 'Browser · loopback HTTP', tauri: 'Desktop · Tauri', none: 'No transport' };
  const connLabel = { connecting: 'Connecting', connected: 'Connected', unavailable: 'Unavailable' };

  const current = $derived(views.find((v) => v.name === route.name));
  const Current = $derived(current?.component);

  $effect(() => {
    if (!route.name && views[0]) location.replace(`#/${views[0].name}`);
  });
  $effect(() => {
    document.title = `${current?.title ?? 'Loomward'} · Loomward`;
  });
</script>

<a class="skip" href="#main" onclick={(e) => { e.preventDefault(); document.getElementById('main')?.focus(); }}>Skip to content</a>

<header class="masthead">
  <div class="brand">
    <span class="wordmark">Loomward</span>
    <span class="tagline">{themeLabel[theme.current]}</span>
  </div>

  <ul class="status" role="status" aria-label="Session status">
    <li class="chip dataset" data-class={session.datasetClass ?? 'unknown'}>
      Dataset <strong>{session.datasetClass ?? 'unknown'}</strong>
    </li>
    <li class="chip sim" title="Loomward v0.3 observes and simulates. It never moves, deletes or changes anything.">
      <strong>Simulation</strong> · no file or process effects
    </li>
    <li class="chip mode" data-mode={session.mode}>{modeLabel[session.mode]}</li>
    <li class="chip conn" data-state={session.state}>{connLabel[session.state]}</li>
  </ul>

  <div class="themes" role="group" aria-label="Theme">
    {#each THEMES as t (t)}
      <button class="btn" type="button" aria-pressed={theme.current === t} onclick={() => theme.set(t)}>{themeLabel[t]}</button>
    {/each}
  </div>
  <div class="thread-rule" aria-hidden="true"></div>
</header>

<div class="frame">
  <nav class="rail" aria-label="Views">
    {#each views as v (v.name)}
      <a href={`#/${v.name}`} aria-current={route.name === v.name ? 'page' : undefined}>{v.title}</a>
    {/each}
  </nav>

  <main id="main" tabindex="-1">
    {#if session.state === 'connected'}
      {#if Current}
        {#key session.info?.session_started_at}
          <Current />
        {/key}
      {:else}
        <section class="panel">
          <h1>No such view</h1>
          <p class="muted">There is no view called <code>{route.name}</code>.</p>
        </section>
      {/if}
    {:else}
      <section class="panel unavailable" role="alert">
        <h1>{session.state === 'connecting' ? 'Connecting' : 'Unavailable'}</h1>
        {#if session.state === 'unavailable'}
          <p>Loomward cannot reach its engine, so it shows nothing. Earlier data is not kept on screen and nothing here is sample data.</p>
          {#if session.reason}<p class="muted">{session.reason}</p>{/if}
        {:else}
          <p class="muted">Waiting for the engine to answer.</p>
        {/if}
      </section>
    {/if}
  </main>
</div>

<footer class="legend" aria-label="Thread legend">
  <span class="legend-title">Threads never blend</span>
  <span class="thread meaning"><i></i><em>Warp</em> · meaning, vertical dyed threads</span>
  <span class="thread residency"><i></i><em>Weft</em> · residency, crossing metal threads</span>
  <span class="thread permission"><i></i><em>Selvedge</em> · permission, a stitched edge</span>
</footer>

<style>
  .skip { position: absolute; left: -999px; top: 0; background: var(--accent); color: var(--accent-ink); padding: 8px 12px; z-index: 10; }
  .skip:focus { left: 8px; top: 8px; }

  .masthead {
    position: sticky; top: 0; z-index: 5;
    display: flex; flex-wrap: wrap; align-items: center; gap: 10px 20px;
    padding: 12px var(--gutter) 14px;
    background: color-mix(in srgb, var(--bg) 92%, transparent);
    backdrop-filter: blur(6px);
    border-bottom: 1px solid var(--line);
  }
  .brand { display: flex; align-items: baseline; gap: 12px; margin-right: auto; }
  .wordmark { font-family: var(--head-font); font-size: 1.55rem; font-weight: var(--head-weight); letter-spacing: var(--head-tracking); }
  .tagline { color: var(--muted); font-size: 0.8rem; letter-spacing: 0.14em; text-transform: uppercase; }

  .status { display: flex; flex-wrap: wrap; gap: 6px; list-style: none; margin: 0; padding: 0; }
  .chip {
    font-size: 0.8rem; padding: 2px 10px; border: 1px solid var(--line); border-radius: 999px;
    background: var(--surface); white-space: nowrap;
  }
  .chip strong { font-family: var(--num-font); }
  .dataset[data-class='synthetic'] { border-color: var(--residency); }
  .dataset[data-class='personal'] { border-color: var(--warn); color: var(--warn); }
  .dataset[data-class='unknown'] { border-style: dashed; color: var(--muted); }
  .sim { border-color: var(--permission); }
  .mode[data-mode='mock'] { border-color: var(--warn); color: var(--warn); }
  .conn::before { content: ''; display: inline-block; width: 8px; height: 8px; margin-right: 6px; border-radius: 50%; background: var(--muted); }
  .conn[data-state='connected']::before { background: var(--ok); box-shadow: var(--glow); }
  .conn[data-state='unavailable'] { border-color: var(--danger); color: var(--danger); }
  .conn[data-state='unavailable']::before { background: var(--danger); }

  .themes { display: flex; gap: 4px; }
  .thread-rule { flex-basis: 100%; height: 3px; border-radius: 2px; background: var(--chrome-line); opacity: 0.85; }

  .frame { display: grid; grid-template-columns: 200px minmax(0, 1fr); gap: 20px; padding: 20px var(--gutter); max-width: 1500px; margin: 0 auto; }
  .rail { display: flex; flex-direction: column; gap: 2px; position: sticky; top: 96px; align-self: start; }
  .rail a {
    padding: 8px 12px; text-decoration: none; color: var(--muted); border-left: 2px solid transparent;
    transition: color var(--dur) var(--ease), border-color var(--dur) var(--ease);
  }
  .rail a:hover { color: var(--text); }
  .rail a[aria-current='page'] { color: var(--text); border-left-color: var(--accent); background: var(--surface); }
  main { min-width: 0; outline: none; }
  .unavailable h1 { color: var(--danger); }

  .legend {
    display: flex; flex-wrap: wrap; gap: 8px 22px; justify-content: center; padding: 14px var(--gutter) 24px;
    color: var(--muted); font-size: 0.82rem; border-top: 1px solid var(--line);
  }
  .legend-title { letter-spacing: 0.1em; text-transform: uppercase; }
  .thread { display: inline-flex; align-items: center; gap: 8px; }
  /* the same structures the views weave: vertical warp, horizontal weft, a stitched selvedge */
  .thread i { width: 22px; height: 14px; }
  .thread em { font-style: normal; color: var(--text); }
  .thread.meaning i { background: repeating-linear-gradient(90deg, var(--meaning) 0 3px, transparent 3px 5px); }
  .thread.residency i { background: repeating-linear-gradient(0deg, var(--residency) 0 3px, transparent 3px 5px); }
  .thread.permission i { border: 2px dashed var(--permission); }

  @media (max-width: 760px) {
    .frame { grid-template-columns: 1fr; gap: 12px; }
    .rail { flex-direction: row; flex-wrap: wrap; position: static; }
    .rail a { border-left: 0; border-bottom: 2px solid transparent; }
    .rail a[aria-current='page'] { border-bottom-color: var(--accent); }
    .brand { margin-right: 0; flex-basis: 100%; }
    .chip { white-space: normal; }
  }
</style>
