import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CommandKey, ErrorCode, EventEnvelope, Job, JobState, SessionInfo } from '../../src/lib/contracts.gen';
import { scans } from '../../src/lib/stores/scan.svelte';
import { session } from '../../src/lib/stores/session.svelte';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { TransportError, type Transport } from '../../src/lib/transport/transport';

const job = (state: JobState, over: Partial<Job> = {}): Job => ({
  job_id: 'jb_1', seq: 1, rev: { queued: 0, running: 1, cancel_requested: 2, completed: 3, failed: 3, cancelled: 3 }[state], kind: 'scan', state, root_id: 'rt_a', created_at: '2026-10-10T10:00:00Z', started_at: null, finished_at: null, progress: null, coverage: null, error: null, ...over,
});
const ev = (event: EventEnvelope['event'], data: unknown): EventEnvelope =>
  ({ protocol: 'loomward/3', epoch: 'e1', seq: 1, event, at: '2026-10-10T10:00:00Z', catalog_rev: null, state_rev: null, data } as EventEnvelope);
const fresh = () => {
  scans.book = {}; scans.problems = {}; scans.pending = {}; scans.change = null;
  session.info = null;
  const epochs = scans as unknown as { streamEpoch: string | null; sessionStartedAt: string | null };
  epochs.streamEpoch = null; epochs.sessionStartedAt = null;
};

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
    scans.onEvent(ev('job.state', { job: job('completed', { coverage: 'complete', progress: p as Job['progress'], finished_at: '2026-10-10T10:00:05Z' }) }));
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

describe('scan requests stay inside their engine generation', () => {
  const oldJob = job('running', { job_id: 'jb_old', seq: 50 });
  const newJob = job('queued', { job_id: 'jb_new', seq: 1 });
  const hello = (epoch: string) => ({ ...ev('stream.hello', {}), epoch });
  const httpHello = () => {
    session.info = { session_started_at: '2026-10-10T11:00:00Z' } as SessionInfo;
    session.epoch++;
  };

  beforeEach(() => {
    fresh();
    session.info = { session_started_at: '2026-10-10T10:00:00Z' } as SessionInfo;
    scans.onEvent(hello('old_epoch'));
    scans.apply(oldJob);
  });
  afterEach(() => { session.client = null; session.info = null; session.state = 'connecting'; vi.clearAllTimers(); vi.useRealTimers(); });

  it.each(['sync', 'start', 'adopt'] as const)('clears old ordering on %s after HTTP hello and before SSE hello', async (operation) => {
    session.client = clientWith((c) => c === 'jobs.list' ? { jobs: [newJob] } :
      c === 'scan.start' && operation === 'adopt' ? { error: { code: 'busy', message: 'running', detail: { job_id: 'jb_new' } } } : { job: newJob }).client;
    httpHello();
    if (operation === 'sync') await scans.sync();
    else await scans.start('rt_a', 'full');
    expect(scans.book.rt_a?.job_id).toBe('jb_new');
    scans.onEvent(hello('new_epoch'));
    expect(scans.book.rt_a?.job_id).toBe('jb_new');
  });

  it.each((['jobs.list', 'jobs.get', 'scan.start', 'scan.cancel', 'adopt'] as const)
    .flatMap((command) => (['http', 'stream'] as const).map((via) => ({ command, via }))))(
    'drops an old $command response after a new epoch via $via', async ({ command, via }) => {
      let answer!: (reply: Reply) => void;
      session.client = clientWith((c) => {
        if (c === 'jobs.list' && command === 'jobs.get') return { jobs: [] };
        if (c === 'scan.start' && command === 'adopt') return { error: { code: 'busy', message: 'old scan', detail: { job_id: 'jb_old' } } };
        return new Promise<Reply>((resolve) => { answer = resolve; });
      }).client;
      const pending = command === 'jobs.list' || command === 'jobs.get' ? scans.sync() :
        command === 'scan.cancel' ? scans.cancel('rt_a') : scans.start('rt_a', 'full');
      await vi.waitFor(() => expect(answer).toBeTypeOf('function'));
      if (via === 'http') httpHello();
      else {
        scans.onEvent(hello('new_epoch'));
        scans.onEvent({ ...ev('job.state', { job: newJob }), epoch: 'new_epoch' });
      }
      // The old response itself must notice the HTTP epoch, without a preceding sync/event.
      answer(command === 'jobs.list' ? { jobs: [oldJob] } : { job: oldJob });
      await pending;
      expect(scans.book).toEqual(via === 'http' ? {} : { rt_a: newJob });
      expect(scans.problems).toEqual({});
      scans.onEvent(hello('new_epoch'));
      scans.onEvent({ ...ev('job.state', { job: newJob }), epoch: 'new_epoch' });
      expect(scans.book.rt_a?.job_id).toBe('jb_new');
    },
  );

  it.each(['jobs.list', 'jobs.get', 'scan.start', 'scan.cancel', 'adopt'] as const)(
    'drops an old %s rejection after an SSE epoch change', async (command) => {
      let answer!: (reply: Reply) => void;
      session.client = clientWith((c) => {
        if (c === 'jobs.list' && command === 'jobs.get') return { jobs: [] };
        if (c === 'scan.start' && command === 'adopt') return { error: { code: 'busy', message: 'old scan', detail: { job_id: 'jb_old' } } };
        return new Promise<Reply>((resolve) => { answer = resolve; });
      }).client;
      const pending = command === 'jobs.list' || command === 'jobs.get' ? scans.sync() :
        command === 'scan.cancel' ? scans.cancel('rt_a') : scans.start('rt_a', 'full');
      await vi.waitFor(() => expect(answer).toBeTypeOf('function'));
      scans.onEvent(hello('new_epoch'));
      scans.onEvent({ ...ev('job.state', { job: newJob }), epoch: 'new_epoch' });
      answer({ error: { code: 'internal_error', message: 'old engine refusal' } });
      await expect(pending).resolves.toBeUndefined();
      expect(scans.book.rt_a?.job_id).toBe('jb_new');
      expect(scans.problems).toEqual({});
    },
  );

  it.each(['tree.invalidated', 'roots.changed'] as const)('keeps same-engine replies across a %s view resync', async (event) => {
    const answers: ((reply: Reply) => void)[] = [];
    session.client = clientWith(() => new Promise<Reply>((resolve) => answers.push(resolve))).client;
    const refused = scans.start('rt_a', 'full');
    session.epoch++; // what session.onEvent does for a view resync
    scans.onEvent({ ...ev(event, {}), epoch: 'old_epoch' });
    answers[0]!({ error: { code: 'busy', message: 'already scanning' } });
    await refused;
    expect(scans.problems.rt_a).toBeTruthy();
    const started = scans.start('rt_a', 'full');
    session.epoch++;
    answers[1]!({ job: job('queued', { job_id: 'jb_next', seq: 51 }) });
    await started;
    expect(scans.book.rt_a?.job_id).toBe('jb_next');
  });

  it.each(['start', 'cancel'] as const)('still hands a transport fault to the shell after a new epoch (%s)', async (operation) => {
    session.state = 'connected';
    let fail!: (e: Error) => void;
    session.client = clientWith((c) => c === 'jobs.list' ? { jobs: [] } :
      new Promise<Reply>((_, reject) => { fail = reject; })).client;
    const pending = operation === 'cancel' ? scans.cancel('rt_a') : scans.start('rt_a', 'full');
    await vi.waitFor(() => expect(fail).toBeTypeOf('function'));
    vi.useFakeTimers();
    scans.onEvent(hello('new_epoch'));
    fail(new TransportError('engine went away'));
    await expect(pending).resolves.toBeUndefined();
    expect(session.state).toBe('unavailable');
  });

  it.each(['jobs.list', 'jobs.get'] as const)('drops a stale %s transport fault in sync without marking the shell unavailable', async (command) => {
    session.state = 'connected';
    let fail!: (e: Error) => void;
    session.client = clientWith((c) => c === 'jobs.list' && command === 'jobs.get' ? { jobs: [] } :
      new Promise<Reply>((_, reject) => { fail = reject; })).client;
    const pending = scans.sync();
    await vi.waitFor(() => expect(fail).toBeTypeOf('function'));
    vi.useFakeTimers();
    scans.onEvent(hello('new_epoch'));
    fail(new TransportError('engine went away'));
    await expect(pending).resolves.toBeUndefined();
    expect(session.state).toBe('connected');
    expect(scans.problems).toEqual({});
  });

  it.each(['jobs.list', 'jobs.get'] as const)('still hands a current-generation %s transport fault to the shell', async (command) => {
    vi.useFakeTimers();
    session.state = 'connected';
    session.client = clientWith((c) => {
      if (c === 'jobs.list' && command === 'jobs.get') return { jobs: [] };
      throw new TransportError('engine went away');
    }).client;
    await expect(scans.sync()).resolves.toBeUndefined();
    expect(session.state).toBe('unavailable');
  });

  it('still throws a current-generation jobs.list refusal from sync', async () => {
    session.client = clientWith(() => ({ error: { code: 'internal_error', message: 'old engine refusal' } })).client;
    await expect(scans.sync()).rejects.toMatchObject({ code: 'internal_error' });
  });

  it('hands a transport fault from adopt to the shell without a busy refusal', async () => {
    vi.useFakeTimers();
    session.state = 'connected';
    session.client = clientWith((c) => {
      if (c === 'scan.start') return { error: { code: 'busy', message: 'already scanning', detail: { job_id: 'jb_9' } } };
      if (c === 'jobs.get') throw new TransportError('engine went away');
      return { jobs: [] };
    }).client;
    await expect(scans.start('rt_a', 'full')).resolves.toBeUndefined();
    expect(session.state).toBe('unavailable');
    expect(scans.pending.rt_a).toBe(false);
  });

  it('does not let an old start clear the pending flag of a new-engine start', async () => {
    const answers: ((reply: Reply) => void)[] = [];
    session.client = clientWith(() => new Promise<Reply>((resolve) => answers.push(resolve))).client;
    const oldStart = scans.start('rt_a', 'full');
    scans.onEvent(hello('new_epoch'));
    const newStart = scans.start('rt_a', 'full');
    expect(answers).toHaveLength(2);
    answers[0]!({ job: oldJob });
    await oldStart;
    expect(scans.pending.rt_a).toBe(true);
    answers[1]!({ job: newJob });
    await newStart;
    expect(scans.pending.rt_a).toBe(false);
    expect(scans.book.rt_a?.job_id).toBe('jb_new');
  });
});

describe('settling and syncing keep what was known', () => {
  beforeEach(fresh);
  afterEach(() => { session.client = null; });

  it('discards job ordering across engine epochs', () => {
    scans.onEvent(ev('job.state', { job: job('completed', { seq: 50 }) }));
    scans.onEvent({ ...ev('stream.hello', {}), epoch: 'restarted_epoch' });
    expect(scans.book).toEqual({});
    scans.onEvent({ ...ev('job.state', { job: job('queued', { job_id: 'jb_new', seq: 1 }) }), epoch: 'restarted_epoch' });
    expect(scans.book.rt_a?.job_id).toBe('jb_new');
  });

  it('surfaces an omitted scan whose state cannot be confirmed, and clears the warning on confirmation', async () => {
    scans.apply(job('running'));
    let confirmed = false;
    const { client } = clientWith((c) => c === 'jobs.list' ? { jobs: [] } : confirmed ? { job: job('running') } :
      { error: { code: 'internal_error', message: 'Synthetic service unavailable' } });
    session.client = client;
    await scans.sync();
    expect(scans.problems.rt_a).toBe("Could not confirm this scan's state: Synthetic service unavailable");
    expect(scans.change?.text).toBe(scans.problems.rt_a);
    expect(scans.book.rt_a?.state).toBe('running'); // the failure is to confirm, not a reported scan failure
    confirmed = true;
    await scans.sync();
    expect(scans.problems.rt_a).toBeUndefined();
    scans.problems.rt_a = 'The scan could not be cancelled: Synthetic refusal';
    await scans.sync();
    expect(scans.problems.rt_a).toBe('The scan could not be cancelled: Synthetic refusal');
  });

  it('settles when a newer finished scan replaces an older finished one for the same root', () => {
    scans.onEvent(ev('job.state', { job: job('completed', { coverage: 'complete', finished_at: '2026-10-10T10:00:05Z' }) }));
    const settled = scans.settled;
    scans.onEvent(ev('job.state', { job: job('completed', { job_id: 'jb_2', seq: 2, created_at: '2026-10-10T11:00:00Z', coverage: 'partial', finished_at: '2026-10-10T11:00:05Z' }) }));
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

describe('the busy notice about an adopted scan does not outlive it', () => {
  beforeEach(fresh);
  afterEach(() => { session.client = null; });
  const running = job('running', { job_id: 'jb_9', started_at: '2026-10-10T10:00:01Z' });
  const ended = job('completed', { job_id: 'jb_9', started_at: '2026-10-10T10:00:01Z', finished_at: '2026-10-10T10:00:05Z', coverage: 'complete' });
  const busy = (answer: Reply) => clientWith((c) => c === 'scan.start' ? { error: { code: 'busy', message: 'running', detail: { job_id: 'jb_9' } } } : answer).client;

  it('says the scan is already running while it runs, then drops the notice when it ends', async () => {
    session.client = busy({ job: running });
    await scans.start('rt_a', 'full');
    expect(scans.view('rt_a', 0)?.active).toBe(true);
    expect(scans.problems.rt_a).toContain('already running');
    scans.onEvent(ev('job.state', { job: ended }));
    expect(scans.view('rt_a', 0)?.activity).toBe('completed');
    expect(scans.problems.rt_a).toBeUndefined();
  });

  it('does not report a scan that was already over by the time it was looked up as running', async () => {
    session.client = busy({ job: ended });
    await scans.start('rt_a', 'full');
    expect(scans.view('rt_a', 0)?.activity).toBe('completed');
    expect(scans.problems.rt_a).toBeUndefined();
  });

  it('keeps the notice when the lookup failed and the book still holds an older finished scan', async () => {
    scans.onEvent(ev('job.state', { job: job('completed', { job_id: 'jb_old', started_at: '2026-10-10T09:00:00Z', finished_at: '2026-10-10T09:00:05Z', coverage: 'complete' }) }));
    session.client = busy({ error: { code: 'not_found', message: 'gone' } });
    await scans.start('rt_a', 'full');
    expect(scans.problems.rt_a).toContain('already running');
  });

  it('keeps a refusal that is not about a running scan', async () => {
    session.client = clientWith((c) => c === 'scan.start' ? { error: { code: 'permission_denied', message: 'no grant' } } : null).client;
    await scans.start('rt_a', 'full');
    scans.onEvent(ev('job.state', { job: ended }));
    expect(scans.problems.rt_a).toContain('grant');
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
    expect(names()).toEqual(['job:queued', 'job:running', 'scan.progress', 'job:running', 'tree.invalidated', 'job:completed']);
    const seqs = events.filter((e) => e.event !== 'stream.hello').map((e) => e.seq);
    expect(seqs).toEqual([1, 2, 3, 4, 5, 6]);
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

  it('serves engine-like job counters through the composite mock transport', async () => {
    const c = new Client(transport);
    const first = (await c.call('scan.start', { root_id: 'rt_mock_10', mode: 'full' })).result.job;
    const second = (await c.call('scan.start', { root_id: 'rt_mock_11', mode: 'full' })).result.job;
    expect([first.seq, second.seq]).toEqual([1, 2]);
    expect(first.rev).toBe(0);
    await vi.advanceTimersByTimeAsync(10_000);
    const snapshots = events.filter((e) => e.event === 'job.state').map((e) => (e.data as unknown as { job: Job }).job).filter((j) => j.job_id === first.job_id);
    expect(snapshots.map((j) => j.rev)).toEqual([0, 1, 2, 3]);
    expect(snapshots.every((j) => j.seq === first.seq)).toBe(true);
    expect((await c.call('jobs.get', { job_id: first.job_id })).result.job).toEqual(snapshots.at(-1));
    expect((await c.call('jobs.list', { limit: 50 })).result.jobs.map((j) => j.seq)).toEqual([2, 1]);
    expect((await c.call('scan.cancel', { job_id: first.job_id })).result.job.rev).toBe(3);
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
