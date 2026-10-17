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
  it('offers copy preview only for a selected source, without requiring browser globals to render', () => {
    const node = nodeInfo(buildTree({ anchor_node_id: src.node_id, basis: 'logical', complete: true, truncated: false, nodes: [src] }).root);
    const show = (status: string) => render(Inspector, { props: { node, status, basis: 'logical', provisional: false, palette: {} as Palette, path: [src.name] } }).body;
    expect(show('Selected')).toContain('Copy to');
    expect(show('Hover')).not.toContain('Copy to');
  });
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

  // Codex P2 on #355: the catalogue nulls allocated_bytes when some allocation is unknown, but an
  // allocated-basis slice still carries the known subtotal in size_bytes.
  it.each([
    ['allocated', 'at least 10 B'],
    ['logical', 'unknown'],
  ] as const)('a %s-basis slice with partial allocation shows %s', (basis, allocated) => {
    const n = { ...src, size_bytes: '10', logical_bytes: '12', allocated_bytes: null, size_unknown_files: 1 } as unknown as SliceNodeLike;
    const node = nodeInfo(buildTree({ anchor_node_id: n.node_id, basis, complete: true, truncated: false, nodes: [n] }).root);
    const { body } = render(Inspector, { props: { node, status: 'synthetic', basis, provisional: false, palette: {} as Palette, path: [n.name] } });
    const fact = (label: string) => body.match(new RegExp(`<dt[^>]*>${label}</dt>\\s*<dd[^>]*>(.*?)</dd>`))![1]!.replace(/<[^>]*>/g, '').trim();
    expect(fact('Allocated')).toBe(allocated);
  });
});
