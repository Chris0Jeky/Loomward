import type { EventEnvelope, RequestEnvelope, ResponseEnvelope, StreamEpoch } from '../contracts.gen';

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
  /** Starts an event stream; returns a stop function. `resume` continues after that point (semantics.md section 7). */
  subscribe(
    onEvent: (event: EventEnvelope) => void,
    onState: (state: StreamState) => void,
    resume?: ResumePoint | null,
  ): () => void;
}

/** Where a stream stopped: sequence numbers are comparable only inside one epoch. */
export interface ResumePoint {
  epoch: StreamEpoch;
  seq: number;
}

/**
 * Tracks the resume point of one event stream across reconnects (semantics.md section 7).
 * `accept` returns false for a duplicate (same epoch, `seq` at or below the last applied), which the
 * transport must drop. `stream.hello` is unsequenced and never advances the point inside an epoch,
 * or a reconnect that dropped mid-replay would skip the events still to come. A new epoch, and the
 * `epoch_changed` / `replay_gap` laggeds, jump to the hello's `last_seq`: the client refetches
 * everything then, so older events are moot. Those laggeds may carry `seq` 0.
 */
export class ResumeTracker {
  point: ResumePoint | null;
  private helloLast: number | null = null;
  constructor(initial: ResumePoint | null = null) {
    this.point = initial;
  }

  accept(ev: EventEnvelope): boolean {
    if (ev.event === 'stream.hello') {
      const d = ev.data as { epoch?: unknown; last_seq?: unknown };
      if (typeof d.epoch === 'string' && Number.isSafeInteger(d.last_seq)) {
        this.helloLast = d.last_seq as number;
        if (this.point?.epoch !== d.epoch) this.point = { epoch: d.epoch, seq: d.last_seq as number };
      }
      return true;
    }
    const reason = ev.event === 'stream.lagged' ? (ev.data as { reason?: unknown }).reason : null;
    if (reason === 'epoch_changed' || reason === 'replay_gap') {
      const seq = Math.max(this.helloLast ?? 0, this.point?.epoch === ev.epoch ? this.point.seq : 0);
      this.point = { epoch: ev.epoch, seq };
      return true;
    }
    if (this.point?.epoch === ev.epoch) {
      if (ev.seq <= this.point.seq) return false;
      this.point.seq = ev.seq;
    } else {
      this.point = { epoch: ev.epoch, seq: ev.seq };
    }
    return true;
  }
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
  if (typeof v.epoch !== 'string' || v.epoch === '') return null;
  return v as unknown as EventEnvelope;
}
