import { describe, expect, it } from 'vitest';
import { asResponse, TransportError } from '../../src/lib/transport/transport.js';

describe('asResponse', () => {
  it('rejects an ok envelope with no result field', () => {
    const envelope = { protocol: 'loomward/3', request_id: 'r_1', ok: true, meta: {} };
    expect(() => asResponse(envelope, 'r_1')).toThrow(TransportError);
  });

  it('rejects error envelopes whose error is not an object with a string code', () => {
    expect(() =>
      asResponse({ protocol: 'loomward/3', request_id: 'r_1', ok: false, error: 'boom' }, 'r_1'),
    ).toThrow(TransportError);
    expect(() =>
      asResponse({ protocol: 'loomward/3', request_id: 'r_1', ok: false, error: { code: 42 } }, 'r_1'),
    ).toThrow(TransportError);
  });

  it('accepts an ok envelope with an object result', () => {
    const envelope = { protocol: 'loomward/3', request_id: 'r_1', ok: true, result: {}, meta: {} };
    expect(asResponse(envelope, 'r_1')).toBe(envelope);
  });

  it('accepts an error envelope with an object error containing a string code', () => {
    const envelope = {
      protocol: 'loomward/3',
      request_id: 'r_1',
      ok: false,
      error: { code: 'NO_SUCH_ROOT' },
    };
    expect(asResponse(envelope, 'r_1')).toBe(envelope);
  });
});
