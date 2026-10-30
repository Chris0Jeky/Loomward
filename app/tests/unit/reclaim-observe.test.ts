import { afterEach, describe, expect, it, vi } from 'vitest';
import type { EventEnvelope, Job } from '../../src/lib/contracts.gen';
import { ReclaimObserve } from '../../src/lib/stores/reclaim-observe.svelte';
import { session } from '../../src/lib/stores/session.svelte';
import { Client, LoomwardError } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { observeLines, observeProgressText, observeRefusal, observeStatus } from '../../src/views/reclaim/model';

const job = (patch: Partial<Job> = {}): Job => ({ job_id: 'jb_fixture', kind: 'reclaim', root_id: null, seq: 1, rev: 0, state: 'running', created_at: '2026-10-01T00:00:00Z', started_at: null, finished_at: null, coverage: null, error: null, progress: null, reclaim: { candidates_total: 4, candidates_done: 1, names_checked: 1200 }, ...patch });
const error = (code: LoomwardError['code'], message: string, detail: Record<string, unknown> | null = null) => new LoomwardError({ code, message, retryable: false, detail });
afterEach(() => { session.client = null; vi.useRealTimers(); });

async function mock() {
  vi.useFakeTimers();
  const transport = createMockTransport({ count: 32 });
  const client = new Client(transport);
  session.client = client;
  const store = new ReclaimObserve();
  transport.subscribe((e) => store.onEvent(e), () => {});
  const volume_id = (await client.call('volumes.list', {})).result.volumes[0]!.volume_id;
  return { client, store, volume_id };
}

describe('reclaim observe store', () => {
  it('observes a selection without changing the old per-candidate result', async () => {
    const { client, store, volume_id } = await mock();
    const selection = ['nd_synthetic'];
    await store.start(volume_id, selection);
    await vi.advanceTimersByTimeAsync(2000);
    const report = (await client.call('reclaim.plan', { volume_id, limit: 50, selection })).result;
    expect(report.selection).toMatchObject({ members: selection, status: 'recorded', reclaimable_bytes: '6144' });
    expect(report.groups[0]!.candidates[0]!.reclaimable_bytes).toBeNull();
    expect((await client.call('reclaim.plan', { volume_id, limit: 50 })).result.selection).toBeUndefined();
    for (const invalid of [[], ['nd_synthetic', 'nd_synthetic'], ['nd_unknown']]) {
      await expect(client.call('reclaim.observe', { volume_id, selection: invalid })).rejects.toMatchObject({ code: 'invalid_request' });
    }
  });
  it('starts, follows progress events and ends completed, after which the plan shows a number', async () => {
    const { client, store, volume_id } = await mock();
    const plan = async () => (await client.call('reclaim.plan', { volume_id, limit: 50 })).result.groups[0]!.candidates[0]!;
    expect(await plan()).toMatchObject({ reclaimable_bytes: null, observed_at: null });
    await store.start(volume_id);
    expect(store.active).toBe(true);
    await vi.advanceTimersByTimeAsync(800);
    expect(store.job).toMatchObject({ state: 'running', reclaim: { candidates_total: 1, candidates_done: 0, names_checked: 4200 } });
    expect(observeStatus(store.job)).toBe('Observing hard links · 0 of 1 candidates observed · 4,200 names checked');
    await vi.advanceTimersByTimeAsync(1000);
    expect(store.job).toMatchObject({ state: 'completed', reclaim: { candidates_done: 1 } });
    expect(store.active).toBe(false);
    // #1029: the known figure says when it was observed; the unknown ones keep no age.
    const stored = await plan();
    expect(stored).toMatchObject({ reclaimable_bytes: '6144', reclaimable_reason: null });
    expect(Date.parse(stored.observed_at!)).not.toBeNaN();
    const report = (await client.call('reclaim.plan', { volume_id, limit: 50 })).result;
    for (const c of report.groups.flatMap((g) => g.candidates)) expect(c.observed_at !== null).toBe(c.reclaimable_bytes !== null);
  });

  it('cancels, stores nothing and leaves relief unknown', async () => {
    const { client, store, volume_id } = await mock();
    await store.start(volume_id);
    await vi.advanceTimersByTimeAsync(300);
    await store.cancel();
    expect(store.job?.state).toBe('cancel_requested');
    await vi.advanceTimersByTimeAsync(2000);
    expect(store.job?.state).toBe('cancelled');
    expect(observeStatus(store.job)).toContain('stay unknown');
    expect((await client.call('reclaim.plan', { volume_id, limit: 50 })).result.groups[0]!.candidates[0]!.reclaimable_bytes).toBeNull();
  });

  it('refuses a second start while one runs, and follows the running job on busy', async () => {
    const { client, store, volume_id } = await mock();
    await store.start(volume_id);
    const first = store.job!.job_id;
    await expect(client.call('reclaim.observe', { volume_id })).rejects.toMatchObject({ code: 'busy' });
    store.job = null as Job | null; // a second page that never saw the first job
    await store.start(volume_id);
    expect(store.problem).toBe(''); // the adopted job's status now says what the refusal meant
    expect((store.job as Job | null)?.job_id).toBe(first);
  });

  it('picks up a running job on sync but not an old finished one, and ignores stale and foreign snapshots', async () => {
    const { client, store, volume_id } = await mock();
    await client.call('reclaim.observe', { volume_id });
    await store.sync();
    expect(store.active).toBe(true);
    await vi.advanceTimersByTimeAsync(2000);
    const fresh = new ReclaimObserve();
    await fresh.sync();
    expect(fresh.job).toBeNull();
    store.apply(job({ job_id: 'jb_old', seq: 0 }));
    store.apply(job({ kind: 'scan', seq: 99 }));
    expect(store.job?.job_id).not.toBe('jb_old');
    expect(store.job?.kind).toBe('reclaim');
  });

  it('reports a refusal from the service in plain words and keeps no job', async () => {
    const store = new ReclaimObserve();
    const refuse = async (r: { request_id: string }) => ({ protocol: 'loomward/3', request_id: r.request_id, ok: false, error: { code: 'permission_denied', message: 'non-elevated access required', retryable: false, detail: { reason: 'elevated_token' } } });
    session.client = new Client({ call: refuse, subscribe: () => () => {} } as never);
    await store.start('vo_x');
    expect(store.job).toBeNull();
    expect(store.problem).toContain('administrator token');
    expect(store.pending).toBe(false);
  });
});

describe('engine lifetime', () => {
  it('discards a jobs.get answer in flight across an engine restart, then adopts the new job', async () => {
    const { client, store } = await mock();
    const hello = (epoch: string) => ({ event: 'stream.hello', epoch } as EventEnvelope);
    store.onEvent(hello('one'));
    store.apply(job({ job_id: 'jb_old', seq: 9, rev: 1 }));
    const real = client.transport.call.bind(client.transport);
    let release!: () => void;
    const held = new Promise<void>((resolve) => { release = resolve; });
    (client.transport as { call: unknown }).call = async (r: { command: string }, s?: AbortSignal) => {
      const res = await real(r as never, s);
      if (r.command !== 'jobs.get') return res;
      await held;
      return { ...res, result: { job: job({ job_id: 'jb_old', seq: 9, rev: 2 }) } };
    };
    const late = store.refresh();
    store.onEvent(hello('two')); // the engine restarted while the poll was out
    expect(store.job).toBeNull();
    release();
    await late;
    expect(store.job).toBeNull(); // the old engine's job is not resurrected
    expect(store.problem).toBe('');
    store.apply(job({ job_id: 'jb_new', seq: 1 })); // the new engine numbers from 1
    expect(store.job?.job_id).toBe('jb_new');
  });
});

describe('observe lines as the view shows them', () => {
  it('after a busy refusal the adopted job is shown with progress, not the stale refusal, and is not claimed for this volume', async () => {
    const { client, store, volume_id } = await mock();
    await client.call('reclaim.observe', { volume_id }); // started elsewhere
    await store.start(volume_id);
    const lines = observeLines(store);
    expect(lines.problem).toBe('');
    expect(lines.status).toMatch(/did not start.*does not say which volume.*0 of 1 candidates observed/);
    await vi.advanceTimersByTimeAsync(800);
    await store.refresh();
    expect(observeLines(store).status).toContain('4,200 names checked');
  });

  it('a job this page started is described as running, not as foreign', async () => {
    const { store, volume_id } = await mock();
    await store.start(volume_id);
    await vi.advanceTimersByTimeAsync(300);
    expect(observeLines(store).status).toMatch(/^Observing hard links · /);
  });

  it('a transient poll error shows beside the status and clears on the next successful poll', async () => {
    const { client, store, volume_id } = await mock();
    await store.start(volume_id);
    await vi.advanceTimersByTimeAsync(300);
    const real = client.transport.call.bind(client.transport);
    let down = true;
    (client.transport as { call: unknown }).call = (r: { command: string }, s?: AbortSignal) =>
      down && r.command === 'jobs.get' ? Promise.reject(new Error('poll failed')) : real(r as never, s);
    await store.refresh();
    expect(observeLines(store)).toMatchObject({ problem: 'poll failed' });
    expect(observeLines(store).status).toMatch(/^Observing hard links/); // the job status is never hidden by the fault
    down = false;
    await store.refresh();
    expect(observeLines(store).problem).toBe('');
  });
});

describe('observe wording', () => {
  it('names each refusal the owner can act on, and falls back to the service text', () => {
    expect(observeRefusal(error('busy', 'x'))).toMatch(/already running/);
    expect(observeRefusal(error('permission_denied', 'x', { reason: 'elevated_token' }))).toMatch(/administrator token.*Nothing was read/);
    expect(observeRefusal(error('invalid_request', 'no_active_grant'))).toMatch(/active grant.*Nothing was read/);
    expect(observeRefusal(error('invalid_request', 'refused', { reason: 'no_active_grant' }))).toMatch(/active grant/);
    expect(observeRefusal(error('internal_error', 'engine fault'))).toBe('engine fault');
    expect(observeRefusal(new Error('boom'))).toBe('boom');
  });

  it('says what each job end means, never presenting unfinished candidates as zero', () => {
    expect(observeStatus(null)).toBe('');
    // A finished job this page did not start never presents its counts as this volume's.
    const foreign = observeStatus(job({ state: 'completed', coverage: 'complete', reclaim: { candidates_total: 2, candidates_done: 2, names_checked: 10 } }), false);
    expect(foreign).toContain('may have covered another volume');
    expect(foreign).not.toContain('2 of 2');
    expect(observeStatus(job({ state: 'completed', coverage: 'complete', reclaim: { candidates_total: 2, candidates_done: 2, names_checked: 10 } }))).toBe('Observation finished · 2 of 2 candidates observed · 10 names checked.');
    expect(observeStatus(job({ state: 'completed', coverage: 'partial' }))).toMatch(/^Observation stopped early · .*stay unknown/);
    expect(observeStatus(job({ state: 'failed', error: { code: 'internal_error', message: 'disk gone', retryable: false, detail: null } }))).toBe('Observation failed: disk gone. Candidates it did not finish stay unknown.');
    expect(observeStatus(job({ state: 'cancel_requested' }))).toMatch(/^Cancelling/);
    expect(observeProgressText({ candidates_total: 0, candidates_done: 0, names_checked: 0 })).toBe('No candidates to observe · 0 names checked');
    expect(observeProgressText(undefined)).toBe('Starting');
  });
});
