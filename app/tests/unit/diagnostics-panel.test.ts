// The diagnostic panel keeps its state outside the component and coalesces reloads (#437).
import { expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import type { DiagnosticsPreviewResult, RequestEnvelope, ResponseEnvelope } from '../../src/lib/contracts.gen';
import { canonicalText } from '../../src/views/health/diagnostics';
import { DiagnosticsPanel } from '../../src/views/health/panel.svelte';

const META = { served_at: '2026-10-12T00:00:00Z', elapsed_ms: 0, dataset_class: 'synthetic' as const, budget_hit: false, catalog_rev: null, state_rev: null };

/** A preview whose bundle differs per `n`, with the size and digest the service would report for it. */
async function preview(base: DiagnosticsPreviewResult, n: number): Promise<DiagnosticsPreviewResult> {
  const bundle = { ...base.bundle, generated_at: `2026-10-12T00:00:${String(n).padStart(2, '0')}Z` };
  const bytes = new TextEncoder().encode(canonicalText(bundle));
  const hash = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes));
  return { bundle, bytes: String(bytes.length), digest: `sha256:${Array.from(hash, (b) => b.toString(16).padStart(2, '0')).join('')}` };
}

/** Previews answer one at a time when released; each call is numbered, and the in-flight maximum is kept. */
async function setup(saves: Array<'deadline' | 'saved'> = []) {
  const inner = createMockTransport();
  const base = (await new Client(inner).call('diagnostics.preview', {})).result;
  const waiting: Array<() => void> = [];
  const state = { started: 0, inFlight: 0, maxInFlight: 0, saves: 0, auto: false };
  const client = new Client({
    ...inner, mode: inner.mode, subscribe: inner.subscribe,
    call: async (req: RequestEnvelope): Promise<ResponseEnvelope> => {
      const rid = req.request_id;
      if (req.command === 'diagnostics.save') {
        state.saves++;
        const a = saves.shift();
        if (a === 'saved') return { protocol: 'loomward/3', request_id: rid, ok: true, result: { outcome: 'saved', bytes_written: '2835', refusal: null }, meta: META };
        return { protocol: 'loomward/3', request_id: rid, ok: false, error: { code: 'deadline_exceeded', message: 'synthetic', retryable: false, detail: null } };
      }
      if (req.command !== 'diagnostics.preview') return inner.call(req);
      const n = ++state.started;
      state.maxInFlight = Math.max(state.maxInFlight, ++state.inFlight);
      if (!state.auto) await new Promise<void>((r) => waiting.push(r));
      state.inFlight--;
      return { protocol: 'loomward/3', request_id: rid, ok: true, result: (await preview(base, n)) as unknown as Record<string, unknown>, meta: META };
    },
  });
  const settle = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); await new Promise((r) => setTimeout(r, 5)); };
  return { client, state, release: async () => { waiting.shift()?.(); await settle(); }, settle, auto: () => { state.auto = true; waiting.splice(0).forEach((r) => r()); } };
}

it('answers a burst of invalidations with one call running and one more, never one each', async () => {
  const { client, state, release, settle } = await setup();
  const p = new DiagnosticsPanel();
  const first = p.refresh(client, String);
  for (let i = 0; i < 100; i++) void p.refresh(client, String); // 100 scan invalidations while the first call is slow
  await settle();
  expect(state.started).toBe(1);
  await release(); // the first answer is older than the next ask: it is never shown
  expect(p.preview).toBeNull();
  expect(p.verified).toBeNull();
  expect(state.started).toBe(2);
  await release();
  await first;
  expect(state.started).toBe(2);
  expect(state.maxInFlight).toBe(1);
  expect(p.preview?.bundle.generated_at).toBe('2026-10-12T00:00:02Z'); // the newest wins
  expect(p.verified).toBe(true);
  expect(p.loading).toBe(false);
});

it('keeps a pending save and its frozen preview across the component, and reloads once afterwards', async () => {
  const { client, state, auto, settle } = await setup(['deadline', 'saved']);
  auto();
  const p = new DiagnosticsPanel();
  await p.refresh(client, String);
  const shown = p.preview?.digest;
  await p.save(client, String); // the dialog outlived the wait
  expect(p.saves.pending?.digest).toBe(shown);
  expect(p.problem).toContain('still pending');
  // the view is left and re-entered (a new component asks again) while the save is unknown
  for (let i = 0; i < 5; i++) void p.refresh(client, String);
  await settle();
  expect(state.started).toBe(1); // nothing reloaded under the open dialog
  expect(p.preview?.digest).toBe(shown); // and the text shown is still the text being saved
  await p.save(client, String);
  expect(p.notice).toContain('Saved 2,835 bytes');
  await settle();
  expect(state.started).toBe(2); // one reload for the five asks
  expect(p.preview?.digest).not.toBe(shown);
});

it('keeps a notice nobody saw across a reload, and clears one that was shown once the text changes', async () => {
  const { client, auto } = await setup(['saved']);
  auto();
  const p = new DiagnosticsPanel();
  await p.refresh(client, String);
  await p.save(client, String);
  expect(p.notice).toContain('Saved');
  await p.refresh(client, String); // the view was closed: the new text does not erase an unseen outcome
  expect(p.notice).toContain('Saved');
  p.seen = true; // the panel shows it
  await p.refresh(client, String);
  expect(p.notice).toBe('');
});
