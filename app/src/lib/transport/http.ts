import type { EventEnvelope, RequestEnvelope, ResponseEnvelope } from '../contracts.gen';
import { SseParser, type SseLimits } from './sse';
import { asEvent, ResumeTracker, TransportError, type StreamState, type Transport } from './transport';

export interface HttpOptions {
  /** Origin of loomward-serve, no trailing slash. */
  base: string;
  /** Session token from the URL fragment; held in memory and sent only in the custom header. */
  token: string;
  fetchImpl?: typeof fetch;
  /** Injected so tests can run the reconnect loop without real delays. */
  sleep?: (ms: number) => Promise<void>;
  /** Overrides the event-stream size bounds (tests only). */
  sseLimits?: SseLimits;
}

const MAX_RESPONSE_BYTES = 8 * 1024 * 1024;
/** Reads a body while counting bytes, so the cap bounds memory, not just what is kept afterwards. */
async function readCapped(res: Response, max: number): Promise<string> {
  const declared = Number(res.headers.get('Content-Length'));
  if (Number.isFinite(declared) && declared > max) {
    void res.body?.cancel().catch(() => {});
    throw new TransportError('response too large');
  }
  if (!res.body) return '';
  const reader = res.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > max) {
      void reader.cancel().catch(() => {});
      throw new TransportError('response too large');
    }
    chunks.push(value);
  }
  const all = new Uint8Array(size);
  let at = 0;
  for (const c of chunks) { all.set(c, at); at += c.byteLength; }
  return new TextDecoder().decode(all);
}

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
        return JSON.parse(await readCapped(res, MAX_RESPONSE_BYTES)) as ResponseEnvelope;
      } catch (e) {
        throw e instanceof TransportError ? e : new TransportError('response is not JSON');
      }
    },

    subscribe(onEvent, onState, resume) {
      const stop = new AbortController();
      const tracker = new ResumeTracker(resume ?? null);
      let delay = BACKOFF_START_MS;

      const emit = (state: StreamState) => {
        if (!stop.signal.aborted) onState(state);
      };

      async function run() {
        while (!stop.signal.aborted) {
          try {
            const h: Record<string, string> = { ...headers, Accept: 'text/event-stream' };
            // The frame id is `<epoch>.<seq>`; a bare seq reads as another epoch to the service (`epoch_changed`).
            if (tracker.point) h['Last-Event-ID'] = `${tracker.point.epoch}.${tracker.point.seq}`;
            const res = await doFetch(`${opts.base}/api/v3/events`, {
              headers: h,
              cache: 'no-store',
              credentials: 'omit',
              signal: stop.signal,
            });
            if (res.status !== 200 || !res.body) throw new TransportError(`event stream HTTP ${res.status}`);
            emit('open');
            delay = BACKOFF_START_MS;
            const parser = new SseParser(opts.sseLimits);
            const decoder = new TextDecoder();
            const reader = res.body.getReader();
            try {
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
                  if (!ev || !tracker.accept(ev)) continue;
                  onEvent(ev);
                }
              }
            } finally {
              void reader.cancel().catch(() => {}); // overflow or error: do not leave the connection streaming
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
