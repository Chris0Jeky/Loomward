<script module lang="ts">
  export const meta = { title: 'Ask', order: 32 };
</script>
<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { session } from '../../lib/stores/session.svelte';
  import { formatBytes } from '../../lib/format/bytes';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import type { AskField, AskStatus, AskTurnRecord, AskCitation, NodeDetail, Root } from '../../lib/contracts.gen';
  import { FAILURE_WORDS, sentencesOf, providerOf, scopeOf, scopeSentence, shortDigest } from './model';

  const FIELD_WORDS: Record<AskField, string> = { path: 'Paths inside the folder', size: 'Sizes', modified: 'Modified dates', attributes: 'Attributes', volumes: 'All drives and free space' };
  const POLL_MS = 400;

  let status = $state<AskStatus | null>(null);
  let roots = $state<Root[]>([]);
  let turn = $state<AskTurnRecord | null>(null);
  let question = $state('');
  let error = $state('');
  let notice = $state('');
  let loading = $state(false);
  let sending = $state(false);
  let pickRoots = $state<string[]>([]);
  let pickFields = $state<AskField[]>(['path', 'size', 'modified']);
  let profileUrl = $state('http://127.0.0.1:11434/v1');
  let profileModel = $state('');
  let opened = $state<{ citation: AskCitation; detail: NodeDetail | null; error: string } | null>(null);
  let openedEl = $state<HTMLElement | null>(null);
  let generation = 0;
  let pollTimer: ReturnType<typeof setTimeout> | undefined;

  const scope = $derived(scopeOf(roots, status?.grant ?? null));
  const provider = $derived(providerOf(status, turn));
  const active = $derived(roots.filter((r) => r.grant_state === 'active'));
  const desktop = $derived(session.mode === 'tauri');
  const canAsk = $derived(status?.provider === 'local' && scope.visible.length > 0 && turn?.state !== 'running');
  const label = (r: Root) => r.display_path.text;

  async function load(): Promise<void> {
    if (!session.client) return;
    const mine = ++generation;
    loading = true; error = '';
    try {
      const [s, r] = await Promise.all([session.client.call('ask.status', {}), session.client.call('roots.list', {})]);
      if (mine !== generation) return;
      status = s.result; roots = r.result.roots;
      if (s.result.last_turn && (turn?.turn_id !== s.result.last_turn || turn?.state === 'running')) await read(s.result.last_turn, mine);
    } catch (e) {
      if (mine === generation) { error = session.handle(e); status = null; }
    } finally { if (mine === generation) loading = false; }
  }

  async function read(turnId: string, mine: number): Promise<void> {
    clearTimeout(pollTimer);
    try {
      const { result } = await session.client!.call('ask.result', { turn_id: turnId });
      if (mine !== generation) return;
      turn = result;
      if (result.state === 'running') pollTimer = setTimeout(() => void read(turnId, mine), POLL_MS);
      else notice = result.state === 'completed' ? 'Answer ready.' : 'The question could not be answered.';
    } catch (e) { if (mine === generation) error = session.handle(e); }
  }

  async function ask(): Promise<void> {
    if (!session.client || sending) return;
    const q = question.trim();
    if (!q) { error = 'Type a question first.'; return; }
    error = ''; notice = ''; opened = null; sending = true;
    const mine = ++generation;
    try {
      const { result } = await session.client.call('ask.turn', { question: q });
      if (mine !== generation) return;
      turn = result; notice = 'Asking the local model…';
      if (status) status = { ...status, last_turn: result.turn_id };
      await read(result.turn_id, mine);
    } catch (e) { if (mine === generation) error = session.handle(e); }
    finally { sending = false; }
  }

  /** An owner command that answers with the new status. */
  async function owner(run: () => Promise<{ result: AskStatus }>, done: string): Promise<void> {
    if (!session.client) return;
    error = ''; notice = '';
    try {
      status = (await run()).result; notice = done;
    } catch (e) { error = session.handle(e); }
  }

  const grant = () => {
    if (!pickRoots.length) { error = 'Choose at least one folder Ask may see.'; return; }
    void owner(() => session.client!.call('ask.grant', { roots: pickRoots, fields: pickFields }), 'Ask may now see the folders you chose. Revoke it at any time.');
  };

  async function open(c: AskCitation): Promise<void> {
    opened = { citation: c, detail: null, error: '' };
    try {
      const { result } = await session.client!.call('node.inspect', { node_id: c.node_id });
      if (opened?.citation === c) opened = { citation: c, detail: result, error: '' };
    } catch (e) { if (opened?.citation === c) opened = { citation: c, detail: null, error: session.handle(e) }; }
    await tick(); openedEl?.focus();
  }

  $effect(() => {
    session.epoch;
    untrack(() => void load());
    return () => { generation++; clearTimeout(pollTimer); };
  });
</script>

<h1>Ask</h1>
<p class="lede">Ask a model on this machine where things are, what a folder holds, or which files match a description. It looks things up with the same search you use and cites every entry it names.</p>
<p class="note">Ask can only read. It has no tool that moves, deletes, renames, opens or changes anything, and it cannot approve anything for you: a suggestion in an answer is text for you to act on yourself. <strong>No preview per question:</strong> a local model works under the folder grant below, which is the control (the docs/46 recommendation for Q3, waiting on your decision). Cloud models are not available.</p>
<p class="live muted" class:has={!!notice} role="status" aria-label="Ask result">{notice}</p>
<p id="ask-error" class="live bad" class:has={!!error} role="alert">{error}</p>

<section class="panel" aria-labelledby="ask-provider">
  <h2 id="ask-provider">Model</h2>
  <p><span class="tag" class:ok={provider.tone === 'ok'} class:bad={provider.tone === 'bad'} class:unk={provider.tone === 'unk'} data-testid="ask-provider">{provider.label}</span> <span class="muted">{provider.detail}</span></p>
  {#if status?.profile_digest}<p class="muted small">Profile digest <code title={status.profile_digest}>{shortDigest(status.profile_digest)}</code>, bound into every record below.</p>{/if}
  <p class="muted small">The configured loopback endpoint is unauthenticated. Another local process occupying that port can receive the approved request. Loomward checks the endpoint is loopback and the model name is not a cloud model; it cannot prove the server keeps your data on this machine.</p>
  {#if desktop}
    <form class="controls" onsubmit={(e) => { e.preventDefault(); void owner(() => session.client!.call('ask.configure', { profile: { base_url: profileUrl.trim(), model: profileModel.trim() } }), 'Model saved. Nothing is contacted until you ask a question.'); }}>
      <label class="lw-field"><span class="lbl">Endpoint (loopback, with port)</span><input type="text" bind:value={profileUrl} maxlength="160" autocomplete="off" spellcheck="false" /></label>
      <label class="lw-field"><span class="lbl">Model name</span><input type="text" bind:value={profileModel} maxlength="128" autocomplete="off" spellcheck="false" /></label>
      <button class="btn primary" type="submit">Save model</button>
      {#if status?.profile}<button class="btn" type="button" onclick={() => void owner(() => session.client!.call('ask.configure', { profile: null }), 'Model removed.')}>Remove model</button>{/if}
    </form>
  {:else if status?.provider === 'not_configured'}
    <p>Set up a model in the desktop app: a loopback endpoint such as <code>http://127.0.0.1:11434/v1</code> and a model you run locally. Loomward never picks, downloads or discovers one.</p>
  {/if}
</section>

<section class="panel" aria-labelledby="ask-grant">
  <h2 id="ask-grant">What Ask may see</h2>
  <p id="ask-scope" data-testid="ask-scope">{scopeSentence(scope)}</p>
  {#if scope.visible.length}
    <ul class="roots">
      {#each scope.visible as r (r.root_id)}<li><VisibleName name={label(r)} />{#if r.scan.state !== 'complete' || r.scan.coverage !== 'complete'} <span class="tag unk">unknown: {scope.unknown.find((u) => u.root === r)?.why}</span>{/if}</li>{/each}
    </ul>
    <p class="muted small">Also disclosed: {status?.grant?.fields.length ? status.grant.fields.map((f) => FIELD_WORDS[f].toLowerCase()).join(', ') : 'names and kinds only'}.</p>
    <button class="btn" type="button" onclick={() => void owner(() => session.client!.call('ask.revoke', {}), 'Revoked. The next lookup and the next question see nothing.')}>Revoke Ask's access</button>
    <p class="muted small">A question already running is not stopped: it finishes with what it already looked up, sending those rows to the same local model again, and gets nothing new.</p>
  {:else}
    <div class="empty-grant">
      <p>Ask sees nothing until you choose folders for it. It then reads names{pickFields.length ? ' and the details you tick' : ''} inside those folders only, never file contents, and only when you ask a question. Revoking stops it from the next lookup.</p>
      {#if active.length}
        <form onsubmit={(e) => { e.preventDefault(); grant(); }}>
          <fieldset><legend>Folders</legend>
            {#each active as r (r.root_id)}<label class="check"><input type="checkbox" value={r.root_id} bind:group={pickRoots} /> <VisibleName name={label(r)} /></label>{/each}
          </fieldset>
          <fieldset><legend>Details besides names</legend>
            {#each Object.entries(FIELD_WORDS) as [f, w] (f)}<label class="check"><input type="checkbox" value={f} bind:group={pickFields} /> {w}</label>{/each}
          </fieldset>
          <button class="btn primary" type="submit">Let Ask see these folders</button>
        </form>
      {:else}
        <p class="muted">Add a folder to Loomward first (Grants &amp; health); then choose it here.</p>
      {/if}
    </div>
  {/if}
</section>

<section class="panel" aria-labelledby="ask-q">
  <h2 id="ask-q">Question</h2>
  <form class="ask-form" onsubmit={(e) => { e.preventDefault(); void ask(); }}>
    <label class="lw-field grow"><span class="lbl">Question</span>
      <textarea bind:value={question} maxlength="2000" rows="3" aria-describedby="ask-scope"></textarea></label>
    <button class="btn primary" type="submit" aria-disabled={!canAsk || sending} onclick={(e) => { if (!canAsk || sending) e.preventDefault(); }}>{turn?.state === 'running' ? 'Thinking…' : 'Ask'}</button>
  </form>
  {#if !loading && status && !canAsk && turn?.state !== 'running'}<p class="muted small">{status.provider !== 'local' ? 'No model is set up yet.' : 'Choose folders Ask may see first.'}</p>{/if}
  <details class="tools"><summary>Tools the model may use ({status?.tools.length ?? 0}, all read-only)</summary>
    <ul aria-label="Ask tools">{#each status?.tools ?? [] as t (t.name)}<li><code>{t.name}</code> <span class="tag ok">read-only</span> <span class="muted">via {t.backed_by}</span></li>{/each}</ul>
    <p class="muted small">Any other tool name the model tries is refused and listed under the answer.</p>
  </details>
</section>

{#if turn}
  <section class="panel" aria-labelledby="ask-answer" aria-busy={turn.state === 'running'}>
    <h2 id="ask-answer">Answer</h2>
    <p class="muted small">You asked: <q>{turn.question}</q></p>
    {#if turn.state === 'running'}
      <p class="muted">The local model is working. Lookups run against your granted folders only.</p>
    {:else if turn.failure}
      <p class="bad" data-testid="ask-failure">{FAILURE_WORDS[turn.failure.kind] ?? turn.failure.message} <span class="muted small">({turn.failure.kind}{turn.failure.dispatched ? '; the request may have been sent' : '; nothing was sent'})</span></p>
    {:else}
      <p class="answer" data-testid="ask-answer">{#each sentencesOf(turn) as sentence, s (s)}<span>{#if sentence.unsupported}<span class="tag warn">Unsupported · no citation</span> {/if}{#each sentence.pieces as p, i (i)}{#if 'text' in p}{p.text}{:else if p.citation}<button class="cite" type="button" aria-label={`Citation ${p.handle}: ${p.citation.name ?? 'entry no longer readable'}`} onclick={() => void open(p.citation!)}><span class="h">{p.handle}</span>{#if p.citation.name}<VisibleName name={p.citation.name} />{:else}<span class="unknown">no longer readable</span>{/if}</button>{:else}<span class="tag warn" title="The model named this handle, but it never looked it up">{p.handle} · unverified reference</span>{/if}{/each}</span>{/each}</p>
      {#if turn.answer_truncated}<p class="muted small">The answer was cut at 16,000 characters.</p>{/if}
      <p class="muted small">Plain text from the model. Suggestions are not actions: nothing here runs, moves or approves anything.</p>
    {/if}
    {#if opened}
      <div class="opened" tabindex="-1" bind:this={openedEl} role="region" aria-label="Cited entry">
        {#if opened.detail}
          <p><strong><VisibleName name={opened.detail.row.name} /></strong> · {opened.detail.row.kind} · {formatBytes(opened.detail.row.logical_bytes)}{#if opened.detail.row.coverage !== 'complete'} · <span class="unknown">coverage {opened.detail.row.coverage}</span>{/if}</p>
          <p class="muted small"><VisibleName name={opened.detail.display_path.text} /></p>
          <p class="small"><a href="#/explorer">Open Explorer</a> to browse around it.</p>
        {:else if opened.error}<p class="bad">{opened.error}</p>{:else}<p class="muted">Opening {opened.citation.handle}…</p>{/if}
      </div>
    {/if}
    {#if turn.state !== 'running'}
      <h3>What Ask looked up</h3>
      {#if turn.tool_calls.length}
        <div class="tbl-wrap"><table class="tbl" aria-label="Tool calls">
          <thead><tr><th scope="col">Tool</th><th scope="col">Outcome</th><th scope="col" class="r">Rows</th></tr></thead>
          <tbody>{#each turn.tool_calls as u, i (i)}<tr><th scope="row"><code>{u.name}</code></th><td>{#if u.ok}<span class="tag ok">ran</span>{:else}<span class="tag bad">refused</span> {u.detail}{/if}</td><td class="r">{u.rows ?? '—'}</td></tr>{/each}</tbody>
        </table></div>
        {#if turn.tool_calls_total > turn.tool_calls.length}<p class="muted small">{turn.tool_calls_total - turn.tool_calls.length} more calls not listed.</p>{/if}
      {:else}<p class="muted">No lookups: the answer used nothing from your folders.</p>{/if}
      <h3>What was sent</h3>
      <p class="muted small">One record per request to the model, written before it was sent. Only names and the details you granted travel; never file contents.</p>
      <div class="tbl-wrap"><table class="tbl" aria-label="Disclosure records">
        <thead><tr><th scope="col">Round</th><th scope="col">To</th><th scope="col" class="r">Bytes</th><th scope="col">SHA-256</th><th scope="col">Outcome</th></tr></thead>
        <tbody>{#each turn.rounds as r (r.round)}<tr><th scope="row">{r.round}</th><td>{r.principal}</td><td class="r num">{r.bytes.toLocaleString()}</td><td><code title={r.sha256}>{shortDigest(r.sha256)}</code></td><td>{r.outcome}{r.dispatched ? '' : ' (not sent)'}</td></tr>{/each}</tbody>
      </table></div>
    {/if}
  </section>
{/if}

<style>
  .roots { margin: 0 0 var(--sp-2); padding-left: 1.2em; }
  .empty-grant { max-width: var(--measure); }
  fieldset { border: 1px solid var(--line); border-radius: var(--radius); margin: 0 0 var(--sp-3); padding: var(--sp-2) var(--sp-3); }
  legend { color: var(--muted); font-size: var(--fs-sm); padding: 0 var(--sp-1); }
  .check { display: flex; gap: var(--sp-2); align-items: center; min-height: 24px; }
  .ask-form { display: flex; flex-wrap: wrap; gap: var(--sp-3); align-items: flex-end; }
  .grow { flex: 1 1 24rem; }
  .grow textarea { width: 100%; box-sizing: border-box; font: inherit; color: inherit; background: var(--bg); border: 1px solid var(--line); border-radius: var(--radius); padding: var(--sp-2); resize: vertical; }
  .tools { margin-top: var(--sp-3); }
  .tools ul { margin: var(--sp-2) 0; padding-left: 1.2em; }
  .answer { max-width: var(--measure); line-height: 1.7; white-space: pre-wrap; overflow-wrap: anywhere; }
  .cite { display: inline-flex; gap: var(--sp-1); align-items: baseline; margin: 0 2px; padding: 0 6px; font: inherit; color: var(--text); background: var(--surface); border: 1px solid var(--accent); border-radius: 999px; cursor: pointer; }
  .cite:hover { background: var(--raised); }
  .cite .h { font: 0.8em var(--font-mono); color: var(--accent); }
  .opened { margin: var(--sp-3) 0; padding: var(--sp-2) var(--sp-3); border: 1px solid var(--line); border-radius: var(--radius); background: var(--bg); }
  .opened p { margin: 0 0 var(--sp-1); }
  h3 { margin: var(--sp-4) 0 var(--sp-2); font-size: var(--fs-sm); }
</style>
