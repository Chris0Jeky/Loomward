// RegionList "Not in this slice" remainder: an unknown input makes the remainder unknown, never a byte count (invariant 4).
import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import RegionList from '../../src/views/atlas/RegionList.svelte';
import type { SliceNode, TreeSlice } from '../../src/lib/contracts.gen';

const THREADS = { permission: { state: 'granted', reason: null }, residency: { volume_id: 'v', tier: 1, tier_basis: 'declared' }, meaning: { state: 'none', label: null } };
const node = (over: Record<string, unknown>): SliceNode => ({
  node_id: 'n', parent: 0, kind: 'dir', name: 'N', depth: 1, size_bytes: '0', size_unknown_files: 0, logical_bytes: '0', allocated_bytes: null,
  files: 0, dirs: 0, child_count: 0, folded_count: null, coverage: 'complete', live: false, ext_family: null, modified_at: null, threads: THREADS, ...over,
}) as unknown as SliceNode;
const html = (nodes: SliceNode[], basis: 'logical' | 'allocated' = 'logical') =>
  render(RegionList, { props: { slice: { basis, nodes } as unknown as TreeSlice, onopen: () => undefined, oninspect: () => undefined } }).body;
const root = (size: unknown) => node({ node_id: 'r', parent: null, kind: 'root', depth: 0, size_bytes: size });
const rest = (body: string) => /<li class="rest[^"]*">(.*?)<\/li>/s.exec(body)?.[1]!.replace(/<[^>]+>/g, '').replace(/\s+/g, ' ').trim() ?? null;

describe('RegionList remainder', () => {
  it('is the exact difference when every size is known', () => {
    expect(rest(html([root('1000'), node({ node_id: 'a', size_bytes: '600' })]))).toBe('Not in this slice 400 B');
    expect(rest(html([root('1000'), node({ node_id: 'a', size_bytes: '1000' })]))).toBeNull();
  });
  it.each([['malformed', 'bogus'], ['missing', null]])('is unknown, not total minus zero, when a child size is %s', (_, bad) => {
    const body = html([root('100'), node({ node_id: 'a', size_bytes: '40' }), node({ node_id: 'b', size_bytes: bad })]);
    expect(rest(body)).toBe('Not in this slice unknown');
  });
  it('is unknown when a child was denied and carries 0, or has unmeasured coverage', () => {
    const denied = node({ node_id: 'a', size_bytes: '0', coverage: 'denied', threads: { ...THREADS, permission: { state: 'denied', reason: null } } });
    expect(rest(html([root('100'), denied]))).toBe('Not in this slice unknown');
    expect(rest(html([root('100'), node({ node_id: 'a', size_bytes: '0', coverage: 'unscanned' })]))).toBe('Not in this slice unknown');
  });
  it('is unknown when the total itself is malformed and children exist', () => {
    expect(rest(html([root('bogus'), node({ node_id: 'a', size_bytes: '40' })]))).toBe('Not in this slice unknown');
  });
});
