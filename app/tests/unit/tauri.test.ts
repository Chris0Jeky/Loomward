import { describe, expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createTauriTransport, type TauriApi } from '../../src/lib/transport/tauri';
import { TransportError } from '../../src/lib/transport/transport';

class FakeChannel<T> {
  onmessage: (m: T) => void = () => {};
}
const Channel = FakeChannel as unknown as TauriApi['Channel'];
const env = (epoch: string, seq: number, event: string, data: object = {}) => ({ protocol: 'loomward/3', epoch, seq, event, at: 'x', catalog_rev: null, state_rev: null, data });
const ev = (seq: number, epoch = 'e1') => env(epoch, seq, 'roots.changed');

describe('tauri transport', () => {
  it('invokes lw_call with the request and lw_events with a channel, lastEpoch and lastSeq', async () => {
    const calls: [string, Record<string, unknown> | Uint8Array | undefined][] = [];
    let channel!: FakeChannel<unknown>;
    const api: TauriApi = {
      invoke: async (cmd, args) => {
        calls.push([cmd, args]);
        if (cmd === 'lw_call') {
          expect(args).toBeInstanceOf(Uint8Array);
          const r = JSON.parse(new TextDecoder().decode(args as Uint8Array)) as { request_id: string };
          return { protocol: 'loomward/3', request_id: r.request_id, ok: true, result: { roots: [] }, meta: { served_at: 'x', elapsed_ms: 0, dataset_class: 'synthetic', budget_hit: false } };
        }
        channel = (args as Record<string, unknown>).channel as FakeChannel<unknown>;
        return null;
      },
      Channel,
    };
    const t = await createTauriTransport(api);
    expect((await new Client(t).call('roots.list', {})).result).toEqual({ roots: [] });
    const got: number[] = [];
    const states: string[] = [];
    const stop = t.subscribe((e) => got.push(e.seq), (s) => states.push(s), { epoch: 'e1', seq: 41 });
    await Promise.resolve();
    channel.onmessage(ev(42));
    channel.onmessage({ nonsense: true });
    stop();
    channel.onmessage(ev(43));
    expect(got).toEqual([42]);
    expect(states).toEqual(['open']);
    expect(calls.map((c) => c[0])).toEqual(['lw_call', 'lw_events']);
    expect(calls[1]![1]).toMatchObject({ lastEpoch: 'e1', lastSeq: 41 });
  });

  it('maps an invoke failure to a transport fault', async () => {
    const api: TauriApi = { invoke: async () => { throw 'boom'; }, Channel };
    await expect(new Client(await createTauriTransport(api)).call('roots.list', {})).rejects.toBeInstanceOf(TransportError);
  });
  it.each([false, true])('retries transport busy only once with the same raw request (still busy: %s)', async (stillBusy) => {
    const bodies: unknown[] = [];
    const sleeps: number[] = [];
    const api: TauriApi = { Channel, invoke: async (_, body) => {
      bodies.push(body);
      if (bodies.length === 1 || stillBusy) throw 'busy: at most four calls in flight';
      return { ok: true };
    } };
    const t = await createTauriTransport(api, { sleep: async (ms) => { sleeps.push(ms); } });
    const call = t.call({ protocol: 'loomward/3', request_id: 'r_busy', command: 'session.hello', payload: {} });
    if (stillBusy) await expect(call).rejects.toBeInstanceOf(TransportError);
    else expect(await call).toEqual({ ok: true });
    expect(sleeps).toEqual([500]);
    expect(bodies).toHaveLength(2);
    expect(bodies[1]).toBe(bodies[0]);
  });
  describe('liveness', () => {
    const mk = (opts: { rejectEvents?: number } = {}) => {
      const channels: FakeChannel<unknown>[] = [];
      const lastSeqs: unknown[] = [];
      const resumes: unknown[] = [];
      let rejects = opts.rejectEvents ?? 0;
      const api: TauriApi = {
        invoke: async (cmd, args) => {
          if (cmd !== 'lw_events') return null;
          const a = args as Record<string, unknown>;
          lastSeqs.push(a.lastSeq);
          resumes.push([a.lastEpoch, a.lastSeq]);
          if (rejects-- > 0) throw 'engine gone';
          channels.push(a.channel as FakeChannel<unknown>);
          return null;
        },
        Channel,
      };
      return { api, channels, lastSeqs, resumes };
    };
    const until = async (cond: () => boolean) => { for (let i = 0; i < 200 && !cond(); i++) await new Promise((r) => setTimeout(r, 5)); };

    it('surfaces one lagged event for a stalled renderer, discards stale backlog, then accepts fresh data', async () => {
      const { api, channels } = mk();
      const t = await createTauriTransport(api);
      const got: string[] = [];
      const stop = t.subscribe((e) => got.push(e.event), () => {});
      await until(() => channels.length === 1);
      const old = new Date(Date.now() - 10_000).toISOString();
      channels[0]!.onmessage({ ...ev(1), at: old });
      channels[0]!.onmessage({ ...ev(2), at: old });
      channels[0]!.onmessage({ ...ev(3), at: new Date().toISOString() });
      stop();
      expect(got).toEqual(['stream.lagged', 'roots.changed']);
    });
    it('replays retained events older than five seconds up to the first hello watermark, then guards live backlog', async () => {
      const { api, channels } = mk();
      const t = await createTauriTransport(api);
      const got: string[] = [];
      const stop = t.subscribe((e) => got.push(`${e.event}:${e.seq}`), () => {}, { epoch: 'e1', seq: 1 });
      try {
        await until(() => channels.length === 1);
        const channel = channels[0]!;
        const old = new Date(Date.now() - 60_000).toISOString();
        channel.onmessage(env('e1', 3, 'stream.hello', { epoch: 'e1', last_seq: 3, oldest_replayable_seq: 1 }));
        for (const seq of [1, 2, 3, 3]) channel.onmessage({ ...ev(seq), at: old });
        // A later heartbeat must not widen the initial replay exemption.
        channel.onmessage(env('e1', 4, 'stream.hello', { epoch: 'e1', last_seq: 4, oldest_replayable_seq: 1 }));
        channel.onmessage({ ...ev(4), at: old });
        channel.onmessage({ ...ev(5), at: new Date().toISOString() });
        expect(got).toEqual(['stream.hello:3', 'roots.changed:2', 'roots.changed:3', 'stream.hello:4', 'stream.lagged:4', 'roots.changed:5']);
      } finally { stop(); }
    });

    it.each([
      { last: 99, oldest: 2, seq: 6, replay: false },
      { last: 1, oldest: 3, seq: 3, replay: true },
      { last: 5, oldest: 3, seq: 2, replay: false },
      { last: -1, oldest: 2, seq: 2, replay: false },
      { last: 2.5, oldest: 2, seq: 2, replay: false },
      { last: '5', oldest: 2, seq: 2, replay: false },
      { last: Number.MAX_SAFE_INTEGER + 1, oldest: 2, seq: 2, replay: false },
      { last: 5, oldest: -1, seq: 2, replay: false },
      { last: 5, oldest: 2.5, seq: 3, replay: false },
      { last: 5, oldest: undefined, seq: 2, replay: false },
      { last: 5, oldest: 6, seq: 5, replay: false },
    ])('bounds the first-hello replay window: $last/$oldest, event $seq', async ({ last, oldest, seq, replay }) => {
      const { api, channels } = mk();
      const t = await createTauriTransport(api);
      const got: string[] = [];
      const stop = t.subscribe((e) => got.push(`${e.event}:${e.seq}`), () => {}, { epoch: 'e1', seq: 1 });
      try {
        await until(() => channels.length === 1);
        channels[0]!.onmessage({ ...env('e1', 5, 'stream.hello', { epoch: 'e1', last_seq: last, oldest_replayable_seq: oldest }), at: new Date().toISOString() });
        channels[0]!.onmessage({ ...ev(seq), at: new Date(Date.now() - 60_000).toISOString() });
        expect(got).toEqual(['stream.hello:5', `${replay ? 'roots.changed' : 'stream.lagged'}:${seq}`]);
        channels[0]!.onmessage({ ...ev(7), at: new Date().toISOString() });
        expect(got.at(-1)).toBe('roots.changed:7');
      } finally { stop(); }
    });

    it('reports closed when the channel goes silent past the heartbeat window, then resubscribes from the last seq', async () => {
      const { api, channels, lastSeqs, resumes } = mk();
      const t = await createTauriTransport(api, { idleMs: 40, sleep: async () => {} });
      const states: string[] = [];
      const stop = t.subscribe(() => {}, (s) => states.push(s), null);
      await until(() => channels.length === 1);
      channels[0]!.onmessage(ev(9));
      await until(() => channels.length === 2);
      expect(states.slice(0, 3)).toEqual(['open', 'closed', 'open']);
      expect(lastSeqs).toEqual([null, 9]);
      expect(resumes).toEqual([[null, null], ['e1', 9]]);
      channels[0]!.onmessage(ev(10)); // the dead channel is ignored
      stop();
    });

    it('resumes after every discarded stale event without replaying the collapsed backlog', async () => {
      const { api, channels, resumes } = mk();
      const t = await createTauriTransport(api, { idleMs: 40, sleep: async () => {} });
      const got: string[] = [];
      const stop = t.subscribe((e) => got.push(e.event), () => {}, { epoch: 'e1', seq: 0 });
      try {
        await until(() => channels.length === 1);
        const old = new Date(Date.now() - 10_000).toISOString();
        for (const seq of [1, 2, 9, 3]) channels[0]!.onmessage({ ...ev(seq), at: old });
        await until(() => channels.length === 2);
        expect(got).toEqual(['stream.lagged']);
        expect(resumes[1]).toEqual(['e1', 9]);
      } finally {
        stop();
      }
    });

    it('resumes in the new epoch after a hello and an epoch_changed lagged, without trusting the unsequenced hello inside one epoch', async () => {
      const { api, channels, resumes } = mk();
      const t = await createTauriTransport(api, { idleMs: 40, sleep: async () => {} });
      const got: string[] = [];
      const stop = t.subscribe((e) => got.push(`${e.event}:${e.epoch}.${e.seq}`), () => {}, { epoch: 'e1', seq: 40 });
      await until(() => channels.length === 1);
      channels[0]!.onmessage(env('e2', 3, 'stream.hello', { epoch: 'e2', last_seq: 3, oldest_replayable_seq: 1 }));
      channels[0]!.onmessage(env('e2', 0, 'stream.lagged', { reason: 'epoch_changed', dropped: null, resync: ['roots'] }));
      channels[0]!.onmessage(ev(4, 'e2'));
      channels[0]!.onmessage(ev(4, 'e2')); // duplicate
      await until(() => channels.length === 2);
      stop();
      expect(resumes).toEqual([['e1', 40], ['e2', 4]]);
      expect(got).toEqual(['stream.hello:e2.3', 'stream.lagged:e2.0', 'roots.changed:e2.4']);
    });

    it.each([0, 9])('delivers a subscriber_overflow lagged with seq %i and keeps the resume point', async (seq) => {
      const { api, channels, resumes } = mk();
      const t = await createTauriTransport(api, { idleMs: 40, sleep: async () => {} });
      const got: string[] = [];
      const stop = t.subscribe((e) => got.push(`${e.event}:${e.epoch}.${e.seq}`), () => {}, null);
      await until(() => channels.length === 1);
      channels[0]!.onmessage(env('e1', 4, 'stream.hello', { epoch: 'e1', last_seq: 4, oldest_replayable_seq: 1 }));
      channels[0]!.onmessage(ev(9));
      channels[0]!.onmessage(env('e1', seq, 'stream.lagged', { reason: 'subscriber_overflow', dropped: 3, resync: ['roots'] }));
      await until(() => channels.length === 2);
      stop();
      expect(got).toEqual(['stream.hello:e1.4', 'roots.changed:e1.9', `stream.lagged:e1.${seq}`]);
      expect(resumes[1]).toEqual(['e1', 9]);
    });

    it('stays open while messages keep arriving', async () => {
      const { api, channels } = mk();
      const t = await createTauriTransport(api, { idleMs: 60, sleep: async () => {} });
      const states: string[] = [];
      const stop = t.subscribe(() => {}, (s) => states.push(s));
      await until(() => channels.length === 1);
      for (let i = 0; i < 10; i++) { channels[0]!.onmessage(ev(i)); await new Promise((r) => setTimeout(r, 20)); }
      stop();
      expect(states).toEqual(['open']);
    });

    it('reports closed when the subscribe call is refused, and retries', async () => {
      const { api, channels } = mk({ rejectEvents: 2 });
      const t = await createTauriTransport(api, { idleMs: 1000, sleep: async () => {} });
      const states: string[] = [];
      const stop = t.subscribe(() => {}, (s) => states.push(s));
      await until(() => channels.length === 1);
      stop();
      expect(states.slice(0, 3)).toEqual(['closed', 'closed', 'open']);
    });

    it('stops everything on unsubscribe', async () => {
      const { api, channels, lastSeqs } = mk();
      const t = await createTauriTransport(api, { idleMs: 20, sleep: async () => {} });
      const stop = t.subscribe(() => {}, () => {});
      await until(() => channels.length === 1);
      stop();
      await new Promise((r) => setTimeout(r, 80));
      expect(lastSeqs).toHaveLength(1);
    });
  });
});
