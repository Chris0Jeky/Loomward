import { describe, expect, it } from 'vitest';
import type { EventEnvelope, ResponseEnvelope } from '../../src/lib/contracts.gen';
import { asEvent, asResponse } from '../../src/lib/transport/transport';

describe('transport envelope validators', () => {
  it('rejects a malformed response envelope', () => {
    expect(() => asResponse({ ok: true }, 'r1')).toThrow();
  });

  it('returns a well-formed response envelope unchanged', () => {
    const envelope: ResponseEnvelope = {
      protocol: 'loomward/3',
      request_id: 'r1',
      ok: true,
      result: {},
      meta: {
        served_at: '2026-10-01T00:00:00Z',
        elapsed_ms: 1,
        dataset_class: 'synthetic',
        budget_hit: false,
        catalog_rev: null,
        state_rev: null,
      },
    };
    expect(asResponse(envelope, 'r1')).toBe(envelope);
  });

  it('returns null for a non-event', () => {
    expect(asEvent({ type: 'nope' })).toBeNull();
  });

  it('returns a well-formed event unchanged', () => {
    const event: EventEnvelope = {
      protocol: 'loomward/3',
      epoch: 'e1',
      seq: 3,
      event: 'roots.changed',
      at: '2026-10-01T00:00:00Z',
      catalog_rev: null,
      state_rev: null,
      data: {},
    };
    expect(asEvent(event)).toBe(event);
  });
});
