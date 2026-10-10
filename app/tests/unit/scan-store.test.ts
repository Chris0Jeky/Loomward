import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CommandKey, ErrorCode, EventEnvelope, Job, JobState } from '../../src/lib/contracts.gen';
import { scans } from '../../src/lib/stores/scan.svelte';
import { session } from '../../src/lib/stores/session.svelte';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import type { Transport } from '../../src/lib/transport/transport';

const job = (state: JobState, over: Partial<Job> = {}): Job => ({
  job_id: 'jb_1', kind: 'scan', state, root_id: 'rt_a', created_at: '2026-10-10T10:00:00Z', started_at: null, finished_at: null, progress: null, coverage: null, error: null, ...over,
});
const ev = (event: EventEnvelope['event'], data: unknown): EventEnvelope =>
  ({ protocol: 'loomward/3', epoch: 'e1', seq: 1, event, at: '2026-10-10T10:00:00Z', catalog_rev: null, state_rev: null, data } as EventEnvelope);
const fresh = () => { scans.book = {}; scans.problems = {}; scans.pending = {}; scans.change = null; };

describe('the scan store follows the event stream', () => {
  beforeEach(fresh);

  it('says each state once, and settles when a running scan ends', () => {
    const settled = scans.settled;
    scans.onEvent(ev('job.state', { job: job('queued') }));
    expect(scans.change?.text).toBe('Queued');
    scans.onEvent(ev('job.state', { job: job('running', { started_at: '2026-10-10T10:00:01Z' }) }));
    expect(scans.change?.text).toBe('Scanning');
    expect(scans.anyActive).toBe(true);
    expect(scans.settled).toBe(settled);
    const n = scans.change!.n;
    scans.onEvent(ev('job.state', { job: job('running') })); // a replay changes nothing and says nothing
    expect(scans.change!.n).toBe(n);
    scans.onEvent(ev('job.state', { job: job('completed', { coverage: 'partial', finished_at: '2026-10-10T10:00:05Z' }) }));
    expect(scans.change?.text).toMatch(/^Finished, partial coverage/);
    expect(scans.settled).toBe(settled + 1);
    expect(scans.anyActive).toBe(false);
  });

  it('attaches the final scan.progress to the job and uses it for the counts', () => {
    scans.onEvent(ev('job.state', { job: job('running') }));
    const p = { phase: 'done', strategy: 'std_read_dir', examined: 9, indexed_files: 7, indexed_dirs: 2, skipped: 0, failed: 0, logical_bytes: '10', allocated_bytes: null, pending_dirs: 0, writer_queue_depth: 0, entries_per_second: 1, elapsed_ms: 900, limit_hit: false };
    scans.onEvent(ev('scan.progress', { job_id: 'jb_1', root_id: 'rt_a', progress: p }));
    scans.onEvent(ev('job.state', { job: job('completed', { coverage: 'complete', finished_at: '2026-10-10T10:00:05Z' }) }));
    expect(scans.view('rt_a', 0)).toMatchObject({ activity: 'completed', elapsedMs: 900 });
    expect(scans.view('rt_a', 0)?.detail).toContain('7 files, 2 folders');
  });

  it('does not announce a finished scan met for the first time (history on page load)', async () => {
    session.client = new Client({ call: async (r: { request_id: string }) => ({ protocol: 'loomward/3', request_id: r.request_id, ok: true, result: { jobs: [job('completed', { coverage: 'complete' })] }, meta: {} }) } as unknown as Transport);
    const settled = scans.settled;
    await scans.sync();
    expect(scans.view('rt_a', 0)?.activity).toBe('completed');
    expect(scans.change).toBeNull();
    expect(scans.settled).toBe(settled);
    session.client = null;
  });
});

type Reply = unknown | { error: { code: ErrorCode; message: string; detail?: Record<string, unknown> | null } };
/** A client whose engine is a function; `calls` records the commands it was asked. */
function clientWith(handler: (command: string, payload: Record<string, unknown>) => Reply | Promise<Reply>) {
  const calls: string[] = [];
  const transport = {
    call: async (r: { request_id: string; command: string; payload: Record<string, unknown> }) => {
      calls.push(r.command);
      const h = (await handler(r.command, r.payload)) as { error?: object } | null;
      const base = { protocol: 'loomward/3', request_id: r.request_id };
      return h && typeof h === 'object' && 'error' in h ? { ...base, ok: false, error: { retryable: false, detail: null, ...h.error } } : { ...base, ok: true, result: h, meta: {} };
    },
  } as unknown as Transport;
  return { client: new Client(transport), calls };
}

describe('settling and syncing keep what was known', () => {
  beforeEach(fresh);
  afterEach(() => { session.client = null; });

  it('settles when a newer finished scan replaces an older finished one for the same root', () => {
    scans.onEvent(ev('job.state', { job: job('completed', { coverage: 'complete', finished_at: '2026-10-10T10:00:05Z' }) }));
    const settled = scans.settled;
    scans.onEvent(ev('job.state', { job: job('completed', { job_id: 'jb_2', created_at: '2026-10-10T11:00:00Z', coverage: 'partial', finished_at: '2026-10-10T11:00:05Z' }) }));
    expect(scans.view('rt_a', 0)?.activity).toBe('partial');
    expect(scans.settled).toBe(settled + 1);
  });

  it('does not settle for a replay of the same finished job', () => {
    const done = job('completed', { coverage: 'complete', finished_at: '2026-10-10T10:00:05Z' });
    scans.onEvent(ev('job.state', { job: done }));
    const settled = scans.settled;
    scans.onEvent(ev('job.state', { job: done }));
    expect(scans.settled).toBe(settled);
  });

  it('keeps a finished scan the jobs.list answer leaves out, and says nothing', async () => {
    scans.onEvent(ev('job.state', { job: job('completed', { coverage: 'complete', finished_at: '2026-10-10T10:00:05Z' }) }));
    const said = scans.change?.n;
    const { client, calls } = clientWith(() => ({ jobs: [] }));
    session.client = client;
    await scans.sync();
    expect(scans.view('rt_a', 0)?.activity).toBe('completed');
    expect(scans.change?.n).toBe(said);
    expect(calls).toEqual(['jobs.list']);
  });

  it('asks the service about a running scan the list leaves out, and applies the answer', async () => {
    scans.onEvent(ev('job.state', { job: job('running', { started_at: '2026-10-10T10:00:01Z' }) }));
    const { client, calls } = clientWith((c) => (c === 'jobs.list' ? { jobs: [] } : { job: job('completed', { coverage: 'complete', finished_at: '2026-10-10T10:00:05Z' }) }));
    session.client = client;
    await scans.sync();
    expect(calls).toEqual(['jobs.list', 'jobs.get']);
    expect(scans.view('rt_a', 0)?.activity).toBe('completed');
  });

  it('says so when the service no longer knows a scan that was running, never leaving it scanning', async () => {
    scans.onEvent(ev('job.state', { job: job('running', { started_at: '2026-10-10T10:00:01Z' }) }));
    const { client } = clientWith((c) => (c === 'jobs.list' ? { jobs: [] } : { error: { code: 'not_found', message: 'no such job' } }));
    session.client = client;
    await scans.sync();
    const v = scans.view('rt_a', 0)!;
    expect(v.activity).toBe('failed');
    expect(v.active).toBe(false);
    expect(v.detail).toContain('no longer lists this scan');
    expect(scans.change?.text).toContain('no longer lists this scan');
  });

  it('keeps a newer state that arrived on the stream while jobs.list was in flight', async () => {
    scans.onEvent(ev('job.state', { job: job('running', { started_at: '2026-10-10T10:00:01Z' }) }));
    let answer!: (r: Reply) => void;
    const { client } = clientWith(() => new Promise<Reply>((resolve) => { answer = resolve; }));
    session.client = client;
    const syncing = scans.sync();
    scans.onEvent(ev('job.state', { job: job('completed', { coverage: 'complete', started_at: '2026-10-10T10:00:01Z', finished_at: '2026-10-10T10:00:05Z' }) }));
    answer({ jobs: [job('running', { started_at: '2026-10-10T10:00:01Z' })] }); // the list was read before the job ended
    await syncing;
    expect(scans.view('rt_a', 0)?.activity).toBe('completed');
  });

  it('does not claim a scan is running when the service is busy for another reason', async () => {
    const { client, calls } = clientWith(() => ({ error: { code: 'busy', message: 'too many mutations are pending; retry shortly' } }));
    session.client = client;
    await scans.start('rt_a', 'full');
    expect(scans.problems.rt_a).toContain('too many mutations are pending');
    expect(scans.problems.rt_a).not.toContain('already running');
    expect(calls).toEqual(['scan.start']); // no running job was named, so none is looked up
  });
});

describe('the shell resyncs on the events a scan produces', () => {
  const feed = (e: EventEnvelope) => (session as unknown as { onEvent(e: EventEnvelope): void }).onEvent(e);
  it.each(['tree.invalidated', 'roots.changed'] as const)('bumps the epoch on %s, which the Explorer and Atlas refetch on', (name) => {
    const before = session.epoch;
    feed(ev(name, name === 'roots.changed' ? { roots: [] } : { root_id: 'rt_a', generation: '2', scope: 'all', node_ids: [] }));
    expect(session.epoch).toBe(before + 1);
  });
});

describe('against the mock transport', () => {
  let transport: Transport;
  let events: EventEnvelope[];
  let stop: () => void;
  const call = async <C extends CommandKey>(command: C, payload: Record<string, unknown>) => {
    const r = await transport.call({ protocol: 'loomward/3', request_id: `r_${Math.random()}`, command, payload, deadline_ms: 5000 });
    return r;
  };
  const names = () => events.filter((e) => e.event !== 'stream.hello').map((e) => (e.event === 'job.state' ? `job:${(e.data as unknown as { job: Job }).job.state}` : e.event));

  beforeEach(() => {
    vi.useFakeTimers();
    fresh();
    transport = createMockTransport({ count: 200 });
    events = [];
    stop = transport.subscribe((e) => events.push(e), () => undefined);
  });
  afterEach(() => { stop(); vi.useRealTimers(); });

  it('runs a lab root queued, running, progress, tree.invalidated, completed, in that order', async () => {
    const r = await call('scan.start', { root_id: 'rt_mock_10', mode: 'full' });
    expect(r.ok && (r.result as unknown as { job: Job }).job.state).toBe('queued');
    await vi.advanceTimersByTimeAsync(10_000);
    expect(names()).toEqual(['job:queued', 'job:running', 'scan.progress', 'tree.invalidated', 'job:completed']);
    const seqs = events.filter((e) => e.event !== 'stream.hello').map((e) => e.seq);
    expect(seqs).toEqual([1, 2, 3, 4, 5]);
    const list = await call('roots.list', {});
    const root = list.ok && (list.result as unknown as { roots: { root_id: string; scan: { state: string; coverage: string } }[] }).roots.find((x) => x.root_id === 'rt_mock_10');
    expect(root).toMatchObject({ scan: { state: 'complete', coverage: 'complete' } });
  });

  it('reports the lab root with unreadable folders as partial, never complete', async () => {
    await call('scan.start', { root_id: 'rt_mock_11', mode: 'full' });
    await vi.advanceTimersByTimeAsync(10_000);
    const done = events.filter((e) => e.event === 'job.state').map((e) => (e.data as unknown as { job: Job }).job).at(-1)!;
    expect(done).toMatchObject({ state: 'completed', coverage: 'partial' });
    expect(done.progress?.failed).toBe(4);
  });

  it('refuses a fixture root with the service\'s reason, and an unknown root as not found', async () => {
    const r = await call('scan.start', { root_id: 'rt_mock_0', mode: 'full' });
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error).toMatchObject({ code: 'permission_denied', detail: { reason: 'synthetic_session_requires_lab_root' } });
    const u = await call('scan.start', { root_id: 'rt_nope', mode: 'full' });
    expect(u.ok === false && u.error.code).toBe('not_found');
    expect(events.filter((e) => e.event !== 'stream.hello')).toEqual([]);
  });

  it('answers a second start busy with the running job\'s id', async () => {
    const a = await call('scan.start', { root_id: 'rt_mock_10', mode: 'full' });
    const b = await call('scan.start', { root_id: 'rt_mock_10', mode: 'refresh' });
    const id = a.ok ? (a.result as unknown as { job: Job }).job.job_id : '';
    expect(b.ok === false && b.error).toMatchObject({ code: 'busy', detail: { job_id: id } });
  });

  it('cancels: cancel_requested at once, cancelled after, never completed; a repeat is harmless', async () => {
    const a = await call('scan.start', { root_id: 'rt_mock_10', mode: 'full' });
    const id = a.ok ? (a.result as unknown as { job: Job }).job.job_id : '';
    await vi.advanceTimersByTimeAsync(500);
    const c = await call('scan.cancel', { job_id: id });
    expect(c.ok && (c.result as unknown as { job: Job }).job.state).toBe('cancel_requested');
    await call('scan.cancel', { job_id: id });
    await vi.advanceTimersByTimeAsync(10_000);
    expect(names()).toEqual(['job:queued', 'job:running', 'job:cancel_requested', 'job:cancelled']);
    const again = await call('scan.cancel', { job_id: id });
    expect(again.ok && (again.result as unknown as { job: Job }).job.state).toBe('cancelled');
    const list = await call('roots.list', {});
    const root = list.ok && (list.result as unknown as { roots: { root_id: string; scan: { coverage: string } }[] }).roots.find((x) => x.root_id === 'rt_mock_10');
    expect(root && root.scan.coverage).not.toBe('complete');
  });

  it('drives the store end to end through the mock, including a refusal', async () => {
    session.client = new Client(transport);
    stop();
    stop = transport.subscribe((e) => scans.onEvent(e), () => undefined);
    await scans.start('rt_mock_0', 'full');
    expect(scans.problems.rt_mock_0).toContain('registered lab roots only');
    expect(scans.view('rt_mock_0', 0)).toBeNull();
    await scans.start('rt_mock_10', 'full');
    expect(scans.view('rt_mock_10', 0)?.activity).toBe('queued');
    await scans.start('rt_mock_10', 'full'); // a second start adopts the running job and says so
    expect(scans.problems.rt_mock_10).toContain('already running');
    await vi.advanceTimersByTimeAsync(500);
    await scans.cancel('rt_mock_10');
    expect(scans.view('rt_mock_10', 0)?.activity).toBe('cancelling');
    await vi.advanceTimersByTimeAsync(1000);
    expect(scans.view('rt_mock_10', 0)?.activity).toBe('cancelled');
    session.client = null;
  });
});
