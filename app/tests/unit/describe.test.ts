// describeNode (Atlas and Observatory announcements): an unknown size is never spoken as "0 B" (invariant 4).
import { describe, expect, it } from 'vitest';
import { deniedBelow, describeNode, describeSize, sizeUnknown } from '../../src/views/atlas/shared.svelte';
import type { NodeInfo, SliceLike } from '../../viz/types.js';

const THREADS = { permission: { state: 'granted', reason: null }, residency: { volume_id: 'v', tier: 1, tier_basis: 'declared' }, meaning: { state: 'labelled', label: 'Projects' } };
const node = (src: Record<string, unknown> | null, over: Partial<NodeInfo> = {}) =>
  ({
    id: 'n', name: 'Docs', label: 'Docs', size: 0, synthetic: null, folded: 0, zero: [], parentId: null, drillable: false,
    src: src && { kind: 'dir', coverage: 'complete', size_unknown_files: 0, threads: THREADS, ...src },
    ...over,
  }) as unknown as NodeInfo;

describe('describeSize', () => {
  it('a size the slice does not carry reads unknown, not 0 B (layout turns it into 0)', () => {
    for (const bad of [null, 'garbage', '']) expect(describeSize(node({ size_bytes: bad }))).toMatch(/size unknown/);
    expect(describeNode(node({ size_bytes: null }))).not.toContain('0 B');
  });
  it('a denied node with size 0 reads unknown and says why', () => {
    const denied = node({ size_bytes: '0', coverage: 'denied', threads: { ...THREADS, permission: { state: 'denied', reason: null } } });
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

describe('unmeasured coverage is unknown, not 0 B (#169)', () => {
  it.each(['unscanned', 'unknown', 'stale', 'cancelled'])('size 0 under coverage %s reads unknown', (coverage) => {
    expect(describeSize(node({ size_bytes: '0', coverage }))).toMatch(/size unknown/);
    expect(sizeUnknown({ size_bytes: '0', coverage, size_unknown_files: 0, threads: THREADS } as never, false)).toBe(true);
  });
  it('a measured size under those coverages, and size 0 of a complete or partial folder, stay as reported', () => {
    expect(describeSize(node({ size_bytes: '2048', coverage: 'stale' }))).toContain('2.00 KiB');
    expect(describeSize(node({ size_bytes: '0', coverage: 'complete' }))).toContain('0 B');
    expect(describeSize(node({ size_bytes: '0', coverage: 'partial' }))).toContain('0 B');
  });
  it('files of unknown allocation make 0 unknown only under the allocated basis', () => {
    const n = { size_bytes: '0', coverage: 'complete', size_unknown_files: 2, threads: THREADS } as never;
    expect(sizeUnknown(n, true)).toBe(true);
    expect(sizeUnknown(n, false)).toBe(false);
  });
});

describe('the spoken description names the kind and the thread states (#157 text alternative)', () => {
  it('says kind, tier, meaning and a non-plain permission, and keeps the name first', () => {
    const said = describeNode(node({ size_bytes: '2048', kind: 'root', threads: { ...THREADS, permission: { state: 'partial', reason: null } } }, { drillable: true }));
    expect(said.startsWith('Docs, root folder, 2.00 KiB')).toBe(true);
    expect(said).toContain('tier 1');
    expect(said).toContain('meaning labelled Projects');
    expect(said).toContain('permission partial');
    expect(said).toContain('Enter opens it.');
  });
  it('unknown residency is said, not left out', () => {
    const said = describeNode(node({ size_bytes: '1', threads: { ...THREADS, residency: { volume_id: null, tier: null, tier_basis: 'unknown' } } }));
    expect(said).toContain('residency unknown');
  });
  it('a label with a hidden character is spelled out, not spoken raw', () => {
    const said = describeNode(node({ size_bytes: '1', threads: { ...THREADS, meaning: { state: 'labelled', label: 'ab‮cd' } } }));
    expect(said).not.toContain('‮');
    expect(said).toContain('U+202E');
  });
  it('counts denied nodes below, as a lower bound, when the slice is given', () => {
    const mk = (id: string, parent: number | null, coverage = 'complete', perm = 'granted') =>
      ({ node_id: id, parent, kind: 'dir', name: id, size_bytes: '0', size_unknown_files: 0, coverage, threads: { ...THREADS, permission: { state: perm, reason: null } } });
    const slice = { nodes: [mk('a', null), mk('b', 0), mk('c', 1, 'denied'), mk('d', 1, 'complete', 'denied'), mk('e', 0, 'denied')] } as unknown as SliceLike;
    const below = deniedBelow(slice);
    expect(below.get('a')).toBe(3);
    expect(below.get('b')).toBe(2);
    expect(below.get('c')).toBeUndefined(); // a denied node is not below itself
    const n = node({ size_bytes: '5' }, { id: 'b' });
    expect(describeSize(n, below)).toContain('at least 2 access denied below');
    expect(describeSize(n, new Map())).not.toContain('access denied');
  });
  it('a malformed slice (a parent that does not precede its child) ends the walk instead of looping', () => {
    const cyc = { nodes: [{ node_id: 'a', parent: 1, coverage: 'denied', threads: THREADS }, { node_id: 'b', parent: 0, coverage: 'complete', threads: THREADS }] } as unknown as SliceLike;
    expect(() => deniedBelow(cyc)).not.toThrow();
  });
});
