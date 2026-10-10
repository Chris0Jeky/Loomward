import type { EventEnvelope, RequestEnvelope, ResponseEnvelope } from '../contracts.gen';
import { asEvent, ResumeTracker, TransportError, type Transport } from './transport';

/** The two Tauri entry points this transport needs (`@tauri-apps/api/core`); injectable for tests. */
export interface TauriApi {
  invoke(cmd: string, args?: Record<string, unknown> | Uint8Array): Promise<unknown>;
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
        return (await invoke('lw_call', new TextEncoder().encode(JSON.stringify(request)))) as ResponseEnvelope;
      } catch (e) {
        throw new TransportError(`desktop engine not reachable: ${typeof e === 'string' ? e : e instanceof Error ? e.message : 'invoke failed'}`);
      }
    },

    subscribe(onEvent, onState, resume) {
      let stopped = false;
      const tracker = new ResumeTracker(resume ?? null);
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
          let lagged = false;
          channel.onmessage = (m) => {
            if (!attemptLive || stopped) return;
            beat(); // any message, even a malformed one, proves the channel is alive
            const ev = asEvent(m);
            // Channel.send is fire-and-forget: discard queued stale data after a renderer stall.
            if (ev && Date.now() - Date.parse(ev.at) > 5000) {
              if (!lagged) {
                const gap: EventEnvelope = { ...ev, event: 'stream.lagged', at: new Date().toISOString(),
                  data: { reason: 'subscriber_overflow', dropped: null, resync: ['roots', 'volumes', 'jobs', 'tree', 'learning'] } };
                if (tracker.accept(gap)) onEvent(gap);
              }
              lagged = true;
              return;
            }
            lagged = false;
            if (ev && tracker.accept(ev)) onEvent(ev);
          };
          try {
            await invoke('lw_events', { channel, lastEpoch: tracker.point?.epoch ?? null, lastSeq: tracker.point?.seq ?? null });
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
