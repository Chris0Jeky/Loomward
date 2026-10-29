import type { CommandKey, CommandMap, ErrorBody, ErrorCode, ResponseMeta } from '../contracts.gen';
import { asResponse, TransportError, type Transport } from './transport';

/** The engine answered with an error envelope. */
export class LoomwardError extends Error {
  override name = 'LoomwardError';
  readonly code: ErrorCode;
  readonly retryable: boolean;
  readonly detail: ErrorBody['detail'];
  constructor(e: ErrorBody) {
    super(e.message);
    this.code = e.code;
    this.retryable = e.retryable;
    this.detail = e.detail;
  }
}

export interface CallOptions {
  deadlineMs?: number;
  expectedStateRev?: string;
  expectedGeneration?: string;
  signal?: AbortSignal;
  /** A mutation retried after an unknown outcome reuses its id, so the service replays rather than repeats it. */
  requestId?: string;
}

/** One typed entry point over any transport. Throws `LoomwardError` or `TransportError`. */
export class Client {
  private n = 0;
  // The service's idempotency cache outlives a page reload; a per-client tag keeps a reloaded page's
  // ids from replaying an earlier page's outcomes (#380 review).
  private readonly tag = Math.random().toString(36).slice(2, 10);
  constructor(readonly transport: Transport) {}

  /** A fresh request id, for a caller that must keep one across retries (`CallOptions.requestId`). */
  nextRequestId(): string {
    return `r_${this.tag}_${String(++this.n).padStart(6, '0')}`;
  }

  async call<C extends CommandKey>(
    command: C,
    payload: CommandMap[C]['request'],
    opts: CallOptions = {},
  ): Promise<{ result: CommandMap[C]['result']; meta: ResponseMeta }> {
    const request_id = opts.requestId ?? this.nextRequestId();
    const res = asResponse(
      await this.transport.call(
        { protocol: 'loomward/3', request_id, command, payload: payload as unknown as Record<string, unknown>, deadline_ms: opts.deadlineMs ?? 5000, ...(opts.expectedStateRev !== undefined ? { expected_state_rev: opts.expectedStateRev } : {}), ...(opts.expectedGeneration !== undefined ? { expected_generation: opts.expectedGeneration } : {}) },
        opts.signal,
      ),
      request_id,
    );
    if (!res.ok) throw new LoomwardError(res.error);
    return { result: res.result as unknown as CommandMap[C]['result'], meta: res.meta };
  }
}

export { TransportError };
