import type { EventEnvelope, RequestEnvelope, ResponseEnvelope } from '../types';
import { asEvent, TransportError, type Transport } from './transport';

/** The two Tauri entry points this transport needs (`@tauri-apps/api/core`); injectable for tests. */
export interface TauriApi {
  invoke(cmd: string, args?: Record<string, unknown>): Promise<unknown>;
  Channel: new <T>() => { onmessage: (m: T) => void };
}

/** Loads the Tauri API lazily so browser builds never evaluate it. */
export async function createTauriTransport(api?: TauriApi): Promise<Transport> {
  const { invoke, Channel } = api ?? ((await import('@tauri-apps/api/core')) as unknown as TauriApi);
  return {
    mode: 'tauri',

    async call(request: RequestEnvelope): Promise<ResponseEnvelope> {
      try {
        // In-process IPC: the envelope comes back as-is; the client validates its shape.
        return (await invoke('lw_call', { request })) as ResponseEnvelope;
      } catch (e) {
        throw new TransportError(`desktop engine not reachable: ${typeof e === 'string' ? e : e instanceof Error ? e.message : 'invoke failed'}`);
      }
    },

    subscribe(onEvent, onState, lastSeq) {
      let live = true;
      const channel = new Channel<EventEnvelope>();
      channel.onmessage = (m) => {
        const ev = asEvent(m);
        if (live && ev) onEvent(ev);
      };
      invoke('lw_events', { channel, lastSeq: lastSeq ?? null }).then(
        () => live && onState('open'),
        () => live && onState('closed'),
      );
      return () => {
        live = false;
        channel.onmessage = () => {};
      };
    },
  };
}
