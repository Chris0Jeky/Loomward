import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import Inspector from '../../src/views/atlas/Inspector.svelte';
import { buildTree } from '../../viz/layout.js';
import { nodeInfo } from '../../viz/woven-treemap.js';
import type { Palette, SliceNodeLike } from '../../viz/types.js';

const src: SliceNodeLike = {
  node_id: 'root', parent: null, name: 'Synthetic root', kind: 'root', depth: 0,
  size_bytes: '0', logical_bytes: '0', allocated_bytes: '0', size_unknown_files: 0,
  files: 0, dirs: 0, child_count: 0, folded_count: null, coverage: 'complete', live: false,
  threads: { meaning: { state: 'none', label: null }, residency: { volume_id: null, tier: null, tier_basis: 'unknown' }, permission: { state: 'granted', reason: null } },
};

describe('Inspector sizes', () => {
  it.each([
    ['denied', { coverage: 'denied' }, 'unknown', 'unknown'],
    ['permission denied', { threads: { ...src.threads, permission: { state: 'denied', reason: null } } }, 'unknown', 'unknown'],
    ['unscanned', { coverage: 'unscanned' }, 'unknown', 'unknown'],
    ['unmeasured', { coverage: 'unknown' }, 'unknown', 'unknown'],
    ['allocation unknown', { size_unknown_files: 1 }, '0 B', 'unknown'],
    ['partial allocation', { logical_bytes: '10', allocated_bytes: '10', size_unknown_files: 1 }, '10 B', 'at least 10 B'],
    ['field-specific measurement', { size_bytes: '10', logical_bytes: '10', coverage: 'unknown' }, '10 B', 'unknown'],
  ])('%s never displays an unknown measurement as zero', (_name, extra, logical, allocated) => {
    const n = { ...src, ...extra } as SliceNodeLike;
    const node = nodeInfo(buildTree({ anchor_node_id: n.node_id, basis: 'logical', complete: true, truncated: false, nodes: [n] }).root);
    const { body } = render(Inspector, { props: { node, status: 'synthetic', basis: 'logical', provisional: false, palette: {} as Palette, path: [n.name] } });
    const fact = (label: string) => body.match(new RegExp(`<dt[^>]*>${label}</dt>\\s*<dd[^>]*>(.*?)</dd>`))![1]!.replace(/<[^>]*>/g, '').trim();
    expect(fact('Logical')).toBe(logical);
    expect(fact('Allocated')).toBe(allocated);
  });
});
