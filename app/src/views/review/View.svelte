<script module lang="ts">
  export const meta = { title: 'Learning review', order: 50 };
</script>

<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import { session } from '../../lib/stores/session.svelte';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import { LoomwardError } from '../../lib/transport/client';
  import { formatTime } from '../../lib/format/time';
  import { HistoryPanel } from './history.svelte';
  import type { FeedbackRequest, Job, LearningQueue, LearningStatus } from '../../lib/contracts.gen';

  let status = $state<LearningStatus | null>(null);
  let queue = $state<LearningQueue | null>(null);
  let error = $state('');
  let message = $state('');
  let working = $state(false);
  let selected = $state('');
  let label = $state('Documents');
  let job = $state<Job | null>(null);
  let pending = $state<FeedbackRequest | null>(null);
  let timer: ReturnType<typeof setTimeout> | undefined;
  let generation = 0;
  let alive = true;
  const history = new HistoryPanel();
  const handle = (e: unknown) => session.handle(e);
  const gated = $derived(status?.gate !== null && status?.gate !== undefined);
  const personal = $derived(status?.dataset_class === 'personal');
  const active = $derived(job !== null && ['queued', 'running', 'cancel_requested'].includes(job.state));

  async function load(cursor: string | null = null, restarted = false): Promise<void> {
    const client = session.client;
    if (!client) return;
    const mine = ++generation;
    const [s, q] = await Promise.allSettled([client.call('learning.status', {}), client.call('learning.queue', { root_id: null, limit: 20, cursor })]);
    if (!alive || mine !== generation) return;
    status = s.status === 'fulfilled' ? s.value.result : null;
    queue = q.status === 'fulfilled' ? q.value.result : null;
    if (!pending) selected = '';
    const failures = [s, q].filter(r => r.status === 'rejected');
    // A concurrent label or root change invalidates the queue's revision-bound snapshot.
    if (!restarted && failures.some(r => r.reason instanceof LoomwardError && r.reason.code === 'stale_generation')) return await load(null, true);
    error = failures.map(r => session.handle(r.reason)).join(' ');
  }
  async function record(retract = false): Promise<void> {
    const client = session.client;
    if (!client || !selected || working || gated) return;
    pending ??= { node_id: selected, label: retract ? null : label, retract, client_event_id: `ce_${crypto.randomUUID().replaceAll('-', '')}` };
    working = true;
    history.clear();
    try {
      const response = await client.call('feedback.record', pending);
      pending = null;
      message = `Human feedback recorded, revision ${response.result.revision}. Refit when ready.`;
      await load();
      if (alive) await history.load(session.client, handle);
    } catch (e) {
      if (e instanceof LoomwardError && ['invalid_request', 'not_found', 'permission_denied', 'capability_unavailable'].includes(e.code)) pending = null;
      error = session.handle(e);
    }
    finally { working = false; }
  }
  async function poll(): Promise<void> {
    const client = session.client;
    if (!client || !job || !alive) return;
    try {
      const result = await client.call('jobs.get', { job_id: job.job_id });
      if (!alive) return;
      job = result.result.job;
      if (['queued', 'running', 'cancel_requested'].includes(job.state)) timer = setTimeout(() => void poll(), 250);
      else { message = `Refit ${job.state}.`; await load(); }
    } catch (e) { if (alive) error = session.handle(e); }
  }
  async function refit(): Promise<void> {
    const client = session.client;
    if (!client || working || active || gated) return;
    working = true;
    try { job = (await client.call('learning.refit', {})).result.job; await poll(); }
    catch (e) { error = session.handle(e); }
    finally { working = false; }
  }
  async function cancel(): Promise<void> {
    if (!session.client || !job) return;
    try { job = (await session.client.call('jobs.cancel', { job_id: job.job_id })).result.job; }
    catch (e) { error = session.handle(e); }
  }
  function refresh(): void { void load(); void history.load(session.client, handle); }
  $effect(() => { void session.epoch; untrack(refresh); });
  onDestroy(() => { alive = false; generation++; clearTimeout(timer); history.clear(); });
</script>

<section aria-labelledby="review-title">
  <h1 id="review-title">Learning review</h1>
  <p class="note">Human labels teach organisation. They never grant permission to move or delete.</p>
  <p class="bad live" class:has={!!error} role="alert">{error}</p>
  <p class="live" class:has={!!message} role="status">{message}</p>
  {#if status}
    <div class="panel status-panel">
      {#if gated}
        <p class="note gated">Learning is gated ({status.gate}). Recording labels and refitting are unavailable.</p>
      {:else}
        <p class="counts"><strong class="num">{status.human_events}</strong> human events · <strong class="num">{status.training_count}</strong> fitted examples · {status.unresolved_human_events} unresolved events excluded</p>
        {#if status.model_id && status.human_events > status.training_count}
          <p class="note warn">Human history exceeds fitted examples. The candidate may be stale after label changes or withdrawals; refit to refresh advice.</p>
        {/if}
        {#if personal}<p class="note">Your labels train a local student on this machine. The teacher never sees personal data.</p>{/if}
        <p class="muted">{status.model_id ? `${personal ? 'Personal' : 'Synthetic'} candidate fitted; promotion has not run.` : 'No fitted model yet. Suggestions abstain until human evidence supports them.'} Relative scores, not probabilities.</p>
      {/if}
      <div class="controls">
        <button class="btn primary" disabled={working || active || gated} onclick={() => void refit()}>Refit from human labels</button>
        <button class="btn" disabled={working} onclick={refresh}>Refresh review</button>
        {#if active}<button class="btn" onclick={() => void cancel()}>Cancel refit</button>{/if}
      </div>
      {#if job}<p role="status">Refit {job.state}{#if job.error}: {job.error.message}{/if}</p>{/if}
    </div>
  {:else}
    <div class="empty">
      <p>Learning status unknown{error ? '.' : ' — loading…'}</p>
      <button class="btn" onclick={() => void load()}>Retry learning status</button>
    </div>
  {/if}
  {#if status && queue && !gated}
    <div class="panel">
      <div class="tbl-wrap">
        <table class="tbl">
          <caption>Review queue · relative scores, not probabilities</caption>
          <thead><tr><th scope="col">Item</th><th scope="col">Suggestion</th><th scope="col">Evidence</th><th scope="col">Review</th></tr></thead>
          <tbody>
            {#each queue.items as item (item.row.node_id)}
              <tr>
                <th scope="row"><VisibleName name={item.row.name} /></th>
                <td>{#if item.suggestion.ranked[0]}{item.suggestion.ranked[0].label} · <span class="num">{typeof item.suggestion.ranked[0].score === 'number' && Number.isFinite(item.suggestion.ranked[0].score) && item.suggestion.ranked[0].score >= 0 && item.suggestion.ranked[0].score <= 1 ? item.suggestion.ranked[0].score.toFixed(3) : 'unknown'}</span>{:else}Unknown · <span class="unknown">unknown</span>{/if}</td>
                <td><span class="tag" class:sim={!item.suggestion.abstain} class:unk={item.suggestion.abstain}>{item.suggestion.abstain ? 'Abstaining' : 'Advice only'}</span>{#if item.suggestion.reasons.length}<small>{item.suggestion.reasons.map(r => r.replaceAll('_', ' ')).join(', ')}</small>{/if}</td>
                <td><button class="btn" disabled={working || pending !== null} aria-pressed={selected === item.row.node_id} onclick={() => { selected = item.row.node_id; message = ''; }}>Review <span class="sr-only"><VisibleName name={item.row.name} /></span></button></td>
              </tr>
            {:else}<tr><td colspan="4" class="muted">No eligible durable files in this scope.</td></tr>{/each}
          </tbody>
        </table>
      </div>
      {#if selected}
        <div class="controls feedback">
          <div class="lw-field">
            <label class="lbl" for="human-label">Human label</label>
            <select id="human-label" bind:value={label} disabled={working || pending !== null}>
              {#each Object.keys(status?.human_support ?? {}) as choice}<option>{choice}</option>{/each}
            </select>
          </div>
          {#if pending}<button class="btn primary" disabled={working} onclick={() => void record()}>Retry same feedback</button>
          {:else}<button class="btn primary" disabled={working} onclick={() => void record()}>Record human label</button><button class="btn" disabled={working} onclick={() => void record(true)}>Withdraw human label</button>{/if}
        </div>
      {/if}
      {#if queue.next_cursor}<div class="controls"><button class="btn" disabled={working} onclick={() => void load(queue?.next_cursor ?? null)}>Next review page</button></div>{/if}
      <p class="muted small limits">{personal ? 'Advice only, never an approval.' : 'Synthetic smoke only.'} Limits: 10,000 eligible files and the latest labels for at most 10,000 resolved objects per refit. No teacher labels are trained here.</p>
    </div>
  {/if}
  <section class="panel history" aria-labelledby="history-title" aria-busy={history.loading}>
    <h2 id="history-title">Human feedback history</h2>
    <p class="note">Newest first. Current means the latest record for that object; a withdrawal leaves no active human label. Recorded names may differ from current names. Teacher labels and permissions are separate.</p>
    <p class="bad live" class:has={!!history.problem} role="alert">{history.problem}</p>
    <p class="live" class:has={!!history.notice || history.loading} role="status">{history.loading ? 'Loading history…' : history.notice}</p>
    <div class="controls">
      <button class="btn" disabled={history.loading || working} onclick={() => void history.load(session.client, handle)}>{history.problem ? 'Retry history' : 'Newest records'}</button>
      <button class="btn" disabled={history.loading || working || !history.page?.next_cursor} onclick={() => void history.load(session.client, handle, history.page?.next_cursor)}>Older records</button>
      {#if history.older}<span class="muted">Older page</span>{/if}
    </div>
    {#if history.page}
      <div class="tbl-wrap">
        <table class="tbl">
          <caption>Human feedback records</caption>
          <thead><tr><th scope="col">Recorded item</th><th scope="col">Feedback</th><th scope="col">Record</th><th scope="col">When</th></tr></thead>
          <tbody>
            {#each history.page.items as item (item.event_id)}
              <tr>
                <th scope="row">{#if item.name !== null}<VisibleName name={item.name} />{:else}<span class="muted">Recorded name hidden</span>{/if}<small>{item.reference_status === 'resolved' ? 'Reference resolved' : item.reference_status === 'redacted' ? 'Details redacted or provenance unavailable' : 'Reference unresolved'}</small></th>
                <td>{item.retracted ? 'Withdrawal' : item.revision > 1 ? 'Correction' : 'Label'} · {item.label ?? 'Unknown label'}</td>
                <td>Revision {item.revision}<small>{item.is_current ? 'Current record' : 'Superseded'}</small></td>
                <td><time datetime={item.recorded_at}>{formatTime(item.recorded_at)}</time></td>
              </tr>
            {:else}<tr><td colspan="4" class="muted">No human feedback records.</td></tr>{/each}
          </tbody>
        </table>
      </div>
    {/if}
  </section>
</section>

<style>
  .counts { margin-top: 0; }
  .counts strong { font-family: var(--num-font); }
  .feedback { margin: var(--sp-4) 0 0; padding-top: var(--sp-4); border-top: 1px solid var(--line); }
  .limits { margin: var(--sp-4) 0 0; max-width: var(--measure); }
  .tbl td small { max-width: 28rem; }
  .history { margin-top: var(--sp-4); }
  .history h2 { margin-top: 0; }
  .history th small { display: block; font-weight: normal; }
</style>
