import { afterEach, describe, expect, it } from 'vitest';
import { session } from '../../src/lib/stores/session.svelte';
import type { EventEnvelope } from '../../src/lib/contracts.gen';

const env = (event: EventEnvelope['event'], data: Record<string, unknown>): EventEnvelope =>
  ({ protocol: 'loomward/3', epoch: 'e1', seq: 0, event, at: '2026-10-01T00:00:00Z', catalog_rev: null, state_rev: null, data });
const hello = (epoch: string) => env('stream.hello', { session_started_at: 'x', epoch, last_seq: 0, oldest_replayable_seq: 1, dataset_class: 'synthetic' });
const lagged = (reason: string) => env('stream.lagged', { reason, dropped: null, resync: ['roots'] });
const feed = (e: EventEnvelope) => (session as unknown as { onEvent(e: EventEnvelope): void }).onEvent(e);

describe('session resync', () => {
  afterEach(() => { (session as unknown as { streamEpoch: string | null }).streamEpoch = null; });

  it('does not resync on the first hello nor on a heartbeat hello of the same epoch', () => {
    const before = session.epoch;
    feed(hello('e1'));
    feed(hello('e1'));
    expect(session.epoch).toBe(before);
  });

  it('resyncs when a hello names a different epoch (the engine restarted)', () => {
    feed(hello('e1'));
    const before = session.epoch;
    feed(hello('e2'));
    expect(session.epoch).toBe(before + 1);
  });

  it.each(['epoch_changed', 'replay_gap', 'subscriber_overflow'])('resyncs on stream.lagged %s', (reason) => {
    const before = session.epoch;
    feed(lagged(reason));
    expect(session.epoch).toBe(before + 1);
  });
});
