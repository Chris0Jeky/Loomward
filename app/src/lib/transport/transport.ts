import type { EventEnvelope, RequestEnvelope, ResponseEnvelope } from '../types';

export type TransportMode = 'mock' | 'http' | 'tauri';

/** The engine could not be reached or answered with something that is not an envelope. */
export class TransportError extends Error {
  override name = 'TransportError';
}

/** Event stream state as seen by the shell: `closed` means the shell must show "unavailable". */
export type StreamState = 'open' | 'closed';

export interface Transport {
  readonly mode: TransportMode;
  /** Resolves with the response envelope (ok or error); rejects with `TransportError` on a transport fault. */
  call(request: RequestEnvelope, signal?: AbortSignal): Promise<ResponseEnvelope>;
  /** Starts an event stream; returns a stop function. `lastSeq` resumes after that sequence number. */
  subscribe(
    onEvent: (event: EventEnvelope) => void,
    onState: (state: StreamState) => void,
    lastSeq?: number | null,
  ): () => void;
}

const EVENT_NAMES = new Set([
  'stream.hello', 'stream.lagged', 'job.state', 'scan.progress', 'tree.invalidated',
  'telemetry.sample', 'learning.updated', 'roots.changed', 'volumes.changed', 'health.warning',
]);

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

/** Shape check for a response envelope; anything else is a transport fault, not data. */
export function asResponse(v: unknown, requestId: string): ResponseEnvelope {
  if (!isObject(v) || v.protocol !== 'loomward/3' || v.request_id !== requestId || typeof v.ok !== 'boolean') {
    throw new TransportError('response is not a loomward/3 envelope for this request');
  }
  if (v.ok ? !isObject(v.meta) : !isObject(v.error)) throw new TransportError('malformed response envelope');
  return v as unknown as ResponseEnvelope;
}

/** Shape check for an event envelope; returns null for anything that does not fit (frame is dropped). */
export function asEvent(v: unknown): EventEnvelope | null {
  if (!isObject(v) || v.protocol !== 'loomward/3' || typeof v.event !== 'string' || !EVENT_NAMES.has(v.event)) return null;
  if (!Number.isSafeInteger(v.seq) || (v.seq as number) < 0 || !isObject(v.data)) return null;
  return v as unknown as EventEnvelope;
}
