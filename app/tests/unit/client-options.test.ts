import { describe, expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import type { RequestEnvelope, ResponseMeta } from '../../src/lib/contracts.gen';
import type { Transport } from '../../src/lib/transport/transport';

const meta: ResponseMeta = {
  served_at: '2026-10-01T00:00:00Z',
  elapsed_ms: 1,
  dataset_class: 'synthetic',
  budget_hit: false,
  catalog_rev: '1',
  state_rev: null,
};

/** Fake transport recording each request and answering with a valid ok envelope. */
function makeClient(seen: RequestEnvelope[]): Client {
  const transport: Transport = {
    mode: 'mock',
    call: async (request) => {
      seen.push(request);
      return {
        protocol: 'loomward/3',
        request_id: request.request_id,
        ok: true,
        result: {},
        meta,
      };
    },
    subscribe: () => () => {},
  };
  return new Client(transport);
}

describe('Client.call() envelope options', () => {
  it('sends the default deadline and omits expected keys entirely', async () => {
    const seen: RequestEnvelope[] = [];
    await makeClient(seen).call('roots.list', {});
    expect(seen).toHaveLength(1);
    expect(seen[0]!.deadline_ms).toBe(5000);
    expect('expected_state_rev' in seen[0]!).toBe(false);
    expect('expected_generation' in seen[0]!).toBe(false);
  });

  it('forwards an explicit deadline and both expected values verbatim', async () => {
    const seen: RequestEnvelope[] = [];
    await makeClient(seen).call(
      'collections.update_members',
      { collection_id: 'col_test', add: ['nd_test'], remove: [] },
      { expectedStateRev: '7', expectedGeneration: 'g1', deadlineMs: 123 },
    );
    expect(seen).toHaveLength(1);
    expect(seen[0]!.deadline_ms).toBe(123);
    expect(seen[0]!.expected_state_rev).toBe('7');
    expect(seen[0]!.expected_generation).toBe('g1');
  });

  it('reuses an explicit requestId across calls', async () => {
    const seen: RequestEnvelope[] = [];
    const client = makeClient(seen);
    await client.call('roots.list', {}, { requestId: 'r_fixed_1' });
    await client.call('roots.list', {}, { requestId: 'r_fixed_1' });
    expect(seen).toHaveLength(2);
    expect(seen[0]!.request_id).toBe('r_fixed_1');
    expect(seen[1]!.request_id).toBe('r_fixed_1');
    expect(seen[1]!.request_id).toBe(seen[0]!.request_id);
  });
});
