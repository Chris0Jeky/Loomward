import { describe, expect, it } from 'vitest';
import fixture from '../../../fixtures/v3/query-grammar-parity.json';
import { Client, LoomwardError } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { modifiedBounds, parseQuery } from '../../src/lib/search/grammar';
import type { SearchRequest } from '../../src/lib/contracts.gen';

// The same file drives crates/loomward-protocol/tests/query_grammar.rs (docs/46 F1).
describe('query grammar parity with the Rust parser', () => {
  it('parses every shared case to the expected JSON', () => {
    expect(fixture.cases.length).toBeGreaterThanOrEqual(90);
    for (const c of fixture.cases) expect(parseQuery(c.input), `${c.name}: ${JSON.stringify(c.input)}`).toEqual(c.expected);
  });

  it('resolves relative modified times from now', () => {
    for (const b of fixture.bounds) {
      expect(modifiedBounds(parseQuery(b.input), fixture.now_ms), b.input).toEqual({ after: b.after, before: b.before });
    }
  });

  it('is total: no prefix of a fixture input throws, and terms stay within the wire bounds', () => {
    for (const c of fixture.cases) {
      const cs = Array.from(c.input);
      for (let n = 0; n <= cs.length; n++) {
        const q = parseQuery(cs.slice(0, n).join(''));
        expect(q.terms.length).toBeLessThanOrEqual(8);
        expect(q.extensions.length).toBeLessThanOrEqual(16);
        for (const t of q.terms) expect(Array.from(t.text).length).toBeLessThanOrEqual(64);
      }
    }
  });
});

describe('federated search contract (F1)', () => {
  const base: SearchRequest = { root_id: null, text: 'a', extension: null, min_bytes: null, max_bytes: null, modified_after: null, modified_before: null, kind: 'any', limit: 5, cursor: null };
  const c = new Client(createMockTransport({ now: () => new Date('2026-10-01T00:00:00Z') }));

  it('advertises no federated search until an engine serves it', async () => {
    expect((await c.call('session.hello', {})).result.features.federated_search).toBe(false);
  });

  it('serves v1 requests unchanged and refuses each federated field rather than ignoring it', async () => {
    await c.call('search.query', base);
    for (const extra of [{ sources: ['own'] }, { terms: [{ text: 'a', mode: 'token', negated: false }] }, { extensions: ['txt'] }, { collection_id: 'co_x' }, { under_node_id: 'nd_x' }]) {
      const e = await c.call('search.query', { ...base, ...extra } as SearchRequest).catch((x) => x);
      expect(e).toBeInstanceOf(LoomwardError);
      expect((e as LoomwardError).code).toBe('capability_unavailable');
    }
  });
});
