import { afterEach, describe, expect, it, vi } from 'vitest';
import type { CoverageReport, EventEnvelope, Job } from '../../src/lib/contracts.gen';
import { Covers } from '../../src/lib/stores/covers.svelte';
import { session } from '../../src/lib/stores/session.svelte';
import { Client, LoomwardError } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { bytesText, coversRefusal, explorerLink, headline, reasonText, statusText, totalRows } from '../../src/views/duplicates/covers-model';

const total = (files: number, bytes: string, size_unknown = 0) => ({ files, bytes, size_unknown });
const report = (patch: Partial<CoverageReport> = {}): CoverageReport => ({
  job_id: 'jb_1', a_root_id: 'rt_a', b_root_id: 'rt_b', outcome: 'complete', reasons: [], bytes_read: '2048',
  totals: { covered: total(3, '3072'), not_covered: total(0, '0'), unknown: total(0, '0') },
  not_covered: [], not_covered_truncated: false, unknown: [], unknown_truncated: false, observed_at: '2026-10-30T09:00:00Z', note: 'n', ...patch,
});
const job = (patch: Partial<Job> = {}): Job => ({ job_id: 'jb_1', kind: 'coverage', root_id: 'rt_a', seq: 1, rev: 0, state: 'running', created_at: '2026-10-30T09:00:00Z', started_at: '2026-10-30T09:00:00Z', finished_at: null, coverage: null, error: null, progress: null, ...patch });
const error = (code: LoomwardError['code'], message: string, detail: Record<string, unknown> | null = null) => new LoomwardError({ code, message, retryable: false, detail });
afterEach(() => { session.client = null; session.info = null; vi.useRealTimers(); });

describe('covers wording', () => {
  it('says "every file covered" only for a complete run with nothing missing or unknown', () => {
    const h = headline(report());
    expect(h.tone).toBe('complete');
    expect(h.heading).toBe('Every file of the first folder was covered by the second when read.');
    expect(h.detail).toContain('3 files (3.00 KiB)');
  });
  it('names the files that were not found, as a finished answer', () => {
    const h = headline(report({ totals: { covered: total(2, '10'), not_covered: total(1, '9437184'), unknown: total(0, '0') } }));
    expect(h.tone).toBe('complete');
    expect(h.heading).toBe('1 file of the first folder was not found in the second.');
    expect(h.detail).toContain('9.00 MiB');
  });
  it('never reads a partial run as a clean verdict in either direction', () => {
    const partial = report({
      outcome: 'partial', reasons: ['budget', 'changed_during_read'],
      totals: { covered: total(5, '50'), not_covered: total(0, '0'), unknown: total(2, '10', 1) },
    });
    const h = headline(partial);
    expect(h.tone).toBe('partial');
    expect(h.heading).toBe('Partial result: this is not a complete answer.');
    expect(h.detail).toContain('2 files could not be judged (the read budget ran out; a file changed while it was being read)');
    expect(h.heading + h.detail).not.toMatch(/every file|all covered|safe to delete/i);
    // Even a payload that claims complete cannot show a clean heading while something is unknown.
    expect(headline({ ...partial, outcome: 'complete', reasons: [] }).tone).toBe('partial');
  });
  it('keeps unknown bytes a lower bound and an unreadable total "unknown", never zero', () => {
    expect(bytesText('1024', 0)).toBe('1.00 KiB');
    expect(bytesText('1024', 3)).toBe('at least 1.00 KiB (3 files of unknown size)');
    expect(bytesText('1024', 1)).toContain('1 file of unknown size');
    expect(bytesText('not a number', 0)).toBe('unknown');
    expect(totalRows(report()).map((r) => r.label)).toEqual(['Covered by content', 'Not found in the second folder', 'Unknown']);
  });
  it('gives plain reasons and a catalogue link only where the catalogue has the file', () => {
    expect(reasonText('byte_budget')).toBe('byte budget spent');
    expect(reasonText('some_new_reason')).toBe('some new reason');
    expect(explorerLink('nd_a b')).toBe('#/explorer?node=nd_a%20b');
    expect(explorerLink(null)).toBeNull();
  });
  it('reports each job state in plain words and never promises a result a job does not have', () => {
    const at = Date.parse('2026-10-30T09:00:07Z');
    expect(statusText(null, null, true, at, 60)).toBe('Sending request…');
    expect(statusText(job(), null, false, at, 60)).toContain('7 s so far, at most 60 s');
    expect(statusText(job({ state: 'cancelled' }), null, false, at, 60)).toContain('Cancelled. No verdict was produced and nothing was changed. The read may still be finishing in the background');
    expect(statusText(job({ state: 'failed', error: { code: 'permission_denied', message: 'entry_budget', retryable: false, detail: null } }), null, false, at, 60)).toBe('Failed: entry_budget. No verdict was produced.');
    expect(statusText(job({ state: 'completed' }), null, false, at, 60)).toContain('no longer available');
    expect(statusText(job({ state: 'completed' }), report(), false, at, 60)).toBe('');
  });
  it('translates refusals', () => {
    expect(coversRefusal(error('busy', 'x'))).toContain('already running');
    expect(coversRefusal(error('busy', 'x', { reason: 'read_still_finishing' }))).toContain('still finishing its read in the background');
    expect(coversRefusal(error('invalid_request', 'x', { reason: 'roots_must_differ' }))).toBe('Choose two different folders.');
    expect(coversRefusal(error('invalid_request', 'x', { reason: 'roots_overlap' }))).toContain('inside the other');
    expect(coversRefusal(error('permission_denied', 'grant_not_active'))).toContain('Nothing was read');
    expect(coversRefusal(error('internal_error', 'plain'))).toBe('plain');
  });
});

async function mock() {
  vi.useFakeTimers();
  const transport = createMockTransport({ count: 32 });
  const client = new Client(transport);
  session.client = client;
  const store = new Covers();
  transport.subscribe((e) => store.onEvent(e), () => {});
  return { client, store, transport };
}

describe('covers store', () => {
  it('keeps a failed result readable again after a successful job-list resync', async () => {
    const finished = job({ state: 'completed' });
    const call = vi.fn(async (command: string) => {
      if (command === 'jobs.list') return { result: { jobs: [finished] } };
      throw error('internal_error', 'Synthetic result read failed');
    });
    session.client = { call } as unknown as Client;
    const store = new Covers();
    store.apply(finished);
    await vi.waitFor(() => expect(store.canRetryResult).toBe(true));
    await store.sync();
    expect(store.problem).toBe('Synthetic result read failed');
    expect(store.canRetryResult).toBe(true);
    expect(call.mock.calls.map(([command]) => command)).toEqual(['coverage.result', 'jobs.list']);
  });

  it('retries a failed result read without starting another content check', async () => {
    const call = vi.fn().mockRejectedValueOnce(error('internal_error', 'Synthetic result read failed')).mockResolvedValue({ result: report() });
    session.client = { call } as unknown as Client;
    const store = new Covers();
    store.apply(job({ state: 'completed' }));
    await vi.waitFor(() => expect(store.problem).toBe('Synthetic result read failed'));
    expect(store.canRetryResult).toBe(true);
    await store.retryResult();
    expect(store.report).toEqual(report());
    expect(store.problem).toBe('');
    expect(store.canRetryResult).toBe(false);
    expect(call.mock.calls.map(([command]) => command)).toEqual(['coverage.result', 'coverage.result']);
  });

  it('coalesces repeated result retries and never retries an expired result', async () => {
    let resolve!: (value: { result: CoverageReport }) => void;
    const pending = new Promise<{ result: CoverageReport }>(r => { resolve = r; });
    const call = vi.fn().mockRejectedValueOnce(error('internal_error', 'Synthetic result read failed')).mockReturnValueOnce(pending);
    session.client = { call } as unknown as Client;
    const store = new Covers();
    store.apply(job({ state: 'completed' }));
    await vi.waitFor(() => expect(store.problem).toBe('Synthetic result read failed'));
    const retry = store.retryResult();
    await store.retryResult();
    expect(store.resultPending).toBe(true);
    expect(call).toHaveBeenCalledTimes(2);
    resolve({ result: report() }); await retry;
    expect(store.resultPending).toBe(false);
    const expired = new Covers();
    call.mockRejectedValueOnce(error('not_found', 'Synthetic expired result'));
    expired.apply(job({ state: 'completed' }));
    await vi.waitFor(() => expect(expired.resultPending).toBe(false));
    const before = call.mock.calls.length;
    await expired.retryResult();
    expect(expired.canRetryResult).toBe(false);
    expect(call).toHaveBeenCalledTimes(before);
    expect(expired.report).toBeNull();
  });

  it('drops an old result-read failure after a newer coverage job succeeds', async () => {
    let reject!: (error: Error) => void;
    const pending = new Promise<never>((_, fail) => { reject = fail; });
    const fresh = report({ job_id: 'jb_2' });
    const call = vi.fn().mockReturnValueOnce(pending).mockResolvedValue({ result: fresh });
    session.client = { call } as unknown as Client;
    const store = new Covers();
    store.apply(job({ state: 'completed' }));
    store.apply(job({ job_id: 'jb_2', seq: 2, state: 'completed' }));
    await vi.waitFor(() => expect(store.report).toEqual(fresh));
    reject(error('internal_error', 'Obsolete result failure'));
    await Promise.resolve(); await Promise.resolve();
    expect(store.problem).toBe('');
    expect(store.report).toEqual(fresh);
    expect(store.canRetryResult).toBe(false);
  });

  it('starts, follows the job and fetches the report once it completes', async () => {
    const { store } = await mock();
    await store.start('rt_mock_10', 'rt_mock_11', { bytes: '1073741824', secs: 60 });
    expect(store.active).toBe(true);
    expect(store.report).toBeNull();
    await vi.advanceTimersByTimeAsync(2500);
    expect(store.job).toMatchObject({ kind: 'coverage', state: 'completed', root_id: 'rt_mock_10' });
    expect(store.report).toMatchObject({ job_id: store.job!.job_id, outcome: 'partial', reasons: ['budget'], a_root_id: 'rt_mock_10', b_root_id: 'rt_mock_11' });
    expect(store.report!.not_covered[0]!.node_id).toMatch(/^nd_/);
    // Covered files are counts only.
    expect(JSON.stringify(store.report)).not.toContain('"covered":[');
  });
  it('shows a finished answer when the other direction was read completely', async () => {
    const { store } = await mock();
    await store.start('rt_mock_11', 'rt_mock_10');
    await vi.advanceTimersByTimeAsync(2500);
    expect(store.report).toMatchObject({ outcome: 'complete', reasons: [], unknown: [] });
    expect(headline(store.report!).tone).toBe('complete');
  });
  it('cancels, keeps no report and says so', async () => {
    const { client, store } = await mock();
    await store.start('rt_mock_10', 'rt_mock_11');
    const id = store.job!.job_id;
    await store.cancel();
    expect(store.job).toMatchObject({ job_id: id, state: 'cancel_requested' });
    await vi.advanceTimersByTimeAsync(2500);
    expect(store.job?.state).toBe('cancelled');
    expect(store.report).toBeNull();
    await expect(client.call('coverage.result', { job_id: id })).rejects.toMatchObject({ code: 'not_found' });
  });
  it('is refused for the same folder twice, an unknown folder and a second concurrent check', async () => {
    const { client, store } = await mock();
    await expect(client.call('coverage.start', { a_root_id: 'rt_mock_10', b_root_id: 'rt_mock_10' })).rejects.toMatchObject({ code: 'invalid_request', detail: { reason: 'roots_must_differ' } });
    await expect(client.call('coverage.start', { a_root_id: 'rt_mock_10', b_root_id: 'rt_mock_99' })).rejects.toMatchObject({ code: 'permission_denied' });
    await expect(client.call('coverage.start', { a_root_id: 'rt_mock_10', b_root_id: 'rt_mock_11', budget_secs: 0 })).rejects.toMatchObject({ code: 'invalid_request' });
    await store.start('rt_mock_10', 'rt_mock_11');
    await store.start('rt_mock_11', 'rt_mock_10'); // ignored while one is active
    await expect(client.call('coverage.start', { a_root_id: 'rt_mock_11', b_root_id: 'rt_mock_10' })).rejects.toMatchObject({ code: 'busy' });
    expect(store.job?.root_id).toBe('rt_mock_10');
  });
  it('drops the report when a folder\'s grant ends', async () => {
    const { client, store } = await mock();
    await store.start('rt_mock_10', 'rt_mock_11');
    await vi.advanceTimersByTimeAsync(2500);
    const id = store.job!.job_id;
    await client.call('roots.revoke', { root_id: 'rt_mock_11', purge_catalog: false });
    await expect(client.call('coverage.result', { job_id: id })).rejects.toMatchObject({ code: 'not_found' });
  });
  it('clears a cached report on resync once a folder grant has ended', async () => {
    const { client, store } = await mock();
    await store.start('rt_mock_10', 'rt_mock_11');
    await vi.advanceTimersByTimeAsync(2500);
    expect(store.report?.not_covered.length).toBeGreaterThan(0);
    await store.sync();
    expect(store.report).not.toBeNull(); // still granted: the report stands
    await client.call('roots.revoke', { root_id: 'rt_mock_11', purge_catalog: false });
    expect(store.report).not.toBeNull(); // nothing has asked the service yet
    await store.sync();
    expect(store.report).toBeNull();
    expect(store.job).toBeNull();
  });
  it('ignores stale snapshots and other kinds, and forgets everything when the engine restarts', async () => {
    const { store } = await mock();
    store.apply(job({ rev: 3, state: 'cancelled' }));
    store.apply(job({ rev: 1, state: 'running' }));
    store.apply(job({ kind: 'duplicates', seq: 9 }));
    expect(store.job).toMatchObject({ rev: 3, state: 'cancelled' });
    store.onEvent({ event: 'stream.hello', epoch: 'one' } as EventEnvelope);
    store.onEvent({ event: 'stream.hello', epoch: 'two' } as EventEnvelope);
    expect(store.job).toBeNull();
  });
});
