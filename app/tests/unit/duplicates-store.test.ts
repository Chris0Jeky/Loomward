import { afterEach, describe, expect, it, vi } from 'vitest';
import { Duplicates } from '../../src/lib/stores/duplicates.svelte';
import { session } from '../../src/lib/stores/session.svelte';
import { Client } from '../../src/lib/transport/client';
import { TransportError } from '../../src/lib/transport/transport';
import { createMockTransport } from '../../src/lib/transport/mock';
import { DUPLICATES_FIXTURE } from '../../src/lib/transport/mock-scan';
import type { EventEnvelope, Job } from '../../src/lib/contracts.gen';
import type { Transport } from '../../src/lib/transport/transport';

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
