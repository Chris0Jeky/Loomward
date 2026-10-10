import { afterEach, describe, expect, it, vi } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { Explanation } from '../../src/views/companion/explanation.svelte';

function setup() {
  const inner = createMockTransport({ now: () => new Date('2026-10-01T00:00:00Z') });
  let explains = 0;
  const client = new Client({ ...inner, call: (req, signal) => {
    if (req.command === 'processes.explain' && ++explains === 1) {
      return Promise.resolve({ protocol: 'loomward/3', request_id: req.request_id, ok: false,
        error: { code: 'partial_coverage', message: 'No fresh process sample.', retryable: true, detail: null } });
    }
    return inner.call(req, signal);
  } });
  const handle = vi.fn((e: unknown) => String(e));
  return { client, explanation: new Explanation(() => client, handle), handle, count: () => explains };
}

describe('Companion deep-link explanation', () => {
  afterEach(() => vi.restoreAllMocks());

  it('does not retry on stale observations, even with a newer system sample sequence', async () => {
    const at = Date.parse('2026-10-01T00:00:00Z');
    vi.spyOn(Date, 'now').mockReturnValue(at);
    const { client, explanation, count } = setup();
    const { result: retained } = await client.call('processes.list', { sort: 'private_desc', limit: 20 });
    await explanation.open(retained.rows[0]!.process_ref);
    for (const observed_at of ['2026-09-30T23:59:59Z', retained.observed_at, 'invalid']) {
      await explanation.resume({ ...retained, sample_seq: retained.sample_seq + 10, observed_at });
      expect(count()).toBe(1);
      expect(explanation.pending).not.toBeNull();
    }
    await explanation.resume({ ...retained, observed_at: '2026-10-01T00:00:01Z' });
    expect(count()).toBe(2);
    expect(explanation.answer).not.toBeNull();
  });

  it('handles a rejected retry and shows the error even when the session error handler throws', async () => {
    vi.spyOn(Date, 'now').mockReturnValue(Date.parse('2026-10-01T00:00:00Z'));
    const { client } = setup();
    const { result: retained } = await client.call('processes.list', { sort: 'private_desc', limit: 20 });
    const handle = vi.fn(() => { throw new Error('Synthetic retry handler failed'); });
    const explanation = new Explanation(() => client, handle);
    await explanation.open('pc_missing');
    await expect(explanation.resume({ ...retained, observed_at: '2026-10-01T00:00:01Z' })).resolves.toBeUndefined();
    expect(handle).toHaveBeenCalledOnce();
    expect(explanation.error).toBe('Synthetic retry handler failed');
    expect(explanation.answer).toBeNull();
    expect(explanation.pending).toBeNull();
  });

  it('waits after partial coverage and retries once when its process sample arrives', async () => {
    vi.spyOn(Date, 'now').mockReturnValue(Date.parse('2026-10-01T00:00:00Z'));
    const { client, explanation, handle, count } = setup();
    const { result: retained } = await client.call('processes.list', { sort: 'private_desc', limit: 20 });
    const ref = retained.rows[0]!.process_ref;
    await explanation.open(ref);
    expect(explanation.pending).toBe(ref);
    expect(explanation.error).toBe('');
    expect(handle).not.toHaveBeenCalled();
    await explanation.resume(null);
    expect(count()).toBe(1);
    const { result: sample } = await client.call('processes.list', { sort: 'private_desc', limit: 20 });
    await explanation.resume({ ...sample, observed_at: '2026-10-01T00:00:01Z' });
    expect(count()).toBe(2);
    expect(explanation.answer?.result.process_ref).toBe(ref);
    expect(explanation.pending).toBeNull();
    await explanation.resume(sample);
    expect(count()).toBe(2);
  });

  it('does not retry a closed link, and reports a missing reference instead of the previous answer', async () => {
    const { client, explanation, count } = setup();
    const { result: sample } = await client.call('processes.list', { sort: 'private_desc', limit: 20 });
    await explanation.open('pc_missing');
    explanation.close();
    await explanation.resume(sample);
    expect(count()).toBe(1);
    await explanation.open(sample.rows[0]!.process_ref);
    expect(explanation.answer?.result.process_ref).toBe(sample.rows[0]!.process_ref);
    await explanation.open('pc_missing');
    expect(explanation.error).toContain('no such process');
    expect(explanation.answer).toBeNull();
    expect(explanation.pending).toBeNull();
  });
});
