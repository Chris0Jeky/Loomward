/**
 * Woven treemap: a squarified treemap on Canvas 2D where every leaf is woven cloth.
 * Contract: docs/41 section 13. Consumes a bounded TreeSlice; layout is client-side.
 *
 * Warp = meaning, weft = residency, selvedge = permission (see colour.js). The canvas is one
 * continuous cloth: tiles are anchored to canvas space and every cell sharing a (warp, weft) pair
 * is filled with one batched Path2D, which keeps a full slice smooth.
 * Honest unknowns: zero-area children (access denied, unmeasured) are named in their parent's label
 * band; bytes the slice did not list become a loose-thread cell; unknown allocation is hatched under
 * the allocated basis; a provisional (live-scan) slice is woven loose and says so.
 *
 *   const tm = createWovenTreemap(canvas, { palette, reducedMotion, format })
 *   tm.setSlice(slice)           animated drill in or out when the new anchor relates to the old one
 *   tm.setThreads({ meaning, residency, permission })
 *   tm.setTheme(palette)  tm.setBasis(basis)  tm.focus(nodeId)  tm.resize()  tm.destroy()
 *   tm.on('select' | 'hover' | 'drill' | 'back', handler) -> unsubscribe
 *   tm.reveal()  tm.renderFrame()  tm.benchmark(frames)  tm.timeLayout()
 */
import { buildTree, layoutTreemap } from './layout.js';
import { selvedgeOf, warpOf, weftOf, weaveKey } from './colour.js';
import { weaveTile, fringeTile, looseTile, hatchTile, strokeStitch } from './weave.js';

/** @typedef {import('./types.js').Palette} Palette */
/** @typedef {import('./types.js').SliceLike} SliceLike */
/** @typedef {import('./types.js').NodeInfo} NodeInfo */
/** @typedef {import('./types.js').ThreadToggles} ThreadToggles */
/** @typedef {import('./types.js').VizEvent} VizEvent */
/** @typedef {import('./layout.js').VNode} VNode */
/** @typedef {import('./layout.js').Cell} Cell */
/** @typedef {import('./colour.js').Stitch} Stitch */

/**
 * @typedef {object} TreemapOptions
 * @property {Palette} palette
 * @property {() => boolean} [reducedMotion]
 * @property {(n: number) => string} [format]   bytes for labels
 * @property {number} [maxDepth]
 * @property {string} [marks]   performance-mark prefix: records `<prefix>:layout`, `<prefix>:first-paint`
 *   (CPU ms) and `<prefix>:slice-to-paint` (wall clock) for every slice (docs/41 P9)
 */

/**
 * @typedef {object} HoverEvent
 * @property {NodeInfo | null} node
 * @property {number} clientX
 * @property {number} clientY
 * @property {boolean} viaKeyboard
 */

const LOD = 9;
const ease = (/** @type {number} */ t) => (t >= 1 ? 1 : 1 - Math.pow(2, -10 * t));

/** @param {VNode} v @returns {NodeInfo} */
export function nodeInfo(v) {
  const src = v.src;
  const drillable = !v.synthetic && !!src && src.kind !== 'file' && src.kind !== 'other' && src.coverage !== 'denied'
    && (v.children.length > 0 || (src.child_count ?? 0) > 0);
  return { id: v.id, name: v.name, size: v.size, src, synthetic: v.synthetic, folded: v.folded, zero: v.zero, parentId: v.parent ? v.parent.id : null, drillable };
}

/** Zero-area children by reason, for the label band and the inspector. @param {{ zero: import('./types.js').SliceNodeLike[] }} v */
export function zeroCounts(v) {
  let denied = 0, unmeasured = 0, empty = 0;
  for (const z of v.zero) {
    if (z.coverage === 'denied' || z.threads.permission.state === 'denied') denied++;
    else if (z.coverage !== 'complete') unmeasured++;
    else empty++;
  }
  return { denied, unmeasured, empty };
}

/** @param {HTMLCanvasElement} canvas @param {TreemapOptions} options */
export function createWovenTreemap(canvas, options) {
  const ctxOrNull = canvas.getContext('2d');
  if (!ctxOrNull) throw new Error('2d canvas unavailable');
  const ctx = ctxOrNull;
  let palette = options.palette;
  const fmt = options.format ?? ((/** @type {number} */ n) => String(n));
  const reduced = () => options.reducedMotion?.() ?? false;
  const maxDepth = options.maxDepth ?? 3;
  /** @type {ThreadToggles} */
  let show = { meaning: true, residency: true, permission: true };
  /** @type {'logical' | 'allocated'} */
  let basis = 'logical';
  let dpr = 1, W = 0, H = 0;
  /** @type {SliceLike | null} */
  let slice = null;
  /** @type {ReturnType<typeof build> | null} */
  let view = null;
  /** @type {Map<string, CanvasPattern>} */
  let tiles = new Map();
  /** @type {{ outer: ReturnType<typeof build>, inner: ReturnType<typeof build>, rect: { x: number, y: number, w: number, h: number }, dir: number, start: number, dur: number } | null} */
  let anim = null;
  let revealT = 1, revealStart = 0, raf = 0;
  /** @type {{ start: number, layout: number, nodes: number } | null} */
  let pendingMark = null;
  /** @type {Cell | null} */
  let hover = null;
  let cursor = 0, kbd = false;
  /** @type {string | null} */
  let selected = null;
  /** @type {Map<VizEvent, Set<(e: any) => void>>} */
  const handlers = new Map();
  /** @param {VizEvent} name @param {unknown} payload */
  const emit = (name, payload) => handlers.get(name)?.forEach((h) => h(payload));

  /** @param {string} key @param {() => CanvasImageSource} make */
  function tile(key, make) {
    let t = tiles.get(key);
    if (!t) {
      const pat = ctx.createPattern(make(), 'repeat');
      if (!pat) throw new Error('pattern unavailable');
      pat.setTransform(new DOMMatrix().scale(1 / dpr));
      tiles.set(key, pat);
      t = pat;
    }
    return t;
  }

  /** @param {VNode} focus */
  function build(focus) {
    const t0 = performance.now();
    const cells = layoutTreemap(focus, W, H, { maxDepth });
    const provisional = slice?.aggregate_state === 'provisional_live';
    /** @type {Map<string, { warp: import('./colour.js').Warp, weft: import('./colour.js').Weft, loose: boolean, path: Path2D }>} */
    const woven = new Map();
    /** @type {Map<string, Path2D>} */
    const flat = new Map();
    /** @type {Map<string, Path2D>} */
    const under = new Map();
    /** @type {Map<Stitch, { colour: string, path: Path2D }>} */
    const stitches = new Map();
    const fold = new Path2D(), rest = new Path2D(), heads = new Path2D(), frames = new Path2D(), hatch = new Path2D();
    /** @type {{ cell: Cell, x: number, y: number, w: number, h: number, kind: 'head' | 'head0' | 'leaf' | 'display' | 'fold' | 'rest', name: string, size: string, note: string, text: string | null, sizeText: string | null }[]} */
    const labels = [];
    /** @type {Map<string, Cell>} */
    const byId = new Map();
    for (const c of cells) {
      byId.set(c.node.id, c);
      const g = c.depth === 0 ? 1.5 : 0.5;
      const x = c.x + g, y = c.y + g, w = c.w - 2 * g, h = c.h - 2 * g;
      if (w <= 0.3 || h <= 0.3) continue;
      const v = c.node, src = v.src;
      if (v.synthetic === 'fold') fold.rect(x, y, w, h);
      else if (v.synthetic === 'remainder') rest.rect(x, y, w, h);
      else if (!c.leaf) {
        frames.rect(x, y, w, h);
        if (c.header) heads.rect(x, y, w, c.header - 1);
      } else if (src) {
        const warp = warpOf(src.threads.meaning, palette, show.meaning);
        const weft = weftOf(src.threads, palette, show.residency);
        const loose = provisional || src.live;
        if (Math.min(w, h) >= LOD) {
          const k = weaveKey(warp, weft, loose);
          let e = woven.get(k);
          if (!e) { e = { warp, weft, loose, path: new Path2D() }; woven.set(k, e); }
          e.path.rect(x, y, w, h);
        } else {
          let f = flat.get(warp.colour);
          if (!f) { f = new Path2D(); flat.set(warp.colour, f); }
          f.rect(x, y, w, h);
          if (h >= 6 && weft.colour) {
            let u = under.get(weft.colour);
            if (!u) { u = new Path2D(); under.set(weft.colour, u); }
            u.rect(x, y + h - 1.5, w, 1.5);
          }
        }
        if (basis === 'allocated' && src.size_unknown_files > 0) hatch.rect(x, y, w, h);
      }
      // selvedge: only where a permission state begins
      if (src && w > 5 && h > 5) {
        const parent = v.parent?.src?.threads.permission ?? null;
        const s = selvedgeOf(src.threads.permission, c.depth === 0 && !v.parent?.src ? null : parent, palette, show.permission);
        if (s) {
          let e = stitches.get(s.stitch);
          if (!e) { e = { colour: s.colour, path: new Path2D() }; stitches.set(s.stitch, e); }
          e.path.rect(x + 1.5, y + 1.5, w - 3, h - 3);
        }
      }
      // labels
      const size = fmt(v.size);
      if (v.synthetic === 'fold') {
        if (w > 70 && h > 22) labels.push({ cell: c, x, y, w, h, kind: 'fold', name: `${v.folded.toLocaleString('en-GB')} smaller`, size, note: '', text: null, sizeText: null });
      } else if (v.synthetic === 'remainder') {
        if (w > 70 && h > 26) labels.push({ cell: c, x, y, w, h, kind: 'rest', name: v.name, size, note: '', text: null, sizeText: null });
      } else if (!c.leaf && c.header) {
        const z = zeroCounts(v);
        const parts = [];
        if (z.denied) parts.push(`${z.denied} access denied`);
        if (z.unmeasured) parts.push(`${z.unmeasured} unmeasured`);
        if (basis === 'allocated' && src && src.size_unknown_files > 0) parts.push(`${src.size_unknown_files} alloc unknown`);
        labels.push({ cell: c, x, y, w, h, kind: c.depth === 0 ? 'head0' : 'head', name: v.name, size, note: parts.join(' · '), text: null, sizeText: null });
      } else if (c.leaf && w > 54 && h > 26) {
        labels.push({ cell: c, x, y, w, h, kind: c.depth === 0 && w > 190 && h > 110 ? 'display' : 'leaf', name: v.name, size, note: '', text: null, sizeText: null });
      }
    }
    /** @param {string} k */
    const fontOf = (k) => (k === 'display' ? palette.fontDisplay : k === 'head0' ? palette.fontHead : palette.fontLabel);
    for (const l of labels) {
      const isHead = l.kind === 'head' || l.kind === 'head0';
      const room = isHead ? l.w - 14 : l.w - 18;
      const sizeFull = l.note ? `${l.size} · ${l.note}` : l.size;
      ctx.font = palette.fontNum;
      const sizeW = ctx.measureText(l.size).width, fullW = ctx.measureText(sizeFull).width;
      ctx.font = fontOf(l.kind);
      if (isHead) {
        const useFull = room - fullW > 60;
        l.sizeText = useFull ? sizeFull : room - sizeW > 30 ? l.size : null;
        const sw = l.sizeText === sizeFull ? fullW : l.sizeText ? sizeW : 0;
        l.text = fit(ctx, l.name, room - sw - 10);
      } else {
        l.text = fit(ctx, l.name, room);
        l.sizeText = l.h > (l.kind === 'display' ? 60 : 36) && sizeW < room ? l.size : null;
      }
    }
    return { focus, cells, byId, woven, flat, under, stitches, fold, rest, heads, frames, hatch, labels, provisional, layoutMs: performance.now() - t0 };
  }

  /** @param {CanvasRenderingContext2D} g @param {string} text @param {number} max */
  function fit(g, text, max) {
    if (max <= 8) return null;
    if (g.measureText(text).width <= max) return text;
    let lo = 0, hi = text.length;
    while (lo < hi) { const m = (lo + hi + 1) >> 1; if (g.measureText(text.slice(0, m) + '…').width <= max) lo = m; else hi = m - 1; }
    return lo >= 2 ? text.slice(0, lo) + '…' : null;
  }

  /** @typedef {{ sx: number, sy: number, tx: number, ty: number }} Affine */
  /** @type {Affine} */
  const identity = { sx: 1, sy: 1, tx: 0, ty: 0 };
  /** @param {{x:number,y:number,w:number,h:number}} a @param {{x:number,y:number,w:number,h:number}} b @returns {Affine} */
  const mapRect = (a, b) => { const sx = b.w / a.w, sy = b.h / a.h; return { sx, sy, tx: b.x - a.x * sx, ty: b.y - a.y * sy }; };

  /** @param {Affine} m @param {number} alpha @param {{x:number,y:number,w:number,h:number} | null} clip */
  function begin(m, alpha, clip) {
    ctx.save();
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    if (clip) { ctx.beginPath(); ctx.rect(clip.x, clip.y, clip.w, clip.h); ctx.clip(); }
    ctx.globalAlpha = alpha;
    ctx.transform(m.sx, 0, 0, m.sy, m.tx, m.ty);
  }

  /** @param {ReturnType<typeof build>} v @param {Affine} m @param {number} alpha @param {{x:number,y:number,w:number,h:number} | null} clip @param {number} reveal */
  function paint(v, m, alpha, clip, reveal) {
    begin(m, alpha, clip);
    ctx.fillStyle = palette.frame; ctx.fill(v.frames);
    ctx.fillStyle = palette.head; ctx.fill(v.heads);
    const warpPhase = Math.min(1, reveal / 0.55), weftPhase = Math.max(0, (reveal - 0.42) / 0.58);
    for (const e of v.woven.values()) {
      const full = () => tile(`w|${weaveKey(e.warp, e.weft, e.loose)}`, () => weaveTile(dpr, e.warp, e.weft, { ink: palette.ink, unknown: palette.unknown, loose: e.loose }));
      if (reveal < 1) {
        const warpOnly = tile(`wo|${e.warp.pattern}|${e.warp.colour}|${e.warp.alt}`, () => weaveTile(dpr, e.warp, e.weft, { ink: palette.ink, unknown: palette.unknown, only: 'warp' }));
        ctx.save(); ctx.beginPath(); ctx.rect(0, 0, W, H * ease(warpPhase)); ctx.clip(); ctx.fillStyle = warpOnly; ctx.fill(e.path); ctx.restore();
        if (weftPhase > 0) { ctx.save(); ctx.beginPath(); ctx.rect(0, 0, W * weftPhase, H); ctx.clip(); ctx.fillStyle = full(); ctx.fill(e.path); ctx.restore(); }
      } else { ctx.fillStyle = full(); ctx.fill(e.path); }
    }
    for (const [colour, path] of v.flat) { ctx.fillStyle = colour; ctx.globalAlpha = alpha * 0.85; ctx.fill(path); }
    ctx.globalAlpha = alpha;
    for (const [colour, path] of v.under) { ctx.fillStyle = colour; ctx.fill(path); }
    ctx.fillStyle = tile('fringe', () => fringeTile(dpr, palette.fringe, palette.ink)); ctx.fill(v.fold);
    ctx.fillStyle = tile('loose', () => looseTile(dpr, palette.loose, palette.ink)); ctx.fill(v.rest);
    ctx.fillStyle = tile('hatch', () => hatchTile(dpr, palette.unknown)); ctx.fill(v.hatch);
    const late = reveal >= 1 ? 1 : Math.max(0, (reveal - 0.8) / 0.2);
    ctx.globalAlpha = alpha * late;
    const lw = 1 / Math.sqrt(m.sx * m.sy);
    for (const [stitch, e] of v.stitches) strokeStitch(ctx, e.path, stitch, e.colour, palette.ink, lw);
    drawLabels(v);
    ctx.restore();
  }

  /** @param {ReturnType<typeof build>} v */
  function drawLabels(v) {
    ctx.textBaseline = 'alphabetic';
    for (const l of v.labels) {
      if (!l.text) continue;
      const { x, y } = l;
      if (l.kind === 'head' || l.kind === 'head0') {
        const by = l.kind === 'head0' ? y + 17 : y + 12;
        ctx.font = l.kind === 'head0' ? palette.fontHead : palette.fontLabel; ctx.fillStyle = palette.label;
        ctx.fillText(l.text, x + 7, by);
        if (l.sizeText) { ctx.font = palette.fontNum; ctx.fillStyle = palette.labelDim; const tw = ctx.measureText(l.sizeText).width; ctx.fillText(l.sizeText, x + l.w - tw - 7, by); }
        continue;
      }
      const display = l.kind === 'display';
      ctx.font = display ? palette.fontDisplay : palette.fontLabel;
      const tw = ctx.measureText(l.text).width;
      ctx.font = palette.fontNum;
      const sw = l.sizeText ? ctx.measureText(l.sizeText).width : 0;
      const pw = Math.max(tw, sw) + 12, ph = (display ? 26 : 17) + (l.sizeText ? (display ? 17 : 13) : 0);
      ctx.fillStyle = palette.scrim;
      ctx.beginPath(); ctx.roundRect(x + 4, y + 4, Math.min(pw, l.w - 8), Math.min(ph, l.h - 8), 2); ctx.fill();
      ctx.font = display ? palette.fontDisplay : palette.fontLabel;
      ctx.fillStyle = l.kind === 'fold' || l.kind === 'rest' ? palette.labelDim : palette.label;
      ctx.fillText(l.text, x + 10, y + (display ? 25 : 17));
      if (l.sizeText) { ctx.font = palette.fontNum; ctx.fillStyle = palette.labelDim; ctx.fillText(l.sizeText, x + 10, y + (display ? 43 : 30)); }
    }
  }

  function overlays() {
    if (!view) return;
    ctx.save();
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    // gallery light: the cloth falls off toward the corners
    const gr = ctx.createRadialGradient(W * 0.42, H * 0.38, Math.min(W, H) * 0.2, W * 0.5, H * 0.5, Math.hypot(W, H) * 0.62);
    gr.addColorStop(0, 'rgba(0,0,0,0)'); gr.addColorStop(1, 'rgba(0,0,0,.36)');
    ctx.fillStyle = gr; ctx.fillRect(0, 0, W, H);
    if (view.provisional) {
      const text = 'Provisional: sums from a running scan';
      ctx.font = palette.fontSmall;
      const tw = ctx.measureText(text).width;
      ctx.fillStyle = palette.scrim; ctx.fillRect(W - tw - 22, H - 24, tw + 16, 18);
      ctx.fillStyle = palette.permission; ctx.fillText(text, W - tw - 14, H - 11);
    }
    if (revealT >= 1) {
      const top = view.cells.filter((c) => c.depth === 0);
      const cur = top[cursor];
      if (cur && kbd) {
        ctx.strokeStyle = palette.cursor; ctx.lineWidth = 2; ctx.shadowColor = palette.cursor; ctx.shadowBlur = 12;
        ctx.strokeRect(cur.x + 1, cur.y + 1, cur.w - 2, cur.h - 2);
        ctx.shadowBlur = 0;
      }
      if (hover) { ctx.strokeStyle = palette.hover; ctx.lineWidth = 1.5; ctx.strokeRect(hover.x + 0.75, hover.y + 0.75, hover.w - 1.5, hover.h - 1.5); }
      const sel = selected ? view.byId.get(selected) : null;
      if (sel) { ctx.strokeStyle = palette.cursor; ctx.lineWidth = 1.5; ctx.setLineDash([4, 3]); ctx.strokeRect(sel.x + 0.75, sel.y + 0.75, sel.w - 1.5, sel.h - 1.5); }
    } else {
      const weftPhase = (revealT - 0.42) / 0.58;
      if (weftPhase > 0 && weftPhase < 1) {
        const x = W * weftPhase;
        const g = ctx.createLinearGradient(x - 40, 0, x + 2, 0);
        g.addColorStop(0, 'rgba(0,0,0,0)'); g.addColorStop(1, palette.shuttle);
        ctx.fillStyle = g; ctx.fillRect(x - 40, 0, 42, H);
      }
    }
    ctx.restore();
  }

  let paintStart = 0;
  /** @param {number} now */
  function frame(now) {
    raf = 0;
    if (!view) return;
    paintStart = performance.now();
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.fillStyle = palette.ink;
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    let more = false;
    if (anim) {
      const t = Math.min(1, (now - anim.start) / anim.dur);
      const e = anim.dir > 0 ? ease(t) : 1 - ease(t);
      const R = anim.rect, full = { x: 0, y: 0, w: W, h: H };
      const Rt = { x: R.x * (1 - e), y: R.y * (1 - e), w: R.w + (W - R.w) * e, h: R.h + (H - R.h) * e };
      paint(anim.outer, mapRect(R, Rt), 1, null, 1);
      const a = Math.min(1, Math.max(0, (e - 0.25) / 0.6));
      ctx.save(); ctx.setTransform(dpr, 0, 0, dpr, 0, 0); ctx.fillStyle = palette.ink; ctx.globalAlpha = a; ctx.fillRect(Rt.x, Rt.y, Rt.w, Rt.h); ctx.restore();
      paint(anim.inner, mapRect(full, Rt), a, Rt, 1);
      if (t < 1) more = true; else anim = null;
    } else {
      if (revealT < 1) { revealT = Math.min(1, (now - revealStart) / 1700); more = revealT < 1; }
      paint(view, identity, 1, null, revealT);
      overlays();
    }
    if (pendingMark && options.marks) {
      const end = performance.now(), m = pendingMark, pre = options.marks;
      pendingMark = null;
      const detail = { nodes: m.nodes, cells: view.cells.length };
      performance.measure(`${pre}:layout`, { start: m.start, duration: m.layout, detail });
      performance.measure(`${pre}:first-paint`, { start: paintStart, end, detail });
      performance.measure(`${pre}:slice-to-paint`, { start: m.start, end, detail });
    }
    if (more) raf = requestAnimationFrame(frame);
  }
  const schedule = () => { if (!raf) raf = requestAnimationFrame(frame); };

  // Build every tile for the palette while idle, so a zoom into new cloth never stalls a frame.
  function prewarm() {
    const run = () => {
      const dyes = [...palette.dyes, palette.undyed];
      for (const colour of dyes) for (const tier of [0, 1, 2, null]) {
        const warp = { colour, alt: null, pattern: /** @type {const} */ ('solid') };
        const weft = tier === null ? { colour: null, pattern: /** @type {const} */ ('missing'), tier: null } : { colour: palette.metals[tier] ?? palette.unknown, pattern: /** @type {const} */ ('metal'), tier };
        tile(`w|${weaveKey(warp, weft, false)}`, () => weaveTile(dpr, warp, weft, { ink: palette.ink, unknown: palette.unknown }));
      }
    };
    const ric = /** @type {any} */ (window).requestIdleCallback;
    if (typeof ric === 'function') ric(run); else setTimeout(run, 200);
  }

  function resize() {
    const r = canvas.getBoundingClientRect();
    const ndpr = Math.min(window.devicePixelRatio || 1, 3);
    const nw = Math.max(1, Math.round(r.width)), nh = Math.max(1, Math.round(r.height));
    if (nw === W && nh === H && ndpr === dpr && view) return;
    const dprChanged = ndpr !== dpr;
    dpr = ndpr; W = nw; H = nh;
    canvas.width = Math.round(W * dpr); canvas.height = Math.round(H * dpr);
    if (dprChanged) { tiles = new Map(); prewarm(); }
    if (view) { view = build(view.focus); anim = null; schedule(); }
  }
  const ro = new ResizeObserver(() => resize());
  ro.observe(canvas);

  /** @param {number} px @param {number} py */
  function hitTest(px, py) {
    if (!view) return null;
    for (let i = view.cells.length - 1; i >= 0; i--) {
      const c = /** @type {Cell} */ (view.cells[i]);
      if (px >= c.x && px < c.x + c.w && py >= c.y && py < c.y + c.h) return c;
    }
    return null;
  }
  /** @param {Cell} c */
  function topOf(c) {
    if (!view) return c;
    let n = c.node;
    while (n.parent && n.parent !== view.focus) n = n.parent;
    return view.byId.get(n.id) ?? c;
  }
  const topCells = () => (view ? view.cells.filter((c) => c.depth === 0) : []);

  /** @param {Cell | null} c @param {MouseEvent | null} ev @param {boolean} viaKeyboard */
  function sendHover(c, ev, viaKeyboard) {
    /** @type {HoverEvent} */
    const e = { node: c ? nodeInfo(c.node) : null, clientX: ev ? ev.clientX : 0, clientY: ev ? ev.clientY : 0, viaKeyboard };
    emit('hover', e);
  }

  /** @param {MouseEvent} e */
  const onMove = (e) => {
    if (anim || revealT < 1) return;
    const r = canvas.getBoundingClientRect();
    const c = hitTest(e.clientX - r.left, e.clientY - r.top);
    if (c !== hover) { hover = c; kbd = false; schedule(); }
    sendHover(c, e, false);
  };
  const onLeave = () => { hover = null; sendHover(null, null, false); schedule(); };
  /** @param {MouseEvent} e */
  const onClick = (e) => {
    if (anim || !view) return;
    const r = canvas.getBoundingClientRect();
    const c = hitTest(e.clientX - r.left, e.clientY - r.top);
    if (!c) return;
    selected = c.node.synthetic ? null : c.node.id;
    emit('select', nodeInfo(c.node));
    const t = topOf(c);
    const info = nodeInfo(t.node);
    if (info.drillable) emit('drill', info);
    schedule();
  };
  /** @param {KeyboardEvent} e */
  const onKey = (e) => {
    const top = topCells();
    if (!top.length) return;
    const cur = /** @type {Cell} */ (top[Math.min(cursor, top.length - 1)]);
    /** @type {Record<string, [number, number]>} */
    const dirs = { ArrowRight: [1, 0], ArrowLeft: [-1, 0], ArrowDown: [0, 1], ArrowUp: [0, -1] };
    const d = dirs[e.key];
    if (d) {
      e.preventDefault();
      const [dx, dy] = d;
      const cx = cur.x + cur.w / 2, cy = cur.y + cur.h / 2;
      let best = -1, bestScore = Infinity;
      top.forEach((c, i) => {
        if (c === cur) return;
        const vx = c.x + c.w / 2 - cx, vy = c.y + c.h / 2 - cy;
        const along = vx * dx + vy * dy;
        if (along <= 0) return;
        const s = along + (Math.abs(vx * dy) + Math.abs(vy * dx)) * 2.2;
        if (s < bestScore) { bestScore = s; best = i; }
      });
      if (best >= 0) cursor = best;
    } else if (e.key === 'Home') { cursor = 0; e.preventDefault(); }
    else if (e.key === 'End') { cursor = top.length - 1; e.preventDefault(); }
    else if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      const info = nodeInfo(cur.node);
      selected = cur.node.synthetic ? null : cur.node.id;
      emit('select', info);
      if (info.drillable) emit('drill', info);
      kbd = true; schedule(); return;
    } else if (e.key === 'Escape' || e.key === 'Backspace') {
      e.preventDefault(); kbd = true; emit('back', null); return;
    } else return;
    kbd = true;
    sendHover(/** @type {Cell} */ (top[cursor]), null, true);
    schedule();
  };
  const onFocus = () => { kbd = true; schedule(); };
  const onBlur = () => { kbd = false; schedule(); };
  canvas.addEventListener('pointermove', onMove);
  canvas.addEventListener('pointerleave', onLeave);
  canvas.addEventListener('click', onClick);
  canvas.addEventListener('keydown', onKey);
  canvas.addEventListener('focus', onFocus);
  canvas.addEventListener('blur', onBlur);

  resize();
  prewarm();

  return {
    /**
     * Show a slice. When its anchor is inside the current one (drill) or the current anchor is
     * inside it (back), the change is a zoom; otherwise it is a cut.
     * @param {SliceLike} s @param {{ animate?: boolean }} [opts]
     */
    setSlice(s, opts = {}) {
      const start = performance.now();
      const prev = view;
      slice = s;
      const { root } = buildTree(s);
      const next = build(root);
      pendingMark = { start, layout: performance.now() - start, nodes: s.nodes.length };
      cursor = 0; hover = null;
      anim = null;
      if (prev && opts.animate !== false && !reduced()) {
        const inward = next.focus.id !== prev.focus.id && prev.byId.has(next.focus.id);
        const outward = next.focus.id !== prev.focus.id && next.byId.has(prev.focus.id);
        const outer = inward ? prev : next, inner = inward ? next : prev;
        const cell = inward || outward ? outer.byId.get(inner.focus.id) : undefined;
        if (cell) anim = { outer, inner, rect: { x: cell.x, y: cell.y, w: cell.w, h: cell.h }, dir: inward ? 1 : -1, start: performance.now(), dur: 620 };
      }
      view = next;
      if (selected && !next.byId.has(selected)) selected = null;
      schedule();
    },
    /** @param {Partial<ThreadToggles>} t */
    setThreads(t) { show = { ...show, ...t }; if (view) { view = build(view.focus); schedule(); } },
    /** @param {Palette} p */
    setTheme(p) { palette = p; tiles = new Map(); prewarm(); if (view) { view = build(view.focus); schedule(); } },
    /** @param {'logical' | 'allocated'} b  the next slice carries the sizes; this sets the hatching */
    setBasis(b) { basis = b; if (view) { view = build(view.focus); schedule(); } },
    /** Move the keyboard cursor to a top-level node (or the one containing it). @param {string} id */
    focus(id) {
      if (!view) return;
      const c = view.byId.get(id);
      if (!c) return;
      const t = topOf(c);
      const i = topCells().indexOf(t);
      if (i >= 0) { cursor = i; kbd = true; selected = id; sendHover(t, null, true); schedule(); }
    },
    reveal() { if (reduced()) { revealT = 1; schedule(); return; } revealT = 0; revealStart = performance.now(); schedule(); },
    resize,
    /**
     * @param {VizEvent} name @param {(e: any) => void} handler @returns {() => void}
     */
    on(name, handler) {
      let set = handlers.get(name);
      if (!set) { set = new Set(); handlers.set(name, set); }
      set.add(handler);
      return () => { set.delete(handler); };
    },
    /** Paint the settled view synchronously (for timing inside a rAF loop). */
    renderFrame() { revealT = 1; anim = null; frame(performance.now()); },
    /** Layout time for the current slice, ms. */
    timeLayout() { if (!view) return 0; const v = build(view.focus); view = v; return v.layoutMs; },
    get cellCount() { return view ? view.cells.length : 0; },
    destroy() { ro.disconnect(); cancelAnimationFrame(raf); canvas.removeEventListener('pointermove', onMove); canvas.removeEventListener('pointerleave', onLeave); canvas.removeEventListener('click', onClick); canvas.removeEventListener('keydown', onKey); canvas.removeEventListener('focus', onFocus); canvas.removeEventListener('blur', onBlur); handlers.clear(); },
  };
}

/**
 * A small swatch of one channel for legends and the inspector, from the same tile renderer.
 * @param {HTMLCanvasElement} canvas @param {Palette} palette
 * @param {{ threads?: import('./types.js').ThreadsLike, only?: 'warp' | 'weft' | 'selvedge', stitch?: Stitch }} o
 */
export function drawSwatch(canvas, palette, o) {
  const dpr = Math.min(window.devicePixelRatio || 1, 3);
  const r = canvas.getBoundingClientRect();
  const w = Math.max(1, Math.round(r.width || canvas.width)), h = Math.max(1, Math.round(r.height || canvas.height));
  canvas.width = w * dpr; canvas.height = h * dpr;
  const g = canvas.getContext('2d');
  if (!g) return;
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.fillStyle = palette.ink; g.fillRect(0, 0, w, h);
  if (o.only !== 'selvedge' && o.threads) {
    const warp = warpOf(o.threads.meaning, palette), weft = weftOf(o.threads, palette);
    const pat = g.createPattern(weaveTile(dpr, warp, weft, { ink: palette.ink, unknown: palette.unknown, only: o.only === 'warp' ? 'warp' : o.only === 'weft' ? 'weft' : undefined }), 'repeat');
    if (pat) { pat.setTransform(new DOMMatrix().scale(1 / dpr)); g.fillStyle = pat; g.fillRect(0, 0, w, h); }
  }
  const stitch = o.stitch ?? (o.threads ? selvedgeOf(o.threads.permission, null, palette)?.stitch : undefined);
  if (stitch) {
    const p = new Path2D(); p.rect(1.5, 1.5, w - 3, h - 3);
    strokeStitch(g, p, stitch, stitch === 'dotted' ? palette.unknown : palette.permission, palette.ink);
  }
}
