// describeNode (Atlas and Observatory announcements): an unknown size is never spoken as "0 B" (invariant 4).
import { describe, expect, it } from 'vitest';
import { describeNode, describeSize } from '../../src/views/atlas/shared.svelte';
import type { NodeInfo } from '../../viz/types.js';

const node = (src: Record<string, unknown> | null, over: Partial<NodeInfo> = {}) =>
  ({
    id: 'n', name: 'Docs', label: 'Docs', size: 0, synthetic: null, folded: 0, zero: [], parentId: null, drillable: false,
    src: src && { kind: 'dir', coverage: 'complete', size_unknown_files: 0, threads: { permission: { state: 'granted', reason: null } }, ...src },
    ...over,
  }) as unknown as NodeInfo;

describe('describeSize', () => {
  it('a size the slice does not carry reads unknown, not 0 B (layout turns it into 0)', () => {
    for (const bad of [null, 'garbage', '']) expect(describeSize(node({ size_bytes: bad }))).toMatch(/size unknown/);
    expect(describeNode(node({ size_bytes: null }))).not.toContain('0 B');
  });
  it('a denied node with size 0 reads unknown and says why', () => {
    const denied = node({ size_bytes: '0', coverage: 'denied', threads: { permission: { state: 'denied', reason: null } } });
    expect(describeSize(denied)).toMatch(/size unknown.*access denied/);
    expect(describeNode(denied)).not.toContain('0 B');
  });
  it('size 0 with files of unknown allocation is unknown too', () => {
    expect(describeSize(node({ size_bytes: '0', size_unknown_files: 3 }))).toMatch(/size unknown/);
  });
  it('a known size is spoken exactly; a real empty folder is 0 B', () => {
    expect(describeSize(node({ size_bytes: '2048' }, { size: 2048 }))).toContain('2.00 KiB');
    expect(describeNode(node({ size_bytes: '0' }))).toContain('0 B');
  });
  it('a synthetic cell keeps its derived size and its note', () => {
    expect(describeSize(node(null, { synthetic: 'remainder', size: 1024 }))).toContain('1.00 KiB');
  });
});
