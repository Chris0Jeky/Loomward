import type { EventEnvelope, RequestEnvelope, ResponseEnvelope } from '../types';
import { asEvent, TransportError, type Transport } from './transport';

/** The two Tauri entry points this transport needs (`@tauri-apps/api/core`); injectable for tests. */
export interface TauriApi {
  invoke(cmd: string, args?: Record<string, unknown>): Promise<unknown>;
  Channel: new <T>() => { onmessage: (m: T) => void };
}

export interface TauriOptions {
  /**
   * The channel is declared dead when nothing arrives for this long. The engine must send something at
   * least every 15 s (the SSE heartbeat contract), so three missed beats by default.
   */
  idleMs?: number;
  sleep?: (ms: number) => Promise<void>;
}

const IDLE_MS = 45_000;
const BACKOFF_START_MS = 500;
const BACKOFF_MAX_MS = 15_000;

/** Loads the Tauri API lazily so browser builds never evaluate it. */
export async function createTauriTransport(api?: TauriApi, opts: TauriOptions = {}): Promise<Transport> {
  const { invoke, Channel } = api ?? ((await import('@tauri-apps/api/core')) as unknown as TauriApi);
  const idleMs = opts.idleMs ?? IDLE_MS;
  const sleep = opts.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
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
      let stopped = false;
      let lastId: number | null = lastSeq ?? null;
      let delay = BACKOFF_START_MS;
      let cancelWait: (() => void) | undefined;

      async function run() {
        while (!stopped) {
          // One channel per attempt: a dead channel can never deliver into a newer one.
          let attemptLive = true;
          let beat!: () => void;
          const silent = new Promise<void>((resolve) => {
            let timer: ReturnType<typeof setTimeout>;
            beat = () => { clearTimeout(timer); timer = setTimeout(resolve, idleMs); };
            cancelWait = () => { clearTimeout(timer); resolve(); };
          });
          const channel = new Channel<EventEnvelope>();
          channel.onmessage = (m) => {
            if (!attemptLive || stopped) return;
            beat(); // any message, even a malformed one, proves the channel is alive
            const ev = asEvent(m);
            if (ev) {
              lastId = ev.seq;
              onEvent(ev);
            }
          };
          try {
            await invoke('lw_events', { channel, lastSeq: lastId });
            if (stopped) return;
            onState('open');
            delay = BACKOFF_START_MS;
            beat();
            await silent;
          } catch {
            /* subscribe refused: fall through to closed */
          }
          attemptLive = false;
          channel.onmessage = () => {};
          if (stopped) return;
          onState('closed');
          await sleep(delay);
          delay = Math.min(delay * 2, BACKOFF_MAX_MS);
        }
      }
      void run();
      return () => {
        stopped = true;
        cancelWait?.();
      };
    },
  };
}
