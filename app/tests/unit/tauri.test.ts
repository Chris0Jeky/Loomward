import { describe, expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createTauriTransport, type TauriApi } from '../../src/lib/transport/tauri';
import { TransportError } from '../../src/lib/transport/transport';

class FakeChannel<T> {
  onmessage: (m: T) => void = () => {};
}
const Channel = FakeChannel as unknown as TauriApi['Channel'];
const ev = (seq: number) => ({ protocol: 'loomward/3', seq, event: 'roots.changed', at: 'x', data: {} });

describe('tauri transport', () => {
  it('invokes lw_call with the request and lw_events with a channel and lastSeq', async () => {
    const calls: [string, Record<string, unknown> | undefined][] = [];
    let channel!: FakeChannel<unknown>;
    const api: TauriApi = {
      invoke: async (cmd, args) => {
        calls.push([cmd, args]);
        if (cmd === 'lw_call') {
          const r = args!.request as { request_id: string };
          return { protocol: 'loomward/3', request_id: r.request_id, ok: true, result: { roots: [] }, meta: { served_at: 'x', elapsed_ms: 0, dataset_class: 'synthetic', budget_hit: false } };
        }
        channel = args!.channel as FakeChannel<unknown>;
        return null;
      },
      Channel,
    };
    const t = await createTauriTransport(api);
    expect((await new Client(t).call('roots.list', {})).result).toEqual({ roots: [] });
    const got: number[] = [];
    const states: string[] = [];
    const stop = t.subscribe((e) => got.push(e.seq), (s) => states.push(s), 41);
    await Promise.resolve();
    channel.onmessage(ev(42));
    channel.onmessage({ nonsense: true });
    stop();
    channel.onmessage(ev(43));
    expect(got).toEqual([42]);
    expect(states).toEqual(['open']);
    expect(calls.map((c) => c[0])).toEqual(['lw_call', 'lw_events']);
    expect(calls[1]![1]!.lastSeq).toBe(41);
  });

  it('maps an invoke failure to a transport fault', async () => {
    const api: TauriApi = { invoke: async () => { throw 'boom'; }, Channel };
    await expect(new Client(await createTauriTransport(api)).call('roots.list', {})).rejects.toBeInstanceOf(TransportError);
  });
  describe('liveness', () => {
    const mk = (opts: { rejectEvents?: number } = {}) => {
      const channels: FakeChannel<unknown>[] = [];
      const lastSeqs: unknown[] = [];
      let rejects = opts.rejectEvents ?? 0;
      const api: TauriApi = {
        invoke: async (cmd, args) => {
          if (cmd !== 'lw_events') return null;
          lastSeqs.push(args!.lastSeq);
          if (rejects-- > 0) throw 'engine gone';
          channels.push(args!.channel as FakeChannel<unknown>);
          return null;
        },
        Channel,
      };
      return { api, channels, lastSeqs };
    };
    const until = async (cond: () => boolean) => { for (let i = 0; i < 200 && !cond(); i++) await new Promise((r) => setTimeout(r, 5)); };

    it('reports closed when the channel goes silent past the heartbeat window, then resubscribes from the last seq', async () => {
      const { api, channels, lastSeqs } = mk();
      const t = await createTauriTransport(api, { idleMs: 40, sleep: async () => {} });
      const states: string[] = [];
      const stop = t.subscribe(() => {}, (s) => states.push(s), null);
      await until(() => channels.length === 1);
      channels[0]!.onmessage(ev(9));
      await until(() => channels.length === 2);
      expect(states.slice(0, 3)).toEqual(['open', 'closed', 'open']);
      expect(lastSeqs).toEqual([null, 9]);
      channels[0]!.onmessage(ev(10)); // the dead channel is ignored
      stop();
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
