import type { EventEnvelope, RequestEnvelope, ResponseEnvelope } from '../types';
import { SseParser } from './sse';
import { asEvent, TransportError, type StreamState, type Transport } from './transport';

export interface HttpOptions {
  /** Origin of loomward-serve, no trailing slash. */
  base: string;
  /** Session token from the URL fragment; held in memory and sent only in the custom header. */
  token: string;
  fetchImpl?: typeof fetch;
  /** Injected so tests can run the reconnect loop without real delays. */
  sleep?: (ms: number) => Promise<void>;
}

const MAX_RESPONSE_CHARS = 8 * 1024 * 1024;
const BACKOFF_START_MS = 500;
const BACKOFF_MAX_MS = 15_000;

export function createHttpTransport(opts: HttpOptions): Transport {
  const doFetch = opts.fetchImpl ?? ((...a: Parameters<typeof fetch>) => fetch(...a));
  const sleep = opts.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  const headers = { 'X-Loomward-Token': opts.token };

  return {
    mode: 'http',

    async call(request: RequestEnvelope, signal?: AbortSignal): Promise<ResponseEnvelope> {
      let res: Response;
      try {
        res = await doFetch(`${opts.base}/api/v3/call`, {
          method: 'POST',
          headers: { ...headers, 'Content-Type': 'application/json' },
          body: JSON.stringify(request),
          cache: 'no-store',
          credentials: 'omit',
          signal,
        });
      } catch (e) {
        if (signal?.aborted) throw e;
        throw new TransportError(`engine not reachable: ${e instanceof Error ? e.message : String(e)}`);
      }
      // Always HTTP 200 with an envelope; any other status is a transport fault (docs/41 section 5.1).
      if (res.status !== 200) throw new TransportError(`engine answered HTTP ${res.status}`);
      try {
        const text = await res.text();
        if (text.length > MAX_RESPONSE_CHARS) throw new TransportError('response too large');
        return JSON.parse(text) as ResponseEnvelope;
      } catch (e) {
        throw e instanceof TransportError ? e : new TransportError('response is not JSON');
      }
    },

    subscribe(onEvent, onState, lastSeq) {
      const stop = new AbortController();
      let lastId: number | null = lastSeq ?? null;
      let delay = BACKOFF_START_MS;

      const emit = (state: StreamState) => {
        if (!stop.signal.aborted) onState(state);
      };

      async function run() {
        while (!stop.signal.aborted) {
          try {
            const h: Record<string, string> = { ...headers, Accept: 'text/event-stream' };
            if (lastId !== null) h['Last-Event-ID'] = String(lastId);
            const res = await doFetch(`${opts.base}/api/v3/events`, {
              headers: h,
              cache: 'no-store',
              credentials: 'omit',
              signal: stop.signal,
            });
            if (res.status !== 200 || !res.body) throw new TransportError(`event stream HTTP ${res.status}`);
            emit('open');
            delay = BACKOFF_START_MS;
            const parser = new SseParser();
            const decoder = new TextDecoder();
            const reader = res.body.getReader();
            for (;;) {
              const { done, value } = await reader.read();
              if (done) break;
              for (const frame of parser.push(decoder.decode(value, { stream: true }))) {
                let ev: EventEnvelope | null = null;
                try {
                  ev = asEvent(JSON.parse(frame.data));
                } catch {
                  /* garbled frame: dropped */
                }
                if (!ev) continue;
                lastId = ev.seq;
                onEvent(ev);
              }
            }
          } catch {
            if (stop.signal.aborted) return;
          }
          emit('closed');
          await sleep(delay);
          delay = Math.min(delay * 2, BACKOFF_MAX_MS);
        }
      }
      void run();
      return () => stop.abort();
    },
  };
}
