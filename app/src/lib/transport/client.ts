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
  signal?: AbortSignal;
}

/** One typed entry point over any transport. Throws `LoomwardError` or `TransportError`. */
export class Client {
  private n = 0;
  constructor(readonly transport: Transport) {}

  async call<C extends CommandKey>(
    command: C,
    payload: CommandMap[C]['request'],
    opts: CallOptions = {},
  ): Promise<{ result: CommandMap[C]['result']; meta: ResponseMeta }> {
    const request_id = `r_${String(++this.n).padStart(6, '0')}`;
    const res = asResponse(
      await this.transport.call(
        { protocol: 'loomward/3', request_id, command, payload: payload as unknown as Record<string, unknown>, deadline_ms: opts.deadlineMs ?? 5000 },
        opts.signal,
      ),
      request_id,
    );
    if (!res.ok) throw new LoomwardError(res.error);
    return { result: res.result as unknown as CommandMap[C]['result'], meta: res.meta };
  }
}

export { TransportError };
