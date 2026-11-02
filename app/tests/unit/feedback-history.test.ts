import { expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import type { FeedbackHistory, RequestEnvelope, ResponseEnvelope } from '../../src/lib/contracts.gen';
import { HistoryPanel } from '../../src/views/review/history.svelte';

const META = { served_at: '2026-10-31T00:00:00Z', elapsed_ms: 0, dataset_class: 'synthetic' as const, budget_hit: false, catalog_rev: '1', state_rev: '1' };
const page = (revision: number, next_cursor: string | null = null): FeedbackHistory => ({ items: [{ event_id: `fe_${revision}`, revision, label: 'Archive', retracted: revision === 3, is_current: revision === 3, taxonomy_version: 1, recorded_at: META.served_at, reference_status: 'resolved', name: '<img src=x onerror=alert(1)>' }], next_cursor });
function setup() {
  const waiting: Array<{ request: RequestEnvelope; resolve: (response: ResponseEnvelope) => void }> = [];
  const client = new Client({ mode: 'http', subscribe: () => () => {}, call: request => new Promise(resolve => waiting.push({ request, resolve })) });
  const answer = (index: number, result: FeedbackHistory | 'stale_generation' | 'unknown_command') => {
    const { request, resolve } = waiting[index]!;
    resolve(typeof result === 'string'
      ? { protocol: 'loomward/3', request_id: request.request_id, ok: false, error: { code: result, message: result, retryable: false, detail: null } }
      : { protocol: 'loomward/3', request_id: request.request_id, ok: true, result: result as unknown as Record<string, unknown>, meta: META });
  };
  return { client, waiting, answer, panel: new HistoryPanel() };
}

it('pages human history without recording, refitting or changing withdrawal and visibility semantics', async () => {
  const { client, waiting, answer, panel } = setup();
  let load = panel.load(client, String);
  answer(0, page(3, 'cu_older'));
  await load;
  expect(panel.page?.items[0]).toMatchObject({ retracted: true, is_current: true, label: 'Archive' });
  load = panel.load(client, String, panel.page!.next_cursor);
  answer(1, { items: [{ ...page(2).items[0]!, reference_status: 'redacted', name: null }], next_cursor: null });
  await load;
  expect(panel.page?.items[0]).toMatchObject({ is_current: false, retracted: false, name: null });
  expect(panel.older).toBe(true);
  expect(waiting.map(w => [w.request.command, w.request.payload])).toEqual([
    ['feedback.history', { limit: 20, cursor: null }],
    ['feedback.history', { limit: 20, cursor: 'cu_older' }],
  ]);
});

it('drops late pages after refresh, mutation or leaving the view', async () => {
  const { client, answer, panel } = setup();
  const old = panel.load(client, String, 'cu_old');
  const current = panel.load(client, String);
  answer(1, page(3));
  await current;
  answer(0, page(1));
  await old;
  expect(panel.page?.items[0]?.revision).toBe(3);
  const pending = panel.load(client, String);
  panel.clear();
  answer(2, page(2));
  await pending;
  expect(panel.page).toBeNull();
  expect(panel.loading).toBe(false);
});

it('restarts a stale cursor once and keeps old-service failure local to history', async () => {
  const { client, waiting, answer, panel } = setup();
  const load = panel.load(client, String, 'cu_old');
  answer(0, 'stale_generation');
  await Promise.resolve(); await Promise.resolve();
  expect(waiting).toHaveLength(2);
  expect(waiting[1]!.request.payload).toEqual({ limit: 20, cursor: null });
  answer(1, 'stale_generation');
  await load;
  expect(waiting).toHaveLength(2);
  expect(panel.page).toBeNull();
  expect(panel.problem).toContain('stale_generation');
  const retry = panel.load(client, String);
  answer(2, 'unknown_command');
  await retry;
  expect(panel.problem).toContain('unknown_command');
  expect(panel.loading).toBe(false);
});
