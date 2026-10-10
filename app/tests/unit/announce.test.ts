// Background refetches do not repeat "Showing ..." (PR #168 F3); Say re-announces an identical line on purpose.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { announceSlice } from '../../src/views/atlas/shared.svelte';
import { Say } from '../../src/lib/ui/say.svelte';
import type { SliceLike } from '../../viz/types.js';

const slice = (over: Partial<SliceLike> = {}, n = 5) =>
  ({ anchor_node_id: 'a', basis: 'logical', complete: true, truncated: false, aggregate_state: 'consistent', nodes: Array.from({ length: n }, () => ({})), ...over }) as unknown as SliceLike;

describe('announceSlice', () => {
  it('the first slice is announced, with the escaped name', () => {
    const r = announceSlice(null, slice(), 'rep\u202Eorts');
    expect(r.text).toBe('Showing rep' + 'U+202E RLO' + 'orts: 5 nodes.');
  });
  it('a refetch of the same view is silent, even when the node count moved (a scan adds nodes all the time)', () => {
    const first = announceSlice(null, slice(), 'A');
    expect(announceSlice(first.key, slice({}, 7), 'A').text).toBeNull();
    expect(announceSlice(first.key, slice({}, 7), 'A').key).toEqual(first.key);
  });
  it('drill, back, jump and a basis change always announce', () => {
    const first = announceSlice(null, slice(), 'A');
    expect(announceSlice(first.key, slice({ anchor_node_id: 'b' }), 'B').text).toContain('Showing B');
    expect(announceSlice(first.key, slice({ basis: 'allocated' }), 'A').text).toContain('Showing A');
  });
  it('turning provisional or truncated says so', () => {
    const first = announceSlice(null, slice(), 'A');
    expect(announceSlice(first.key, slice({ aggregate_state: 'provisional_live' }), 'A').text).toContain('provisional');
    expect(announceSlice(first.key, slice({ truncated: true }), 'A').text).toContain('more exist');
  });
  it('after a failed load (no key) the recovered slice is announced again', () => {
    expect(announceSlice(null, slice(), 'A').text).not.toBeNull();
  });
});

describe('Say', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());
  it('empties the region first and fills it later, so an identical repeat is a change', () => {
    const s = new Say();
    s.say('Nothing further left.');
    expect(s.text).toBe('');
    vi.advanceTimersByTime(100);
    expect(s.text).toBe('Nothing further left.');
    s.say('Nothing further left.');
    expect(s.text).toBe('');
    vi.advanceTimersByTime(100);
    expect(s.text).toBe('Nothing further left.');
  });
  it('a newer line replaces a pending one; clear drops it', () => {
    const s = new Say();
    s.say('one'); s.say('two');
    vi.advanceTimersByTime(100);
    expect(s.text).toBe('two');
    s.say('three'); s.clear();
    vi.advanceTimersByTime(100);
    expect(s.text).toBe('');
  });
});
