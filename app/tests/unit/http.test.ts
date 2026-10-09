import { describe, expect, it } from 'vitest';
import { Client, LoomwardError } from '../../src/lib/transport/client';
import { createHttpTransport } from '../../src/lib/transport/http';
import { TransportError } from '../../src/lib/transport/transport';
import type { EventEnvelope } from '../../src/lib/types';

const meta = { served_at: '2026-10-01T00:00:00Z', elapsed_ms: 1, dataset_class: 'synthetic', budget_hit: false };
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
const asFetch = (f: (url: string, init: RequestInit) => Promise<Response>) => f as unknown as typeof fetch;
const idOf = (init: RequestInit) => (JSON.parse(init.body as string) as { request_id: string }).request_id;
/** Parks until the transport aborts it, so a loop that should stop does not spin. */
const hang = (init: RequestInit) => new Promise<never>((_, rej) => init.signal!.addEventListener('abort', () => rej(new Error('aborted'))));

describe('http transport', () => {
  it('posts the envelope with the token header and returns the result', async () => {
    let seen: { url: string; init: RequestInit } | undefined;
    const t = createHttpTransport({
      base: 'http://127.0.0.1:9',
      token: 'tok'.repeat(8),
      fetchImpl: asFetch(async (url, init) => {
        seen = { url, init };
        return json({ protocol: 'loomward/3', request_id: idOf(init), ok: true, result: { hello: 1 }, meta });
      }),
    });
    const { result } = await new Client(t).call('roots.list', {});
    expect(result).toEqual({ hello: 1 });
    expect(seen!.url).toBe('http://127.0.0.1:9/api/v3/call');
    const h = seen!.init.headers as Record<string, string>;
    expect(h['X-Loomward-Token']).toBe('tok'.repeat(8));
    expect(h['Content-Type']).toBe('application/json');
    expect(seen!.init.credentials).toBe('omit');
  });

  it('turns an error envelope into LoomwardError', async () => {
    const t = createHttpTransport({
      base: 'http://x',
      token: 't',
      fetchImpl: asFetch(async (_u, init) =>
        json({ protocol: 'loomward/3', request_id: idOf(init), ok: false, error: { code: 'stale_generation', message: 'old', retryable: true, detail: null } }),
      ),
    });
    const err = await new Client(t).call('roots.list', {}).catch((e) => e);
    expect(err).toBeInstanceOf(LoomwardError);
    expect(err).toMatchObject({ code: 'stale_generation', retryable: true });
  });

  it('treats network failure, non-200, non-JSON and foreign envelopes as transport faults', async () => {
    const mk = (f: () => Promise<Response>) => new Client(createHttpTransport({ base: 'http://x', token: 't', fetchImpl: asFetch(f) }));
    await expect(mk(async () => { throw new TypeError('Failed to fetch'); }).call('roots.list', {})).rejects.toBeInstanceOf(TransportError);
    await expect(mk(async () => json({}, 403)).call('roots.list', {})).rejects.toBeInstanceOf(TransportError);
    await expect(mk(async () => new Response('<html>')).call('roots.list', {})).rejects.toBeInstanceOf(TransportError);
    await expect(mk(async () => json({ protocol: 'loomward/3', request_id: 'r_other', ok: true, result: {}, meta })).call('roots.list', {})).rejects.toBeInstanceOf(TransportError);
  });

  it('streams events, drops garbled frames, and reconnects with Last-Event-ID', async () => {
    const ev = (seq: number): EventEnvelope => ({ protocol: 'loomward/3', seq, event: 'roots.changed', at: '2026-10-01T00:00:00Z', data: {} });
    const frame = (e: unknown, id: number) => `id: ${id}\nevent: roots.changed\ndata: ${JSON.stringify(e)}\n\n`;
    const bodies = [frame(ev(1), 1) + 'data: {not json\n\n' + frame({ protocol: 'other' }, 2) + frame(ev(3), 3), frame(ev(4), 4)];
    const headersSeen: Record<string, string>[] = [];
    const got: number[] = [];
    const states: string[] = [];
    let n = 0;
    let release!: () => void;
    const done = new Promise<void>((r) => (release = r));
    const t = createHttpTransport({
      base: 'http://x',
      token: 't',
      sleep: async () => {},
      fetchImpl: asFetch(async (_u, init) => {
        headersSeen.push(init.headers as Record<string, string>);
        const i = n++;
        return i < bodies.length ? new Response(bodies[i]) : hang(init);
      }),
    });
    const stop = t.subscribe((e) => { got.push(e.seq); if (e.seq === 4) release(); }, (s) => states.push(s), null);
    await done;
    stop();
    expect(got).toEqual([1, 3, 4]);
    expect(headersSeen[0]!['Last-Event-ID']).toBeUndefined();
    expect(headersSeen[1]!['Last-Event-ID']).toBe('3');
    expect(headersSeen[0]!['X-Loomward-Token']).toBe('t');
    expect(states.slice(0, 3)).toEqual(['open', 'closed', 'open']);
  });

  it('reports closed when the stream cannot open', async () => {
    const states: string[] = [];
    let n = 0;
    let release!: () => void;
    const done = new Promise<void>((r) => (release = r));
    const t = createHttpTransport({
      base: 'http://x',
      token: 't',
      sleep: async () => {},
      fetchImpl: asFetch(async (_u, init) => (++n > 2 ? hang(init) : json({}, 403))),
    });
    const stop = t.subscribe(() => {}, (s) => { states.push(s); if (states.length === 2) release(); });
    await done;
    stop();
    expect(states).toEqual(['closed', 'closed']);
  });
  it('bounds a streamed response body: stops reading past the cap and cancels', async () => {
    let pulls = 0;
    let cancelled = false;
    const body = new ReadableStream<Uint8Array>({
      pull(c) { pulls++; c.enqueue(new Uint8Array(1024 * 1024)); },
      cancel() { cancelled = true; },
    });
    const client = new Client(createHttpTransport({ base: 'http://x', token: 't', fetchImpl: asFetch(async () => new Response(body)) }));
    await expect(client.call('roots.list', {})).rejects.toThrow(/too large/);
    expect(cancelled).toBe(true);
    expect(pulls).toBeLessThan(20); // 8 MiB cap, not an unbounded read
  });

  it('rejects an oversized Content-Length before reading the body', async () => {
    let pulls = 0;
    const body = new ReadableStream<Uint8Array>({ pull(c) { pulls++; c.enqueue(new Uint8Array(8)); } });
    const res = new Response(body, { headers: { 'Content-Length': String(64 * 1024 * 1024) } });
    const client = new Client(createHttpTransport({ base: 'http://x', token: 't', fetchImpl: asFetch(async () => res) }));
    await expect(client.call('roots.list', {})).rejects.toThrow(/too large/);
    expect(pulls).toBeLessThanOrEqual(1);
  });

  it('closes and reconnects when the event stream never ends a line', async () => {
    const states: string[] = [];
    let n = 0;
    let release!: () => void;
    const done = new Promise<void>((r) => (release = r));
    const t = createHttpTransport({
      base: 'http://x',
      token: 't',
      sleep: async () => {},
      sseLimits: { maxLine: 256, maxEvent: 1024 },
      fetchImpl: asFetch(async (_u, init) => {
        if (++n > 2) return hang(init);
        return new Response(new ReadableStream<Uint8Array>({ pull(c) { c.enqueue(new TextEncoder().encode('x'.repeat(100))); } }));
      }),
    });
    const stop = t.subscribe(() => {}, (s) => { states.push(s); if (states.length >= 4) release(); });
    await done;
    stop();
    expect(states.slice(0, 4)).toEqual(['open', 'closed', 'open', 'closed']);
  });
});
