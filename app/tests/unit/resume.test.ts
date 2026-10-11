import { describe, expect, it } from 'vitest';
import type { EventEnvelope } from '../../src/lib/contracts.gen';
import { ResumeTracker } from '../../src/lib/transport/transport';

const hello = (last_seq: unknown, epoch = 'e1', seq = 5): EventEnvelope => ({
  protocol: 'loomward/3', epoch, seq, event: 'stream.hello', at: '2026-10-01T00:00:00Z',
  catalog_rev: null, state_rev: null,
  data: { epoch, last_seq, oldest_replayable_seq: 1 },
});
const gap = (epoch = 'e1'): EventEnvelope => ({ ...hello(5, epoch), seq: 0, event: 'stream.lagged', data: { reason: 'replay_gap' } });

describe('hello resume watermark', () => {
  it.each([-1, 2.5, '99', Number.MAX_SAFE_INTEGER + 1, null, undefined])('ignores invalid last_seq %s without moving the cursor', (last) => {
    const first = new ResumeTracker();
    first.accept(hello(last));
    expect(first.point).toBeNull();
    const tracker = new ResumeTracker({ epoch: 'e1', seq: 2 });
    tracker.accept(hello(5));
    tracker.accept(hello(last));
    tracker.accept(gap());
    expect(tracker.point).toEqual({ epoch: 'e1', seq: 2 });
    const fresh: EventEnvelope = { ...hello(3), event: 'roots.changed', seq: 3, data: {} };
    expect(tracker.accept(fresh)).toBe(true);
    expect(tracker.accept(fresh)).toBe(false);
  });

  it.each([['e1', 2], ['e0', 40]] as const)('clamps a future watermark to the envelope sequence when resuming from %s', (epoch, seq) => {
    const tracker = new ResumeTracker({ epoch, seq });
    tracker.accept(hello(99));
    tracker.accept(gap());
    expect(tracker.point).toEqual({ epoch: 'e1', seq: 5 });
    expect(tracker.accept({ ...hello(6), event: 'roots.changed', seq: 6, data: {} })).toBe(true);
  });

  it('does not use a previous epoch watermark after an invalid hello', () => {
    const tracker = new ResumeTracker();
    tracker.accept(hello(40, 'e1', 40));
    tracker.accept(hello(-1, 'e2'));
    tracker.accept(gap('e2'));
    expect(tracker.point).toEqual({ epoch: 'e2', seq: 0 });
  });

  it('does not trust a payload epoch that differs from the envelope', () => {
    const tracker = new ResumeTracker({ epoch: 'e1', seq: 2 });
    tracker.accept({ ...hello(5), data: { epoch: 'e_other', last_seq: 5 } });
    expect(tracker.point).toEqual({ epoch: 'e1', seq: 2 });
  });
});
