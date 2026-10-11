import { afterEach, describe, expect, it, vi } from 'vitest';
import { Duplicates } from '../../src/lib/stores/duplicates.svelte';
import { session } from '../../src/lib/stores/session.svelte';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { DUPLICATES_FIXTURE } from '../../src/lib/transport/mock-scan';
import type { EventEnvelope, Job } from '../../src/lib/contracts.gen';
import type { Transport } from '../../src/lib/transport/transport';

const job = (patch: Partial<Job> = {}): Job => ({ job_id: 'jb_fixture', kind: 'duplicates', root_id: 'rt_fixture', seq: 1, rev: 0, state: 'queued', created_at: '2026-10-01T00:00:00Z', started_at: null, finished_at: null, coverage: null, error: null, progress: null, ...patch });
afterEach(() => { session.client = null; vi.useRealTimers(); });

describe('duplicates jobs', () => {
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
  it('uses the content refusal as a gated state without inventing a result', async () => {
    session.client = new Client({ call: async (r: { request_id: string }) => ({ protocol: 'loomward/3', request_id: r.request_id, ok: false, error: { code: 'capability_unavailable', message: 'gated', retryable: false, detail: { reason: 'content_read_gated' } } }) } as unknown as Transport);
    const store = new Duplicates();
    await store.start('rt_fixture');
    expect(store.gated).toBe(true);
    expect(store.problem).toBe('');
    expect(store.book).toEqual({});
  });
});
