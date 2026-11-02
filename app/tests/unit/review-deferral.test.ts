import { expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import type { ErrorCode, RequestEnvelope, ResponseEnvelope } from '../../src/lib/contracts.gen';
import { ReviewQueue } from '../../src/views/review/queue.svelte';

const META = { served_at: '2026-10-31T00:00:00Z', elapsed_ms: 0, dataset_class: 'synthetic' as const, budget_hit: false, catalog_rev: '1', state_rev: '7' };
const PAGE = { model_id: null, items: [], next_cursor: 'cu_next' };
function setup() {
  const waiting: Array<{ request: RequestEnvelope; resolve: (response: ResponseEnvelope) => void }> = [];
  const client = new Client({ mode: 'http', subscribe: () => () => {}, call: request => new Promise(resolve => waiting.push({ request, resolve })) });
  const answer = (index: number, result: Record<string, unknown> | ErrorCode = PAGE) => {
    const { request, resolve } = waiting[index]!;
    resolve(typeof result === 'string'
      ? { protocol: 'loomward/3', request_id: request.request_id, ok: false, error: { code: result, message: result, retryable: false, detail: null } }
      : { protocol: 'loomward/3', request_id: request.request_id, ok: true, result, meta: META });
  };
  return { client, waiting, answer, queue: new ReviewQueue() };
}
const tick = async () => { await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); };

it('refresh, paging, mode choice and leaving never record a preference or label', async () => {
  const { client, waiting, answer, queue } = setup();
  let load = queue.load(client, String); answer(0); await load;
  expect(waiting[0]!.request.payload).toEqual({ root_id: null, limit: 20, cursor: null });
  load = queue.load(client, String, queue.page!.next_cursor); answer(1); await load;
  load = queue.load(client, String, null, 'deferred'); answer(2); await load;
  expect(waiting[2]!.request.payload.mode).toBe('deferred');
  load = queue.load(client, String); queue.clear(); answer(3); await load;
  expect(queue.page).toBeNull();
  expect(waiting.every(w => w.request.command === 'learning.queue')).toBe(true);
});

it('retains the same durable choice and original revision after an unknown outcome, without automatic retry', async () => {
  const { client, waiting, answer, queue } = setup();
  const load = queue.load(client, String); answer(0); await load;
  let choice = queue.choose(client, String, 'nd_one', true);
  expect(waiting[1]!.request.command).toBe('learning.set_deferred');
  expect(waiting[1]!.request.expected_state_rev).toBe('7');
  answer(1, 'deadline_exceeded'); await choice;
  expect(waiting).toHaveLength(2);
  expect(queue.pending?.payload).toMatchObject({ node_id: 'nd_one', deferred: true });
  choice = queue.choose(client, String);
  expect(waiting[2]!.request.payload).toEqual(waiting[1]!.request.payload);
  expect(waiting[2]!.request.expected_state_rev).toBe('7');
  answer(2, { deferred: true, revision: '8', idempotent_replay: true }); await tick();
  answer(3, { ...PAGE, next_cursor: null }); await choice;
  expect(queue.pending).toBeNull();
  expect(queue.notice).toContain('No label recorded');
  expect(waiting.map(w => w.request.command)).not.toContain('feedback.record');
});

it('a stale choice refreshes the queue and requires a new explicit choice', async () => {
  const { client, waiting, answer, queue } = setup();
  const load = queue.load(client, String); answer(0); await load;
  const choice = queue.choose(client, String, 'nd_one', false);
  answer(1, 'stale_generation'); await tick(); answer(2); await choice;
  expect(queue.pending).toBeNull();
  expect(queue.problem).toContain('stale_generation');
  expect(waiting.filter(w => w.request.command === 'learning.set_deferred')).toHaveLength(1);
});

it('an older service keeps ordinary review usable without pretending its queue is deferred', async () => {
  const { client, waiting, answer, queue } = setup();
  const load = queue.load(client, String, null, 'deferred');
  answer(0, 'invalid_request'); await tick();
  expect(waiting[1]!.request.payload).toEqual({ root_id: null, limit: 20, cursor: null });
  answer(1); await load;
  expect(queue.mode).toBe('review');
  expect(queue.supported).toBe(false);
  expect(queue.page).toEqual(PAGE);
  expect(queue.notice).toContain('unavailable in this service');
});

it('stale pages restart once, and late replies cannot replace a refreshed queue', async () => {
  const { client, waiting, answer, queue } = setup();
  const old = queue.load(client, String);
  const current = queue.load(client, String, 'cu_old', 'deferred');
  answer(1, 'stale_generation'); await tick();
  expect(waiting[2]!.request.payload).toEqual({ root_id: null, limit: 20, cursor: null, mode: 'deferred' });
  answer(2, { ...PAGE, next_cursor: null }); await current;
  answer(0); await old;
  expect(queue.page!.next_cursor).toBeNull();
  expect(queue.notice).toContain('first page');
  expect(waiting).toHaveLength(3);
});
