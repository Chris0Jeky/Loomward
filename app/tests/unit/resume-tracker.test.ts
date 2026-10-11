// ResumeTracker.helloLast follows the latest stream.hello only (semantics.md section 7, #152).
import { describe, expect, it } from 'vitest';
import type { EventEnvelope } from '../../src/lib/contracts.gen';
import { ResumeTracker } from '../../src/lib/transport/transport';

const env = (seq: number, event: string, data: object): EventEnvelope =>
  ({ protocol: 'loomward/3', epoch: 'e1', seq, event, at: '2026-10-11T00:00:00Z', catalog_rev: null, state_rev: null, data }) as unknown as EventEnvelope;

describe('ResumeTracker hello memory', () => {
  it('uses the last_seq of the latest hello for a replay_gap jump', () => {
    const t = new ResumeTracker({ epoch: 'e1', seq: 40 });
    t.accept(env(500, 'stream.hello', { epoch: 'e1', last_seq: 500 }));
    t.accept(env(0, 'stream.lagged', { reason: 'replay_gap' }));
    expect(t.point).toEqual({ epoch: 'e1', seq: 500 });
  });

  it('forgets the last_seq of an earlier connection when a later hello lacks a valid one', () => {
    const t = new ResumeTracker({ epoch: 'e1', seq: 40 });
    t.accept(env(500, 'stream.hello', { epoch: 'e1', last_seq: 500 }));
    t.accept(env(0, 'stream.hello', { epoch: 'e1' })); // reconnect: this hello has no last_seq
    t.accept(env(0, 'stream.lagged', { reason: 'replay_gap' }));
    expect(t.point).toEqual({ epoch: 'e1', seq: 40 });
  });
});
