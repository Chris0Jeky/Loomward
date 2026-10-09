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
});
