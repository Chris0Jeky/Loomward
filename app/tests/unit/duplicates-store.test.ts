import { readFileSync } from 'node:fs';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { Duplicates } from '../../src/lib/stores/duplicates.svelte';
import { session } from '../../src/lib/stores/session.svelte';
import { Client } from '../../src/lib/transport/client';
import { TransportError } from '../../src/lib/transport/transport';
import { createMockTransport } from '../../src/lib/transport/mock';
import { DUPLICATES_FIXTURE } from '../../src/lib/transport/mock-scan';
import type { DuplicateReport, EventEnvelope, Job } from '../../src/lib/contracts.gen';
import type { Transport } from '../../src/lib/transport/transport';
import { sanitizeDuplicatesReport, skippedNotice } from '../../src/views/duplicates/View.svelte';

const job = (patch: Partial<Job> = {}): Job => ({ job_id: 'jb_fixture', kind: 'duplicates', root_id: 'rt_fixture', seq: 1, rev: 0, state: 'queued', created_at: '2026-10-01T00:00:00Z', started_at: null, finished_at: null, coverage: null, error: null, progress: null, ...patch });
afterEach(() => { session.client = null; vi.useRealTimers(); });

describe('duplicates jobs', () => {
  it('validates optional min_bytes as an exact unsigned byte count', async () => {
    for (const min_bytes of [null, -1, 4, '-1', '01', '', '18446744073709551616', '9'.repeat(21)]) {
      const transport = createMockTransport({ count: 32 });
      await expect(transport.call({ protocol: 'loomward/3', request_id: 'rq_fixture', command: 'duplicates.inspect', payload: { root_id: 'rt_mock_10', budget: { max_candidates: 5, max_bytes: '100', max_ms: 1000, min_bytes } } })).resolves.toMatchObject({ ok: false, error: { code: 'invalid_request' } });
    }
    vi.useFakeTimers();
    for (const min_bytes of [undefined, '0', '4', '18446744073709551615']) {
      const client = new Client(createMockTransport({ count: 32 }));
      await expect(client.call('duplicates.inspect', { root_id: 'rt_mock_10', budget: { max_candidates: 5, max_bytes: '100', max_ms: 1000, ...(min_bytes === undefined ? {} : { min_bytes }) } })).resolves.toBeDefined();
    }
  });
  it('retains terminal evidence and ignores stale snapshots and other kinds', () => {
    const store = new Duplicates();
    const terminal = job({ rev: 3, state: 'completed', duplicates: DUPLICATES_FIXTURE });
    store.apply(terminal);
    store.apply(job({ rev: 1, state: 'running' }));
    store.apply(job({ seq: 0, job_id: 'jb_old', rev: 20 }));
    store.apply(job({ kind: 'scan', seq: 2 }));
    expect(store.book.rt_fixture).toEqual(terminal);
    store.onEvent({ event: 'stream.hello', epoch: 'one' } as EventEnvelope);
    store.onEvent({ event: 'stream.hello', epoch: 'two' } as EventEnvelope);
    expect(store.book).toEqual({});
  });
  it('starts once, receives the mock fixture and cancels the existing job', async () => {
    vi.useFakeTimers();
    const transport = createMockTransport({ count: 32 });
    session.client = new Client(transport);
    const store = new Duplicates();
    transport.subscribe((e) => store.onEvent(e), () => {});
    await store.start('rt_mock_10');
    expect(Object.keys(store.book)).toEqual(['rt_mock_10']);
    await vi.advanceTimersByTimeAsync(1500);
    expect(store.book.rt_mock_10?.duplicates).toEqual(DUPLICATES_FIXTURE);
    await store.start('rt_mock_10');
    const id = store.book.rt_mock_10!.job_id;
    await store.cancel('rt_mock_10');
    expect(store.book.rt_mock_10).toMatchObject({ job_id: id, state: 'cancel_requested' });
    await vi.advanceTimersByTimeAsync(700);
    expect(store.book.rt_mock_10?.state).toBe('cancelled');
  });
  it('revokes completed duplicate reports through roots and grants and emits a newer snapshot', async () => {
    vi.useFakeTimers();
    for (const viaGrant of [false, true]) {
      const transport = createMockTransport({ count: 32 });
      const client = new Client(transport);
      const events: Job[] = [];
      transport.subscribe((e) => { if (e.event === 'job.state') events.push((e.data as unknown as { job: Job }).job); }, () => {});
      const started = await client.call('duplicates.inspect', { root_id: 'rt_mock_10', budget: { max_candidates: 5, max_bytes: '100', max_ms: 1000 } });
      const job_id = started.result.job.job_id;
      await vi.advanceTimersByTimeAsync(1500);
      const completed = (await client.call('jobs.get', { job_id })).result.job;
      expect(completed.duplicates?.entries.length).toBe(5);
      expect((await client.call('duplicates.cancel', { job_id })).result.job.duplicates).toBeUndefined();
      if (viaGrant) await client.call('grants.revoke', { grant_id: 'gr_mock_lab_0' });
      else await client.call('roots.revoke', { root_id: 'rt_mock_10', purge_catalog: false });
      const redacted = (await client.call('jobs.get', { job_id })).result.job;
      expect(redacted.duplicates).toBeUndefined();
      expect(redacted.rev).toBeGreaterThan(completed.rev);
      expect(events.at(-1)).toEqual(redacted);
      expect((await client.call('jobs.list', { kinds: ['duplicates'], limit: 50 })).result.jobs[0]?.duplicates).toBeUndefined();
    }
  });
  it('drops a jobs.list answer that arrives after the engine restarted', async () => {
    let requestId = '';
    let answer!: () => void;
    const listed = new Promise<void>((resolve) => { answer = resolve; });
    session.client = new Client({ call: async (r: { request_id: string; command: string }) => {
      requestId = r.request_id;
      await listed;
      return { protocol: 'loomward/3', request_id: r.request_id, ok: true, result: { jobs: [job({ rev: 3, state: 'completed', duplicates: DUPLICATES_FIXTURE })] }, meta: {} };
    } } as unknown as Transport);
    const store = new Duplicates();
    store.onEvent({ event: 'stream.hello', epoch: 'one' } as EventEnvelope);
    const syncing = store.sync();
    await vi.waitFor(() => expect(requestId).not.toBe(''));
    store.onEvent({ event: 'stream.hello', epoch: 'two' } as EventEnvelope); // the engine restarted: the old lifetime is gone
    answer();
    await syncing;
    expect(store.book).toEqual({});
  });
  it('drops inspect and cancel answers that arrive after the engine restarted', async () => {
    for (const command of ['duplicates.inspect', 'duplicates.cancel']) {
      let asked = false;
      let answer!: () => void;
      const answered = new Promise<void>((resolve) => { answer = resolve; });
      session.client = new Client({ call: async (r: { request_id: string; command: string }) => {
        asked = true;
        await answered;
        return { protocol: 'loomward/3', request_id: r.request_id, ok: true, result: { job: job({ rev: 9, state: 'running' }) }, meta: {} };
      } } as unknown as Transport);
      const store = new Duplicates();
      store.onEvent({ event: 'stream.hello', epoch: 'one' } as EventEnvelope);
      if (command === 'duplicates.cancel') store.book = { [job().root_id as string]: job({ rev: 1, state: 'running' }) };
      const pending = command === 'duplicates.inspect' ? store.start(job().root_id as string) : store.cancel(job().root_id as string);
      await vi.waitFor(() => expect(asked).toBe(true));
      store.onEvent({ event: 'stream.hello', epoch: 'two' } as EventEnvelope); // the engine restarted: the old lifetime is gone
      answer();
      await pending;
      expect(store.book, command).toEqual({});
      expect(store.pending).toBe(false);
    }
  });
  it('shows a root refusal as a problem without inventing a result', async () => {
    // q-10: personal reads are open, so no refusal is a special gated state any more.
    session.client = new Client({ call: async (r: { request_id: string }) => ({ protocol: 'loomward/3', request_id: r.request_id, ok: false, error: { code: 'permission_denied', message: 'a personal session scans the folders you granted only', retryable: false, detail: { reason: 'personal_session_requires_owner_grant' } } }) } as unknown as Transport);
    const store = new Duplicates();
    await store.start('rt_fixture');
    expect(store.problem).toBe('a personal session scans the folders you granted only');
    expect(store.book).toEqual({});
    expect('gated' in store).toBe(false);
  });
  it('keeps a busy follow-up failure as a problem without rejecting', async () => {
    vi.useFakeTimers();
    try {
      session.state = 'connected';
      session.client = new Client({ call: async (r: { request_id: string; command: string }) => {
        if (r.command === 'jobs.get') throw new TransportError('engine went away');
        return { protocol: 'loomward/3', request_id: r.request_id, ok: false, error: { code: 'busy', message: 'already running', retryable: false, detail: { job_id: 'jb_9' } } };
      } } as unknown as Transport);
      const store = new Duplicates();
      await store.start('rt_fixture');
      expect(store.problem).toBe('engine went away');
      expect(session.state).toBe('unavailable');
      expect(store.pending).toBe(false);

      session.state = 'connected';
      session.client = new Client({ call: async (r: { request_id: string; command: string }) => {
        if (r.command === 'jobs.get') return { protocol: 'loomward/3', request_id: r.request_id, ok: false, error: { code: 'not_found', message: 'job gone', retryable: false, detail: null } };
        return { protocol: 'loomward/3', request_id: r.request_id, ok: false, error: { code: 'busy', message: 'already running', retryable: false, detail: { job_id: 'jb_9' } } };
      } } as unknown as Transport);
      const retry = new Duplicates();
      await retry.start('rt_fixture');
      expect(retry.problem).toBe('job gone');
      expect(session.state).toBe('connected');
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('duplicates root picker', () => {
  it('names each root through the hidden-character helper, never the raw display path', () => {
    // A native <option> holds text only, so the badge component cannot go in it: escapedName spells the hidden characters out.
    const source = readFileSync(new URL('../../src/views/duplicates/View.svelte', import.meta.url), 'utf8');
    expect(source).toContain('escapedName(r.display_path.text)');
    expect(source).not.toMatch(/<option[^>]*>\{r\.display_path\.text\}<\/option>/);
  });
});

describe('duplicates report index guard', () => {
  const baseReport = (patch: Partial<DuplicateReport> = {}): DuplicateReport => ({
    entries: [{ relative_path: 'a.txt', logical_bytes: '16' }],
    groups: [],
    links: [],
    links_not_compared: 0,
    skipped: [],
    bytes_read: '16',
    partial: false,
    truncated: false,
    stop: null,
    ...patch,
  });
  const renderGuarded = (report: DuplicateReport | undefined) => {
    const guarded = sanitizeDuplicatesReport(report);
    // Mirror the View.svelte render path: every kept index must resolve to an entry.
    for (const group of guarded.report?.groups ?? []) {
      for (const i of group) void guarded.report!.entries[i]!.relative_path;
      void guarded.report!.entries[group[0]!]!.logical_bytes;
    }
    for (const links of guarded.report?.links ?? []) {
      for (const i of links) void guarded.report!.entries[i]!.relative_path;
    }
    for (const skip of guarded.report?.skipped ?? []) {
      void guarded.report!.entries[skip.entry]!.relative_path;
    }
    return guarded;
  };
  it('skips an out-of-range group index without throwing', () => {
    const store = new Duplicates();
    store.apply(job({ state: 'completed', duplicates: baseReport({ groups: [[999]] }) }));
    const report = store.book.rt_fixture?.duplicates;
    let guarded: ReturnType<typeof sanitizeDuplicatesReport> | undefined;
    expect(() => { guarded = renderGuarded(report); }).not.toThrow();
    expect(guarded?.report?.groups).toEqual([]);
    expect(guarded?.problem || 'empty').toBeTruthy();
    expect(guarded?.problem).toBeTruthy();
  });
  it('skips an out-of-range link index without throwing', () => {
    const store = new Duplicates();
    store.apply(job({ state: 'completed', duplicates: baseReport({ links: [[999]] }) }));
    const report = store.book.rt_fixture?.duplicates;
    let guarded: ReturnType<typeof sanitizeDuplicatesReport> | undefined;
    expect(() => { guarded = renderGuarded(report); }).not.toThrow();
    expect(guarded?.report?.links).toEqual([]);
    expect(guarded?.problem || 'empty').toBeTruthy();
    expect(guarded?.problem).toBeTruthy();
  });
  it('skips an out-of-range skipped index without throwing', () => {
    const store = new Duplicates();
    store.apply(job({ state: 'completed', duplicates: baseReport({ skipped: [{ entry: 999, reason: 'changed_during_read' }] }) }));
    const report = store.book.rt_fixture?.duplicates;
    let guarded: ReturnType<typeof sanitizeDuplicatesReport> | undefined;
    expect(() => { guarded = renderGuarded(report); }).not.toThrow();
    expect(guarded?.report?.skipped).toEqual([]);
    expect(guarded?.problem || 'empty').toBeTruthy();
    expect(guarded?.problem).toBeTruthy();
  });
  const twoEntries = (patch: Partial<DuplicateReport> = {}): DuplicateReport => baseReport({
    entries: [{ relative_path: 'a.txt', logical_bytes: '16' }, { relative_path: 'b.txt', logical_bytes: '16' }],
    ...patch,
  });
  it('withholds a verified group that lost its only other member to an invalid reference', () => {
    const guarded = sanitizeDuplicatesReport(twoEntries({ groups: [[0, 999]] }));
    expect(guarded.report?.groups).toEqual([]);
    expect(guarded.withheldGroups).toBe(1);
    expect(guarded.problem).toContain('1 verified group was withheld as incomplete.');
  });
  it('withholds a mixed-reference group but keeps an intact group beside it', () => {
    const guarded = sanitizeDuplicatesReport(twoEntries({ groups: [[0, 1], [0, 999]] }));
    expect(guarded.report?.groups).toEqual([[0, 1]]);
    expect(guarded.withheldGroups).toBe(1);
    expect(guarded.problem).toContain('withheld as incomplete');
  });
  it('keeps an all-valid verified group unchanged and withholds nothing', () => {
    const report = twoEntries({ groups: [[0, 1]] });
    const guarded = sanitizeDuplicatesReport(report);
    expect(guarded.report).toEqual(report);
    expect(guarded.withheldGroups).toBe(0);
    expect(guarded.problem).toBe('');
  });
  it('withholds a mixed-reference link set and counts it, keeping an intact link set beside it', () => {
    const guarded = sanitizeDuplicatesReport(twoEntries({ links: [[0, 1], [0, 999]] }));
    expect(guarded.report?.links).toEqual([[0, 1]]);
    expect(guarded.withheldLinks).toBe(1);
    expect(guarded.problem).toContain('1 link set was withheld as incomplete.');
  });
  it('withholds a link set that lost its only other member to an invalid reference', () => {
    const guarded = sanitizeDuplicatesReport(twoEntries({ links: [[0, 999]] }));
    expect(guarded.report?.links).toEqual([]);
    expect(guarded.withheldLinks).toBe(1);
    expect(guarded.problem).toContain('1 link set was withheld as incomplete.');
  });
  it('withholds a link set that ends with fewer than two members, and a link set that is not a list', () => {
    const guarded = sanitizeDuplicatesReport(twoEntries({ links: [[0], 'x' as unknown as number[]] }));
    expect(guarded.report?.links).toEqual([]);
    expect(guarded.withheldLinks).toBe(2);
    expect(guarded.problem).toContain('2 link sets were withheld as incomplete.');
  });
  it('keeps all-valid link sets unchanged and withholds nothing', () => {
    const guarded = sanitizeDuplicatesReport(twoEntries({ links: [[0, 1]] }));
    expect(guarded.report?.links).toEqual([[0, 1]]);
    expect(guarded.withheldLinks).toBe(0);
    expect(guarded.problem).toBe('');
  });
  it('a link list that is not a list reads as unknown, not empty', () => {
    const guarded = sanitizeDuplicatesReport(twoEntries({ links: 'x' as unknown as number[][] }));
    expect(guarded.report?.links).toEqual([]);
    expect(guarded.withheldLinks).toBe(0);
    expect(guarded.problem).toContain('The link sets could not be read, so they are unknown.');
  });
  it('renders a fully valid report exactly as before', () => {
    const report = twoEntries({ groups: [[0, 1]], links: [[0, 1]], skipped: [{ entry: 1, reason: 'changed_during_read' }] });
    const guarded = sanitizeDuplicatesReport(report);
    expect(guarded.report).toEqual(report);
    expect(guarded.problem).toBe('');
    expect(guarded.unreadableSkipped).toBe(0);
    expect(skippedNotice(guarded.unreadableSkipped, guarded.report!.skipped.length)).toBe('');
  });
  it('says unreadable skipped evidence is unknown when none survive, never the empty-state text', () => {
    const guarded = sanitizeDuplicatesReport(baseReport({ skipped: [{ entry: 999, reason: 'changed_during_read' }] }));
    expect(guarded.report?.skipped).toEqual([]);
    expect(guarded.unreadableSkipped).toBe(1);
    const notice = skippedNotice(guarded.unreadableSkipped, 0);
    expect(notice).toBe('1 skipped record could not be read, so its reason is unknown.');
    expect(notice).not.toContain('No listed entries were skipped');
  });
  it('a skipped list or group list that is not a list reads as unknown, not empty', () => {
    const guarded = sanitizeDuplicatesReport(twoEntries({ groups: 'x' as unknown as number[][], skipped: 'x' as unknown as [] }));
    expect(guarded.skippedUnknown).toBe(true);
    expect(skippedNotice(guarded.unreadableSkipped, guarded.report!.skipped.length, guarded.skippedUnknown)).toBe(
      'The skipped list could not be read, so what was skipped is unknown.',
    );
    expect(guarded.problem).toContain('The verified groups could not be read, so they are unknown.');
  });

  it('lists surviving skipped entries beside the unreadable count', () => {
    const guarded = sanitizeDuplicatesReport(twoEntries({ skipped: [{ entry: 1, reason: 'changed_during_read' }, { entry: 999, reason: 'changed_during_read' }] }));
    expect(guarded.report?.skipped).toEqual([{ entry: 1, reason: 'changed_during_read' }]);
    expect(guarded.unreadableSkipped).toBe(1);
    expect(skippedNotice(guarded.unreadableSkipped, guarded.report!.skipped.length)).toBe('1 skipped record could not be read, so its reason is unknown.');
    const plural = sanitizeDuplicatesReport(baseReport({ skipped: [{ entry: 999, reason: 'changed_during_read' }, { entry: 998, reason: 'changed_during_read' }] }));
    expect(skippedNotice(plural.unreadableSkipped, 0)).toBe('2 skipped records could not be read, so their reasons are unknown.');
  });
});
