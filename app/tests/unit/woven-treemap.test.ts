import { afterEach, expect, it, vi } from 'vitest';
import type { NodeInfo, Palette, SliceLike } from '../../viz/types.js';
import { createWovenTreemap } from '../../viz/woven-treemap.js';

const width = 40;
const height = 20;
const node = (i: number, parent: number | null, size: number) => ({
  node_id: `n${i}`, parent, kind: parent === null ? 'atlas' : 'dir', name: `n${i}`, depth: parent === null ? 0 : 1,
  size_bytes: String(size), size_unknown_files: 0, logical_bytes: String(size), allocated_bytes: String(size),
  files: 0, dirs: 0, child_count: null, folded_count: null, coverage: 'complete', live: false,
  threads: { meaning: { state: 'none' as const, label: null }, residency: { volume_id: null, tier: null, tier_basis: 'unknown' }, permission: { state: 'granted' as const, reason: null } },
});

function showSlice(sizes: number[]) {
  // Keep the renderer's geometry and event handling, without scheduling a paint in Node.
  vi.stubGlobal('window', { requestIdleCallback: () => 1 });
  vi.stubGlobal('requestAnimationFrame', () => 1);
  vi.stubGlobal('cancelAnimationFrame', () => {});
  vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
  vi.stubGlobal('Path2D', class { rect() {} });

  const canvas = Object.assign(new EventTarget(), {
    getContext: () => ({ measureText: (text: string) => ({ width: text.length }) }),
    getBoundingClientRect: () => ({ left: 0, top: 0, width, height }),
  });
  const palette = { undyed: '#808080', unknown: '#404040', permission: '#202020' } as Palette;
  const treemap = createWovenTreemap(canvas as unknown as HTMLCanvasElement, { palette });
  const slice: SliceLike = {
    anchor_node_id: 'n0', basis: 'logical', complete: true, truncated: false,
    nodes: [node(0, null, sizes.reduce((total, size) => total + size, 0)),
      ...sizes.map((size, i) => node(i + 1, 0, size))],
  };
  treemap.setSlice(slice);
  return { canvas, treemap };
}

afterEach(() => vi.unstubAllGlobals());

it('maps the canvas to three proportional hit regions with the expected boundaries', () => {
  const { canvas, treemap } = showSlice([6, 3, 1]);
  let hovered: string | null = null;
  treemap.on('hover', ({ node }: { node: NodeInfo | null }) => { hovered = node?.id ?? null; });
  const hit = (clientX: number, clientY: number) => {
    canvas.dispatchEvent(Object.assign(new Event('pointermove'), { clientX, clientY }));
    return hovered;
  };

  expect(treemap.cellCount).toBe(3);
  expect([hit(23.5, 19.5), hit(24.5, 19.5)]).toEqual(['n1', 'n3']);
  expect([hit(39.5, 14.5), hit(39.5, 15.5)]).toEqual(['n2', 'n3']);

  const hits: Record<string, number> = {};
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const id = hit(x + 0.5, y + 0.5) ?? 'gap';
      hits[id] = (hits[id] ?? 0) + 1;
    }
  }
  expect(hits).toEqual({ n1: 24 * 20, n2: 16 * 15, n3: 16 * 5 });
  treemap.destroy();
});

it('folds sub-budget children before exposing visible nodes', () => {
  const { treemap } = showSlice(Array.from({ length: 80 }, () => 1));
  expect(treemap.cellCount).toBe(2);
  expect(treemap.visibleNodeIds).toEqual(['n1']);
  treemap.destroy();
});
