// The diagnostic save keeps one action across an unknown outcome and freezes the preview while it is open (#399 review).
import { expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import type { RequestEnvelope, ResponseEnvelope } from '../../src/lib/contracts.gen';
import { SaveTracker, shortDigest } from '../../src/views/health/save.svelte';

const DIGEST = `sha256:${'ab'.repeat(32)}`;

/** A desktop-like transport whose diagnostics.save answers come from `answers`, in order, each after `gate` opens. */
function setup(answers: Array<'deadline' | 'saved' | 'busy'>) {
  const inner = createMockTransport();
  const sent: RequestEnvelope[] = [];
  let open: () => void = () => {};
  let gate = Promise.resolve();
  const hold = () => { gate = new Promise((r) => { open = r; }); };
  const client = new Client({
    ...inner, mode: inner.mode, subscribe: inner.subscribe,
    call: async (req): Promise<ResponseEnvelope> => {
      if (req.command !== 'diagnostics.save') return inner.call(req);
      sent.push(req);
      await gate;
      const base = { protocol: 'loomward/3' as const, request_id: req.request_id };
      const a = answers.shift();
      if (a === 'saved') return { ...base, ok: true, result: { outcome: 'saved', bytes_written: '2835', refusal: null }, meta: { served_at: '2026-10-12T00:00:00Z', elapsed_ms: 0, dataset_class: 'synthetic', budget_hit: false, catalog_rev: null, state_rev: null } };
      return { ...base, ok: false, error: { code: a === 'busy' ? 'busy' : 'deadline_exceeded', message: 'synthetic', retryable: a === 'busy', detail: null } };
    },
  });
  return { client, sent, hold, release: () => open() };
}

/** A save that answers `saved` once with the given raw bytes_written, bypassing types for malformed payloads. */
function setupBytesWritten(bytesWritten: unknown) {
  const inner = createMockTransport();
  const client = new Client({
    ...inner, mode: inner.mode, subscribe: inner.subscribe,
    call: async (req): Promise<ResponseEnvelope> => {
      if (req.command !== 'diagnostics.save') return inner.call(req);
      const base = { protocol: 'loomward/3' as const, request_id: req.request_id };
      return { ...base, ok: true, result: { outcome: 'saved', bytes_written: bytesWritten as string, refusal: null }, meta: { served_at: '2026-10-12T00:00:00Z', elapsed_ms: 0, dataset_class: 'synthetic', budget_hit: false, catalog_rev: null, state_rev: null } };
    },
  });
  return { client };
}

it('renders unknown instead of 0 bytes when bytes_written is null', async () => {
  const { client } = setupBytesWritten(null);
  const report = await new SaveTracker().save(client, DIGEST, String);
  expect(report?.kind).toBe('notice');
  expect(report?.text).toContain('unknown');
  expect(report?.text).not.toContain('0 bytes');
  expect(report?.text).not.toContain('NaN');
});

it('renders unknown instead of NaN bytes when bytes_written is not a number', async () => {
  const { client } = setupBytesWritten(Number.NaN);
  const report = await new SaveTracker().save(client, DIGEST, String);
  expect(report?.kind).toBe('notice');
  expect(report?.text).toContain('unknown');
  expect(report?.text).not.toContain('NaN');
  expect(report?.text).not.toContain('0 bytes');
});

it('renders unknown instead of NaN bytes when bytes_written is a malformed string', async () => {
  const { client } = setupBytesWritten('not-a-number');
  const report = await new SaveTracker().save(client, DIGEST, String);
  expect(report?.kind).toBe('notice');
  expect(report?.text).toContain('unknown');
  expect(report?.text).not.toContain('NaN');
  expect(report?.text).not.toContain('0 bytes');
});

it('still counts a numeric byte string', async () => {
  const { client } = setupBytesWritten('2835');
  const report = await new SaveTracker().save(client, DIGEST, String);
  expect(report).toEqual({ kind: 'notice', text: `Saved 2,835 bytes with digest ${shortDigest(DIGEST)}, exactly the text shown. Nothing was uploaded.` });
});

it('keeps the outstanding save after the dialog outlives the wait, and retries it under the same request id', async () => {
  const { client, sent } = setup(['deadline', 'saved']);
  const t = new SaveTracker();
  const first = await t.save(client, DIGEST, String);
  expect(first).toEqual({ kind: 'problem', text: expect.stringContaining('still pending in the dialog') });
  expect(t.pending?.digest).toBe(DIGEST);
  expect(t.frozen).toBe(true); // the preview may not reload while the outcome is unknown
  // even a click for another digest asks for the pending save, never a second one
  const second = await t.save(client, `sha256:${'cd'.repeat(32)}`, String);
  expect(sent.map((r) => r.request_id)).toEqual([sent[0]!.request_id, sent[0]!.request_id]);
  expect(sent.every((r) => (r.payload as { digest: string }).digest === DIGEST)).toBe(true);
  expect(second).toEqual({ kind: 'notice', text: `Saved 2,835 bytes with digest ${shortDigest(DIGEST)}, exactly the text shown. Nothing was uploaded.` });
  expect(t.pending).toBeNull();
  expect(t.frozen).toBe(false);
});

it('freezes the preview while a save runs, ignores a second click, and a new save gets a new id', async () => {
  const { client, sent, hold, release } = setup(['saved', 'busy']);
  const t = new SaveTracker();
  hold();
  const running = t.save(client, DIGEST, String);
  expect(t.frozen).toBe(true);
  expect(await t.save(client, DIGEST, String)).toBeNull(); // a second click while the dialog is open sends nothing
  release();
  expect((await running)?.kind).toBe('notice');
  expect(t.frozen).toBe(false);
  // a known refusal ends the action: the next save is a new one
  expect(await t.save(client, DIGEST, (e) => `handled ${String(e)}`)).toEqual({ kind: 'problem', text: expect.stringContaining('handled') });
  expect(t.pending).toBeNull();
  expect(new Set(sent.map((r) => r.request_id)).size).toBe(2);
});
