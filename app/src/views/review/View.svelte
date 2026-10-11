<script module lang="ts">
  export const meta = { title: 'Learning review', order: 50 };
</script>

<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import { session } from '../../lib/stores/session.svelte';
  import VisibleName from '../../lib/ui/VisibleName.svelte';
  import { LoomwardError } from '../../lib/transport/client';
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
  const gated = $derived(status?.gate !== null && status?.gate !== undefined);
  const active = $derived(job !== null && ['queued', 'running', 'cancel_requested'].includes(job.state));

  async function load(cursor: string | null = null): Promise<void> {
    const client = session.client;
    if (!client) return;
    const mine = ++generation;
    try {
      const [s, q] = await Promise.all([client.call('learning.status', {}), client.call('learning.queue', { root_id: null, limit: 20, cursor })]);
      if (!alive || mine !== generation) return;
      status = s.result; queue = q.result; error = '';
      if (!pending) selected = '';
    } catch (e) {
      if (alive && mine === generation) { status = null; queue = null; error = session.handle(e); }
    }
  }
  async function record(retract = false): Promise<void> {
    const client = session.client;
    if (!client || !selected || working || gated) return;
    pending ??= { node_id: selected, label: retract ? null : label, retract, client_event_id: `ce_${crypto.randomUUID().replaceAll('-', '')}` };
    working = true;
    try {
      const response = await client.call('feedback.record', pending);
      pending = null;
      message = `Human feedback recorded, revision ${response.result.revision}. Refit when ready.`;
      await load();
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
  $effect(() => { void session.epoch; untrack(() => void load()); });
  onDestroy(() => { alive = false; generation++; clearTimeout(timer); });
</script>

<section aria-labelledby="review-title">
  <h1 id="review-title">Learning review</h1>
  <p class="note">Human labels teach organisation. They never grant permission to move or delete.</p>
  <p class="bad live" class:has={!!error} role="alert">{error}</p>
  <p class="live" class:has={!!message} role="status">{message}</p>
  {#if status}
    <div class="panel status-panel">
      {#if gated}
        <p class="note gated">Personal learning is gated. Recording labels and refitting await a separate owner decision.</p>
      {:else}
        <p class="counts"><strong class="num">{status.human_events}</strong> human events · <strong class="num">{status.training_count}</strong> fitted examples · {status.unresolved_human_events} unresolved events excluded</p>
        {#if status.model_id && status.human_events > status.training_count}
          <p class="note warn">Human history exceeds fitted examples. The candidate may be stale after label changes or withdrawals; refit to refresh advice.</p>
        {/if}
        <p class="muted">{status.model_id ? 'Synthetic candidate fitted; promotion has not run.' : 'No fitted model yet. Suggestions abstain until human evidence supports them.'} Relative scores, not probabilities.</p>
      {/if}
      <div class="controls">
        <button class="btn primary" disabled={working || active || gated} onclick={() => void refit()}>Refit from human labels</button>
        <button class="btn" disabled={working} onclick={() => void load()}>Refresh review</button>
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
  {#if queue && !gated}
    <div class="panel">
      <div class="tbl-wrap">
        <table class="tbl">
          <caption>Review queue · relative scores, not probabilities</caption>
          <thead><tr><th scope="col">Item</th><th scope="col">Suggestion</th><th scope="col">Evidence</th><th scope="col">Review</th></tr></thead>
          <tbody>
            {#each queue.items as item (item.row.node_id)}
              <tr>
                <th scope="row"><VisibleName name={item.row.name} /></th>
                <td>{#if item.suggestion.ranked[0]}{item.suggestion.ranked[0].label} · <span class="num">{item.suggestion.ranked[0].score.toFixed(3)}</span>{:else}Unknown · <span class="unknown">unknown</span>{/if}</td>
                <td><span class="tag" class:sim={!item.suggestion.abstain} class:unk={item.suggestion.abstain}>{item.suggestion.abstain ? 'Abstaining' : 'Advice only'}</span>{#if item.suggestion.reasons.length}<small>{item.suggestion.reasons.map(r => r.replaceAll('_', ' ')).join(', ')}</small>{/if}</td>
                <td><button class="btn" disabled={working || pending !== null} aria-pressed={selected === item.row.node_id} onclick={() => { selected = item.row.node_id; message = ''; }}>Review <span class="sr-only"><VisibleName name={item.row.name} /></span></button></td>
              </tr>
            {:else}<tr><td colspan="4" class="muted">No eligible durable files in this scope.</td></tr>{/each}
          </tbody>
        </table>
      </div>
      {#if selected}
        <div class="controls feedback">
          <div class="ctl">
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
      <p class="muted small limits">Synthetic smoke only. Limits: 10,000 eligible files and the latest labels for at most 10,000 resolved objects per refit. No teacher labels are trained here.</p>
    </div>
  {/if}
</section>

<style>
  .counts { margin-top: 0; }
  .counts strong { font-family: var(--num-font); }
  .feedback { margin: var(--sp-4) 0 0; padding-top: var(--sp-4); border-top: 1px solid var(--line); }
  .limits { margin: var(--sp-4) 0 0; max-width: var(--measure); }
  .tbl td small { max-width: 28rem; }
</style>
