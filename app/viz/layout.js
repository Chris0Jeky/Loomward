/**
 * Pure layout: TreeSlice -> node tree -> squarified treemap cells, and the sunburst partition.
 * No DOM. Sizes are the slice's exact decimal strings converted to numbers for geometry only
 * (ADR-V3-04: lossy for layout, never for display).
 */

/** @typedef {import('./types.js').SliceLike} SliceLike */
/** @typedef {import('./types.js').SliceNodeLike} SliceNodeLike */

/**
 * A node of the render tree. `synthetic` cells stand for bytes without a listed node:
 * 'remainder' = the part of a parent not listed in this slice; 'fold' = siblings too small to draw.
 * @typedef {object} VNode
 * @property {string} id
 * @property {string} name
 * @property {number} size
 * @property {SliceNodeLike | null} src
 * @property {VNode | null} parent
 * @property {VNode[]} children
 * @property {SliceNodeLike[]} zero    children with no area (0 bytes, denied, unmeasured): listed, never drawn as zero-size cells
 * @property {'remainder' | 'fold' | null} synthetic
 * @property {number} folded
 */

/** @param {string | null | undefined} s */
export const toNumber = (s) => (typeof s === 'string' && /^\d{1,20}$/.test(s) ? Number(s) : 0);

/** A remainder smaller than this share of its parent is not worth a cell (rounding noise). */
export const REMAINDER_MIN_SHARE = 0.002;

/**
 * Build the render tree from a slice. Children with zero size get no area and are kept on their
 * parent's `zero` list; when listed children do not add up to the parent, the rest becomes a
 * 'remainder' cell, so the treemap never pretends a slice is the whole subtree.
 * @param {SliceLike} slice @returns {{ root: VNode, byId: Map<string, VNode> }}
 */
export function buildTree(slice) {
  /** @type {VNode[]} */
  const at = [];
  /** @type {Map<string, VNode>} */
  const byId = new Map();
  slice.nodes.forEach((n, i) => {
    /** @type {VNode} */
    const v = { id: n.node_id, name: n.name, size: toNumber(n.size_bytes), src: n, parent: null, children: [], zero: [], synthetic: null, folded: n.kind === 'other' ? (n.folded_count ?? 0) : 0 };
    at[i] = v;
    byId.set(v.id, v);
    if (n.parent === null) return;
    const p = at[n.parent];
    if (!p) throw new Error(`slice node ${i} names parent ${n.parent}, which does not precede it`);
    v.parent = p;
    if (v.size > 0) p.children.push(v);
    else p.zero.push(n);
  });
  const root = at[0];
  if (!root) throw new Error('empty slice');
  for (const v of at) {
    if (!v.children.length) continue;
    const listed = v.children.reduce((s, c) => s + c.size, 0);
    const rest = v.size - listed;
    if (rest > v.size * REMAINDER_MIN_SHARE) {
      v.children.push({ id: `${v.id}#rest`, name: 'Not in this slice', size: rest, src: null, parent: v, children: [], zero: [], synthetic: 'remainder', folded: 0 });
    }
    v.children.sort((a, b) => b.size - a.size || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
  }
  return { root, byId };
}

/**
 * @typedef {object} Cell
 * @property {number} x @property {number} y @property {number} w @property {number} h
 * @property {VNode} node
 * @property {number} depth   0 = a child of the focus
 * @property {boolean} leaf
 * @property {number} header  height of the container label band, 0 when none
 */

/**
 * Squarified layout (Bruls, Huizing, van Wijk). `items` must be sorted by value, largest first,
 * and their values must sum to w * h. Appends [item, x, y, w, h] to `out`.
 * @template T
 * @param {{ v: number, item: T }[]} items @param {number} x @param {number} y @param {number} w @param {number} h
 * @param {[T, number, number, number, number][]} out
 */
export function squarify(items, x, y, w, h, out) {
  let i = 0;
  while (i < items.length) {
    const short = Math.min(w, h);
    if (short <= 0) break;
    const first = /** @type {{ v: number, item: T }} */ (items[i]);
    let sum = first.v, j = i + 1;
    let worst = Math.max((short * short * first.v) / (sum * sum), (sum * sum) / (short * short * first.v));
    while (j < items.length) {
      const next = /** @type {{ v: number, item: T }} */ (items[j]);
      const s2 = sum + next.v;
      const w2 = Math.max((short * short * first.v) / (s2 * s2), (s2 * s2) / (short * short * next.v));
      if (w2 > worst) break;
      sum = s2; worst = w2; j++;
    }
    if (j === items.length) sum = w * h; // the last row takes whatever is left: float error never leaves a gap
    if (w >= h) {
      const cw = Math.min(w, sum / h); let cy = y;
      for (let k = i; k < j; k++) { const it = /** @type {{ v: number, item: T }} */ (items[k]); const ch = k === j - 1 ? y + h - cy : it.v / cw; out.push([it.item, x, cy, cw, ch]); cy += ch; }
      x += cw; w -= cw;
    } else {
      const rh = Math.min(h, sum / w); let cx = x;
      for (let k = i; k < j; k++) { const it = /** @type {{ v: number, item: T }} */ (items[k]); const cw = k === j - 1 ? x + w - cx : it.v / rh; out.push([it.item, cx, y, cw, rh]); cx += cw; }
      y += rh; h -= rh;
    }
    i = j;
  }
}

/**
 * @typedef {object} TreemapOptions
 * @property {number} [maxDepth]   nesting levels below the focus (default 3)
 * @property {number} [minArea]    px²: siblings smaller than this fold into one "smaller" cell (default 16)
 * @property {number} [pad]        inset of nested content (default 2)
 * @property {(depth: number) => number} [header]  label band height for a container at depth (default 24 / 17)
 */

/**
 * Lay out the subtree under `focus` into a w x h rectangle. The number of cells is bounded by
 * area (at most one cell per `minArea` px² plus one fold per container), not by tree size.
 * @param {VNode} focus @param {number} W @param {number} H @param {TreemapOptions} [opts] @returns {Cell[]}
 */
export function layoutTreemap(focus, W, H, opts = {}) {
  const maxDepth = opts.maxDepth ?? 3;
  const minArea = opts.minArea ?? 16;
  const pad = opts.pad ?? 2;
  const header = opts.header ?? ((d) => (d === 0 ? 24 : 17));
  /** @type {Cell[]} */
  const cells = [];
  /** @param {VNode} node @param {number} x @param {number} y @param {number} w @param {number} h @param {number} depth */
  function place(node, x, y, w, h, depth) {
    const kids = node.children;
    if (!kids.length || w < 2 || h < 2) return;
    const total = kids.reduce((s, c) => s + c.size, 0);
    if (total <= 0) return;
    const scale = (w * h) / total;
    /** @type {{ v: number, item: VNode }[]} */
    const items = [];
    let rest = 0, restN = 0;
    for (const c of kids) {
      const v = c.size * scale;
      if (v >= minArea || items.length === 0) items.push({ v, item: c });
      else { rest += v; restN += c.folded > 0 ? c.folded : 1; }
    }
    if (restN === 1) {
      // a single straggler is shown as itself, not as "1 smaller item"
      const c = /** @type {VNode} */ (kids[items.length]);
      items.push({ v: c.size * scale, item: c }); rest = 0; restN = 0;
    }
    if (restN) items.push({ v: rest, item: { id: `${node.id}#fold`, name: `${restN} smaller`, size: rest / scale, src: null, parent: node, children: [], zero: [], synthetic: 'fold', folded: restN } });
    items.sort((a, b) => b.v - a.v);
    /** @type {[VNode, number, number, number, number][]} */
    const out = [];
    squarify(items, x, y, w, h, out);
    for (const [n, cx, cy, cw, ch] of out) {
      /** @type {Cell} */
      const cell = { x: cx, y: cy, w: cw, h: ch, node: n, depth, leaf: true, header: 0 };
      cells.push(cell);
      if (n.children.length && depth < maxDepth && cw > 26 && ch > 26) {
        cell.leaf = false;
        const hb = cw > 64 && ch > 44 ? header(depth) : 0;
        cell.header = hb;
        place(n, cx + pad, cy + (hb || pad), cw - 2 * pad, ch - (hb || pad) - pad, depth + 1);
      }
    }
  }
  place(focus, 0, 0, W, H, 0);
  return cells;
}

/**
 * Sunburst partition: each node's [x0, x1) share of the root's turn, children in size order.
 * @param {VNode} root @returns {Map<VNode, [number, number]>}
 */
export function partition(root) {
  /** @type {Map<VNode, [number, number]>} */
  const part = new Map();
  /** @param {VNode} n @param {number} x0 @param {number} x1 */
  const walk = (n, x0, x1) => {
    part.set(n, [x0, x1]);
    const tot = n.children.reduce((s, c) => s + c.size, 0);
    if (!tot) return;
    let x = x0;
    for (const c of n.children) { const w = ((x1 - x0) * c.size) / tot; walk(c, x, x + w); x += w; }
  };
  walk(root, 0, 1);
  return part;
}

/** Depth of `n` below `top` (0 when equal, -1 when not a descendant). @param {VNode} n @param {VNode} top */
export function depthBelow(n, top) {
  let d = 0;
  for (let p = /** @type {VNode | null} */ (n); p; p = p.parent, d++) if (p === top) return d;
  return -1;
}
