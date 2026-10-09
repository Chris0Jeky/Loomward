import { describe, expect, it } from 'vitest';
import { buildTree, layoutTreemap, partition, squarify } from '../../viz/layout.js';
import type { VNode as VNodeT } from '../../viz/layout.js';

type Item = { v: number; item: number };
const rnd = (seed: number) => () => ((seed = (seed * 1103515245 + 12345) >>> 0) / 2 ** 32);

const node = (i: number, parent: number | null, size: number, extra: Record<string, unknown> = {}) => ({
  node_id: `n${i}`, parent, kind: parent === null ? 'atlas' : 'dir', name: `n${i}`, depth: 0,
  size_bytes: String(size), size_unknown_files: 0, logical_bytes: String(size), allocated_bytes: String(size),
  files: 0, dirs: 0, child_count: null, folded_count: null, coverage: 'complete', live: false,
  threads: { meaning: { state: 'none' as const, label: null }, residency: { volume_id: null, tier: null, tier_basis: 'unknown' }, permission: { state: 'granted' as const, reason: null } },
  ...extra,
});
const slice = (nodes: ReturnType<typeof node>[]) => ({ anchor_node_id: 'n0', basis: 'logical' as const, complete: true, truncated: false, nodes });

describe('squarify', () => {
  for (const seed of [1, 2, 3, 4, 5]) {
    it(`tiles the rectangle exactly and keeps every area (seed ${seed})`, () => {
      const r = rnd(seed);
      const W = 400 + r() * 600, H = 200 + r() * 500;
      const raw = Array.from({ length: 5 + Math.floor(r() * 60) }, () => 1 + r() ** 3 * 1000).sort((a, b) => b - a);
      const total = raw.reduce((s, v) => s + v, 0);
      const items: Item[] = raw.map((v, i) => ({ v: (v * W * H) / total, item: i }));
      const out: [number, number, number, number, number][] = [];
      squarify(items, 0, 0, W, H, out);
      expect(out).toHaveLength(items.length);
      let area = 0;
      for (const [i, x, y, w, h] of out) {
        expect(w).toBeGreaterThan(0);
        expect(h).toBeGreaterThan(0);
        expect(x).toBeGreaterThanOrEqual(-1e-6);
        expect(y).toBeGreaterThanOrEqual(-1e-6);
        expect(x + w).toBeLessThanOrEqual(W + 1e-6);
        expect(y + h).toBeLessThanOrEqual(H + 1e-6);
        expect(w * h).toBeCloseTo(items[i]!.v, 0);
        area += w * h;
      }
      expect(area).toBeCloseTo(W * H, 0);
      // no two cells overlap
      for (let a = 0; a < out.length; a++) for (let b = a + 1; b < out.length; b++) {
        const [, ax, ay, aw, ah] = out[a]!;
        const [, bx, by, bw, bh] = out[b]!;
        const ox = Math.min(ax + aw, bx + bw) - Math.max(ax, bx);
        const oy = Math.min(ay + ah, by + bh) - Math.max(ay, by);
        expect(ox <= 1e-6 || oy <= 1e-6).toBe(true);
      }
    });
  }

  it('keeps aspect ratios reasonable for equal items', () => {
    const items: Item[] = Array.from({ length: 16 }, (_, i) => ({ v: 100 * 100 / 16, item: i }));
    const out: [number, number, number, number, number][] = [];
    squarify(items, 0, 0, 100, 100, out);
    for (const [, , , w, h] of out) expect(Math.max(w / h, h / w)).toBeLessThan(2.5);
  });
});

describe('fold into "smaller"', () => {
  it('bounds the cell count by area, not by sibling count', () => {
    const n = 50_000;
    const nodes = [node(0, null, 0), ...Array.from({ length: n }, (_, i) => node(i + 1, 0, 1000 + (i % 7)))];
    nodes[0]!.size_bytes = String(nodes.slice(1).reduce((s, x) => s + Number(x.size_bytes), 0));
    const { root } = buildTree(slice(nodes));
    const W = 900, H = 600, minArea = 16;
    const cells = layoutTreemap(root, W, H, { minArea });
    expect(cells.length).toBeLessThanOrEqual(Math.ceil((W * H) / minArea) + 1);
    const fold = cells.find((c) => c.node.synthetic === 'fold');
    expect(fold).toBeDefined();
    const drawn = cells.filter((c) => !c.node.synthetic).length;
    expect(fold!.node.folded + drawn).toBe(n);
    // the fold keeps its bytes: total area is still the whole canvas
    expect(cells.reduce((s, c) => s + c.w * c.h, 0)).toBeCloseTo(W * H, -1);
  });

  it('never folds a lone straggler into "1 smaller"', () => {
    const nodes = [node(0, null, 1_000_001), node(1, 0, 1_000_000), node(2, 0, 1)];
    const cells = layoutTreemap(buildTree(slice(nodes)).root, 300, 200, { minArea: 16 });
    expect(cells.some((c) => c.node.synthetic === 'fold')).toBe(false);
    expect(cells.filter((c) => c.depth === 0)).toHaveLength(2);
  });
});

describe('buildTree', () => {
  it('keeps zero-area children named, never as zero-size cells', () => {
    const nodes = [node(0, null, 100), node(1, 0, 100), node(2, 0, 0, { coverage: 'denied' })];
    const { root } = buildTree(slice(nodes));
    expect(root.children.map((c: VNodeT) => c.id)).toEqual(['n1']);
    expect(root.zero.map((z) => z.node_id)).toEqual(['n2']);
  });

  it('turns bytes the slice did not list into a remainder cell', () => {
    const nodes = [node(0, null, 1000), node(1, 0, 600)];
    const { root } = buildTree(slice(nodes));
    const rest = root.children.find((c: VNodeT) => c.synthetic === 'remainder');
    expect(rest?.size).toBe(400);
    expect(rest?.src).toBeNull();
  });

  it('rejects a child that names a later parent', () => {
    expect(() => buildTree(slice([node(0, null, 1), node(1, 2, 1), node(2, 0, 1)]))).toThrow();
  });
});

describe('partition', () => {
  it('gives each child a share of its parent proportional to size', () => {
    const nodes = [node(0, null, 10), node(1, 0, 6), node(2, 0, 4), node(3, 1, 3), node(4, 1, 3)];
    const { root } = buildTree(slice(nodes));
    const p = partition(root);
    const kids = root.children;
    expect(p.get(kids[0]!)).toEqual([0, 0.6]);
    expect(p.get(kids[1]!)![0]).toBeCloseTo(0.6);
    const [a0, a1] = p.get(kids[0]!.children[0]!)!;
    expect(a1 - a0).toBeCloseTo(0.3);
  });
});
