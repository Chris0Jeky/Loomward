/**
 * Woven treemap: a squarified treemap on Canvas 2D where every leaf is woven cloth.
 *
 * Three channels, three physical elements of a weave, never mixed into one fill:
 *   warp     (vertical threads)  = MEANING    collection / category hue
 *   weft     (horizontal threads)= RESIDENCY  volume tier tone (hot = bright, cold = dark);
 *                                  online-only placeholders are hollow threads, unknown = no weft
 *   selvedge (stitched edge)     = PERMISSION protected = stitched band, pinned = running stitch,
 *                                  unknown (e.g. access denied) = dotted. Drawn only where the hold
 *                                  begins (a child inheriting its parent's hold is not re-stitched).
 * The whole canvas is one continuous cloth: tiles are anchored to canvas space, so neighbouring
 * cells read as regions of a single tapestry, and every cell of the same (warp, weft) pair is
 * filled with one batched Path2D fill. That batching is what keeps 50k-node trees smooth.
 *
 * Level of detail: cells smaller than LOD px draw as a flat warp tone with a weft underline.
 * Render bound: siblings below MIN_AREA px² are aggregated into one "smaller items" cell, so the
 * number of drawn cells is bounded by canvas area, not by tree size.
 *
 * Public API
 *   const tm = createWovenTreemap(canvas, { palette, basis, maxDepth, onHover, onFocus, onSelect })
 *   tm.setRoot(node)            tree root (nodes: {name, size, alloc, children, meaning, residency, permission, kind})
 *   tm.focus(node)              animated drill (in or out); tm.up() goes to the parent
 *   tm.setBasis('size'|'alloc') logical vs allocated area
 *   tm.setPalette(p)            colours (read from CSS tokens by the host)
 *   tm.reveal()                 the warp-then-weft loom reveal
 *   tm.resize()                 re-measure the canvas (also automatic via ResizeObserver)
 *   tm.benchmark(frames)        -> { mean, p95, max, cells, layoutMs } in ms, GPU flush included
 *   tm.destroy()
 *   drawSwatch(canvas, palette, {meaning, residency, permission})  legend swatch, same renderer
 */

const PITCH = 4;            // CSS px per thread
const THREADS = 8;          // threads per tile side
const LOD = 9;              // below this many px on the short side, draw flat
const MIN_AREA = 16;        // px² below which siblings fold into "smaller items"
const HEADER = 17;          // container label band height (top-level regions: HEADER0)
const HEADER0 = 24;
const PAD = 2;

// ---------- colour helpers (exported for the sunburst) ----------
export function hexRgb(hex) {
  const h = hex.trim().replace('#', '');
  const v = parseInt(h.length === 3 ? h.split('').map((c) => c + c).join('') : h, 16);
  return [(v >> 16) & 255, (v >> 8) & 255, v & 255];
}
export function shade(hex, k, alpha = 1) {
  const [r, g, b] = hexRgb(hex);
  const f = (c) => Math.round(k >= 0 ? c + (255 - c) * k : c * (1 + k));
  return `rgba(${f(r)},${f(g)},${f(b)},${alpha})`;
}
const mkCanvas = (w, h) => {
  const c = typeof OffscreenCanvas !== 'undefined' ? new OffscreenCanvas(w, h) : Object.assign(document.createElement('canvas'), { width: w, height: h });
  return c;
};

// ---------- the weave tile ----------
/**
 * 3/1 warp-faced twill (the structure of denim): the warp floats over three wefts and under one,
 * so the meaning hue dominates and the residency weft shows as a diagonal of small flecks.
 * Each crossing is shaded as a rounded thread segment; a float's ends dip into shadow where the
 * thread passes under. The gap at each crossing shows the under-thread, darkened.
 */
function weaveTile(dpr, warp, weft, weftMode, opts = {}) {
  const p = PITCH * dpr, n = THREADS, size = p * n;
  const c = mkCanvas(size, size), g = c.getContext('2d');
  const warpOn = (i, j) => ((i + j) % 4 + 4) % 4 !== 0;
  const jit = (k, salt) => (((k * 73856093) ^ (salt * 19349663)) % 13) / 13 * 0.14 - 0.07;
  g.fillStyle = opts.ink;
  g.fillRect(0, 0, size, size);
  const showWeft = !opts.warpOnly && weftMode !== 'none';
  for (let i = 0; i < n; i++) {
    for (let j = 0; j < n; j++) {
      const x = i * p, y = j * p;
      const wTop = (warpOn(i, j) || !showWeft) && !opts.weftOnly;
      // gap shows the thread underneath
      if (showWeft && weftMode === 'solid' && wTop) { g.fillStyle = shade(weft, -0.62); g.fillRect(x, y, p, p); }
      if (!opts.weftOnly && (!wTop || (showWeft && weftMode !== 'solid'))) { g.fillStyle = shade(warp, -0.66); g.fillRect(x + p * 0.18, y, p * 0.64, p); }
      if (wTop) {
        const k = jit(i, 1);
        const gr = g.createLinearGradient(x, 0, x + p, 0);
        gr.addColorStop(0, shade(warp, -0.55 + k));
        gr.addColorStop(0.32, shade(warp, 0.06 + k));
        gr.addColorStop(0.5, shade(warp, 0.2 + k));
        gr.addColorStop(0.72, shade(warp, -0.04 + k));
        gr.addColorStop(1, shade(warp, -0.58 + k));
        g.fillStyle = gr;
        g.fillRect(x + p * 0.1, y, p * 0.8, p);
        // dip into shadow where the float ends (thread passes under the weft)
        if (showWeft) {
          if (!warpOn(i, j - 1)) { const s = g.createLinearGradient(0, y, 0, y + p * 0.4); s.addColorStop(0, 'rgba(0,0,0,.55)'); s.addColorStop(1, 'rgba(0,0,0,0)'); g.fillStyle = s; g.fillRect(x + p * 0.1, y, p * 0.8, p * 0.4); }
          if (!warpOn(i, j + 1)) { const s = g.createLinearGradient(0, y + p, 0, y + p * 0.6); s.addColorStop(0, 'rgba(0,0,0,.55)'); s.addColorStop(1, 'rgba(0,0,0,0)'); g.fillStyle = s; g.fillRect(x + p * 0.1, y + p * 0.6, p * 0.8, p * 0.4); }
        }
      } else if (weftMode === 'solid') {
        const k = jit(j, 2);
        const gr = g.createLinearGradient(0, y, 0, y + p);
        gr.addColorStop(0, shade(weft, -0.55 + k));
        gr.addColorStop(0.32, shade(weft, 0.04 + k));
        gr.addColorStop(0.5, shade(weft, 0.16 + k));
        gr.addColorStop(0.72, shade(weft, -0.06 + k));
        gr.addColorStop(1, shade(weft, -0.6 + k));
        g.fillStyle = gr;
        g.fillRect(x, y + p * 0.1, p, p * 0.8);
        if (warpOn(i - 1, j)) { const s = g.createLinearGradient(x, 0, x + p * 0.4, 0); s.addColorStop(0, 'rgba(0,0,0,.5)'); s.addColorStop(1, 'rgba(0,0,0,0)'); g.fillStyle = s; g.fillRect(x, y + p * 0.1, p * 0.4, p * 0.8); }
        if (warpOn(i + 1, j)) { const s = g.createLinearGradient(x + p, 0, x + p * 0.6, 0); s.addColorStop(0, 'rgba(0,0,0,.5)'); s.addColorStop(1, 'rgba(0,0,0,0)'); g.fillStyle = s; g.fillRect(x + p * 0.6, y + p * 0.1, p * 0.4, p * 0.8); }
      } else if (weftMode === 'hollow') {
        // online-only placeholder: the weft is there logically but nothing is resident
        g.strokeStyle = shade(weft, 0, 0.85);
        g.lineWidth = Math.max(1, dpr * 0.8);
        g.strokeRect(x + 0.5 * g.lineWidth, y + p * 0.18, p - g.lineWidth, p * 0.64);
      } else if (weftMode === 'unknown') {
        g.fillStyle = shade(opts.unknown, 0, opts.weftOnly ? 0.9 : 0.5);
        g.fillRect(x + p * 0.35, y + p * 0.42, p * 0.3, p * 0.16);
      }
    }
  }
  return c;
}

/** Diagonal fringe for aggregated "smaller items" cells. */
function fringeTile(dpr, color, ink) {
  const s = 6 * dpr, c = mkCanvas(s, s), g = c.getContext('2d');
  g.fillStyle = ink; g.fillRect(0, 0, s, s);
  g.strokeStyle = color; g.lineWidth = dpr * 0.9;
  g.beginPath(); g.moveTo(0, s); g.lineTo(s, 0); g.moveTo(-s / 2, s / 2); g.lineTo(s / 2, -s / 2); g.moveTo(s / 2, s * 1.5); g.lineTo(s * 1.5, s / 2); g.stroke();
  return c;
}

/** Loose, unwoven warp for bytes whose contents are unknown (not yet scanned). */
function looseTile(dpr, color, ink) {
  const w = 14 * dpr, h = 28 * dpr, c = mkCanvas(w, h), g = c.getContext('2d');
  g.fillStyle = ink; g.fillRect(0, 0, w, h);
  g.strokeStyle = color; g.lineWidth = dpr; g.lineCap = 'round';
  for (const [x0, y0, y1] of [[3, 2, 12], [3, 16, 25], [10, 7, 20]]) {
    g.beginPath(); g.moveTo(x0 * dpr, y0 * dpr);
    g.bezierCurveTo((x0 + 2) * dpr, (y0 + 3) * dpr, (x0 - 2) * dpr, (y1 - 3) * dpr, (x0 + 1) * dpr, y1 * dpr);
    g.stroke();
  }
  return c;
}

const ctxPattern = (ctx, tile, dpr) => {
  const pat = ctx.createPattern(tile, 'repeat');
  pat.setTransform(new DOMMatrix().scale(1 / dpr));
  return pat;
};

// ---------- squarified layout (Bruls, Huizing, van Wijk) ----------
function squarify(items, x, y, w, h, out) {
  let i = 0;
  while (i < items.length) {
    const short = Math.min(w, h);
    if (short <= 0) break;
    let sum = items[i].v, j = i + 1;
    let worst = Math.max((short * short * items[i].v) / (sum * sum), (sum * sum) / (short * short * items[i].v));
    while (j < items.length) {
      const s2 = sum + items[j].v;
      const w2 = Math.max((short * short * items[i].v) / (s2 * s2), (s2 * s2) / (short * short * items[j].v));
      if (w2 > worst) break;
      sum = s2; worst = w2; j++;
    }
    if (w >= h) {
      const cw = sum / h; let cy = y;
      for (let k = i; k < j; k++) { const ch = items[k].v / cw; out.push([items[k], x, cy, cw, ch]); cy += ch; }
      x += cw; w -= cw;
    } else {
      const rh = sum / w; let cx = x;
      for (let k = i; k < j; k++) { const cw = items[k].v / rh; out.push([items[k], cx, y, cw, rh]); cx += cw; }
      y += rh; h -= rh;
    }
    i = j;
  }
}

function layout(focus, W, H, sizeOf, maxDepth) {
  const cells = [];
  function place(node, x, y, w, h, depth, parentPerm) {
    const kids = [];
    let total = 0;
    for (const c of node.children || []) { const v = sizeOf(c); if (v > 0) { kids.push(c); total += v; } }
    if (!kids.length || w < 2 || h < 2) return;
    kids.sort((a, b) => sizeOf(b) - sizeOf(a));
    const scale = (w * h) / total;
    const items = [];
    let rest = 0, restN = 0;
    for (const c of kids) {
      const v = sizeOf(c) * scale;
      if (v >= MIN_AREA || items.length === 0) items.push({ v, node: c }); else { rest += v; restN++; }
    }
    if (restN === 1) { // a single straggler is shown as itself, not as "1 smaller item"
      const c = kids[items.length]; items.push({ v: sizeOf(c) * scale, node: c }); rest = 0; restN = 0;
    }
    if (restN) items.push({ v: rest, agg: { count: restN, size: rest / scale, parent: node } });
    const out = [];
    squarify(items, x, y, w, h, out);
    for (const [it, cx, cy, cw, ch] of out) {
      const n = it.node;
      const cell = { x: cx, y: cy, w: cw, h: ch, node: n || null, agg: it.agg || null, depth, leaf: true, header: false, parentPerm, label: null, sub: null };
      cells.push(cell);
      if (n && n.children && n.children.length && depth < maxDepth && cw > 26 && ch > 26) {
        cell.leaf = false;
        cell.header = cw > 64 && ch > 44;
        const top = cell.header ? (depth === 0 ? HEADER0 : HEADER) : PAD;
        place(n, cx + PAD, cy + top, cw - 2 * PAD, ch - top - PAD, depth + 1, n.permission);
      }
    }
  }
  place(focus, 0, 0, W, H, 0, focus.permission);
  return cells;
}

// ---------- labels ----------
function fit(ctx, text, max) {
  if (max <= 8) return null;
  if (ctx.measureText(text).width <= max) return text;
  let lo = 0, hi = text.length;
  while (lo < hi) { const m = (lo + hi + 1) >> 1; if (ctx.measureText(text.slice(0, m) + '…').width <= max) lo = m; else hi = m - 1; }
  return lo >= 2 ? text.slice(0, lo) + '…' : null;
}

const ease = (t) => (t >= 1 ? 1 : 1 - Math.pow(2, -10 * t)); // exponential ease-out

export function createWovenTreemap(canvas, options = {}) {
  const ctx = canvas.getContext('2d');
  let palette = options.palette;
  let basis = options.basis || 'size';
  let maxDepth = options.maxDepth ?? 3;
  const fmt = options.format || ((n) => String(n));
  const reduced = () => options.reducedMotion?.() ?? false;
  let dpr = 1, W = 0, H = 0;
  let root = null, focusNode = null, view = null; // view = { cells, paths, focus }
  let tiles = new Map();
  let anim = null, revealT = 1, raf = 0;
  let hover = null, cursor = 0, selected = null;
  let lastLayoutMs = 0;

  const sizeOf = (n) => (basis === 'alloc' ? n.alloc : n.size) || 0;

  function tileFor(key, make) {
    let t = tiles.get(key);
    if (!t) { t = ctxPattern(ctx, make(), dpr); tiles.set(key, t); }
    return t;
  }
  const meaningColor = (m) => palette.meaning[m] || palette.meaning.none;
  const weftOf = (n) => (n.residency == null ? ['unknown', palette.unknown] : n.residency === 'cloud' ? ['hollow', palette.residency.cloud] : ['solid', palette.residency[n.residency]]);
  function weavePattern(n, warpOnly) {
    const warp = meaningColor(n.meaning);
    const [mode, weft] = weftOf(n);
    return tileFor(`w|${n.meaning}|${n.residency}|${warpOnly ? 1 : 0}`, () => weaveTile(dpr, warp, weft, mode, { ink: palette.ink, unknown: palette.unknown, warpOnly }));
  }

  // Build every (warp, weft) tile while idle so the first zoom into new cloth never stalls a frame.
  function prewarm() {
    const run = () => {
      for (const m of Object.keys(palette.meaning)) for (const r of ['c', 'g', 'e', 'cloud', null]) {
        const n = { meaning: m === 'none' ? null : m, residency: r };
        weavePattern(n, false); weavePattern(n, true);
      }
    };
    (window.requestIdleCallback || ((f) => setTimeout(f, 200)))(run);
  }

  function build(focus) {
    const t0 = performance.now();
    const cells = layout(focus, W, H, sizeOf, maxDepth);
    // batch geometry by fill style
    const woven = new Map(), flat = new Map(), under = new Map();
    const agg = new Path2D(), gap = new Path2D(), heads = new Path2D(), frames = new Path2D();
    const prot = new Path2D(), pin = new Path2D(), unk = new Path2D();
    const labels = [];
    const add = (map, key, make) => { let e = map.get(key); if (!e) { e = make(); map.set(key, e); } return e; };
    for (const c of cells) {
      const g = c.depth === 0 ? 1.5 : 0.5; // gutter: ink between cells, wider between top regions
      const x = c.x + g, y = c.y + g, w = c.w - 2 * g, h = c.h - 2 * g;
      if (w <= 0.3 || h <= 0.3) continue;
      const n = c.node;
      if (c.agg) agg.rect(x, y, w, h);
      else if (n.kind === 'gap') gap.rect(x, y, w, h);
      else if (!c.leaf) {
        frames.rect(x, y, w, h);
        if (c.header) heads.rect(x, y, w, (c.depth === 0 ? HEADER0 : HEADER) - 1);
      } else if (Math.min(w, h) >= LOD) {
        add(woven, `${n.meaning}|${n.residency}`, () => ({ n, path: new Path2D() })).path.rect(x, y, w, h);
      } else {
        add(flat, n.meaning, () => ({ color: meaningColor(n.meaning), path: new Path2D() })).path.rect(x, y, w, h);
        if (h >= 6 && n.residency) add(under, n.residency, () => ({ color: n.residency === 'cloud' ? palette.residency.cloud : palette.residency[n.residency], path: new Path2D() })).path.rect(x, y + h - 1.5, w, 1.5);
      }
      // selvedge only where a hold begins
      if (n && n.permission !== c.parentPerm && w > 5 && h > 5) {
        const p = n.permission === 'protected' ? prot : n.permission === 'pinned' ? pin : n.permission == null ? unk : null;
        if (p) p.rect(x + 1.5, y + 1.5, w - 3, h - 3);
      }
      // labels
      if (c.agg) {
        if (w > 70 && h > 22) labels.push({ c, x, y, w, h, kind: 'agg', name: `${c.agg.count.toLocaleString('en-GB')} smaller`, size: fmt(c.agg.size) });
      } else if (!c.leaf && c.header) labels.push({ c, x, y, w, h, kind: c.depth === 0 ? 'head0' : 'head', name: n.name, size: fmt(sizeOf(n)), unm: n.unmeasured, den: n.deniedBelow || 0 });
      else if (c.leaf && w > 54 && h > 26) labels.push({ c, x, y, w, h, kind: c.depth === 0 && w > 190 && h > 110 ? 'display' : 'leaf', name: n.kind === 'gap' ? n.name : n.name, size: fmt(sizeOf(n)) });
    }
    // pre-fit label text once per layout
    const fontOf = (k) => (k === 'display' ? palette.fontDisplay : k === 'head0' ? palette.fontHead : palette.fontLabel);
    for (const l of labels) {
      const isHead = l.kind === 'head' || l.kind === 'head0';
      const room = isHead ? l.w - 14 : l.w - 18;
      ctx.font = palette.fontMono;
      const sizeW = ctx.measureText(l.size).width;
      ctx.font = fontOf(l.kind);
      if (isHead) {
        const other = l.unm - l.den;
        const suffix = (l.den ? ` · ${l.den} access denied` : '') + (other > 0 ? ` · ${other} unmeasured` : '');
        l.text = fit(ctx, l.name, room - sizeW - 10);
        l.sizeText = room - sizeW > 30 ? l.size + suffix : null;
        if (l.sizeText && suffix) { ctx.font = palette.fontMono; if (ctx.measureText(l.sizeText).width > room - (l.text ? ctx.measureText(l.text).width : 0) - 10) l.sizeText = l.size; }
      } else {
        l.text = fit(ctx, l.name, room);
        l.sizeText = l.h > (l.kind === 'display' ? 60 : 36) && sizeW < room ? l.size : null;
      }
    }
    lastLayoutMs = performance.now() - t0;
    return { focus, cells, woven, flat, under, agg, gap, heads, frames, prot, pin, unk, labels };
  }

  // ---------- drawing one layout under a camera ----------
  function drawView(v, m, alpha, clip, reveal = 1) {
    ctx.save();
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    if (clip) { ctx.beginPath(); ctx.rect(clip.x, clip.y, clip.w, clip.h); ctx.clip(); }
    ctx.globalAlpha = alpha;
    ctx.transform(m.sx, 0, 0, m.sy, m.tx, m.ty);
    // warp, then weft: during the reveal the weft is clipped behind the passing shuttle
    const warpPhase = Math.min(1, reveal / 0.55), weftPhase = Math.max(0, (reveal - 0.42) / 0.58);
    for (const { n, path } of v.woven.values()) {
      if (reveal < 1) {
        ctx.save(); ctx.beginPath(); ctx.rect(0, 0, W, H * ease(warpPhase)); ctx.clip();
        ctx.fillStyle = weavePattern(n, true); ctx.fill(path); ctx.restore();
        if (weftPhase > 0) { ctx.save(); ctx.beginPath(); ctx.rect(0, 0, W * weftPhase, H); ctx.clip(); ctx.fillStyle = weavePattern(n, false); ctx.fill(path); ctx.restore(); }
      } else { ctx.fillStyle = weavePattern(n, false); ctx.fill(path); }
    }
    for (const { color, path } of v.flat.values()) { ctx.fillStyle = shade(color, -0.12); ctx.fill(path); }
    for (const { color, path } of v.under.values()) { ctx.fillStyle = color; ctx.fill(path); }
    ctx.fillStyle = tileFor('fringe', () => fringeTile(dpr, palette.fringe, palette.ink)); ctx.fill(v.agg);
    ctx.fillStyle = tileFor('loose', () => looseTile(dpr, palette.loose, palette.ink)); ctx.fill(v.gap);
    ctx.restore();
  }

  // containers must sit beneath their children, so frames/heads are drawn before the weave
  function drawBase(v, m, alpha, clip) {
    ctx.save();
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    if (clip) { ctx.beginPath(); ctx.rect(clip.x, clip.y, clip.w, clip.h); ctx.clip(); }
    ctx.globalAlpha = alpha;
    ctx.transform(m.sx, 0, 0, m.sy, m.tx, m.ty);
    ctx.fillStyle = palette.frame; ctx.fill(v.frames);
    ctx.fillStyle = palette.head; ctx.fill(v.heads);
    ctx.restore();
  }

  function drawOver(v, m, alpha, clip, reveal = 1) {
    ctx.save();
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    if (clip) { ctx.beginPath(); ctx.rect(clip.x, clip.y, clip.w, clip.h); ctx.clip(); }
    ctx.transform(m.sx, 0, 0, m.sy, m.tx, m.ty);
    const late = reveal >= 1 ? 1 : Math.max(0, (reveal - 0.8) / 0.2);
    ctx.globalAlpha = alpha * late;
    const lw = 1 / Math.sqrt(m.sx * m.sy);
    // selvedge: protected = stitched band, pinned = running stitch, unknown = dotted
    ctx.strokeStyle = palette.permission; ctx.lineWidth = 3 * lw; ctx.setLineDash([]); ctx.stroke(v.prot);
    ctx.strokeStyle = palette.ink; ctx.globalAlpha = alpha * late * 0.7; ctx.lineWidth = 3 * lw; ctx.setLineDash([1.4 * lw, 2.6 * lw]); ctx.stroke(v.prot);
    ctx.globalAlpha = alpha * late;
    ctx.strokeStyle = palette.permission; ctx.lineWidth = 1.6 * lw; ctx.setLineDash([6 * lw, 4 * lw]); ctx.stroke(v.pin);
    ctx.strokeStyle = palette.unknown; ctx.lineWidth = 1.5 * lw; ctx.setLineDash([1.5 * lw, 2.5 * lw]); ctx.stroke(v.unk);
    ctx.setLineDash([]);
    // labels: care-label patches on the cloth
    ctx.textBaseline = 'alphabetic';
    for (const l of v.labels) {
      if (!l.text) continue;
      const x = l.x, y = l.y;
      if (l.kind === 'head' || l.kind === 'head0') {
        const by = l.kind === 'head0' ? y + 17 : y + 12;
        ctx.font = l.kind === 'head0' ? palette.fontHead : palette.fontLabel; ctx.fillStyle = palette.label;
        ctx.fillText(l.text, x + 7, by);
        if (l.sizeText) { ctx.font = palette.fontMono; ctx.fillStyle = palette.labelDim; const tw = ctx.measureText(l.sizeText).width; ctx.fillText(l.sizeText, x + l.w - tw - 7, by); }
        continue;
      }
      const display = l.kind === 'display';
      ctx.font = display ? palette.fontDisplay : palette.fontLabel;
      const tw = ctx.measureText(l.text).width;
      ctx.font = palette.fontMono;
      const sw = l.sizeText ? ctx.measureText(l.sizeText).width : 0;
      const pw = Math.max(tw, sw) + 12, ph = (display ? 26 : 17) + (l.sizeText ? (display ? 17 : 13) : 0);
      ctx.fillStyle = palette.scrim;
      ctx.beginPath(); ctx.roundRect(x + 4, y + 4, Math.min(pw, l.w - 8), Math.min(ph, l.h - 8), 2); ctx.fill();
      ctx.font = display ? palette.fontDisplay : palette.fontLabel; ctx.fillStyle = l.kind === 'agg' ? palette.labelDim : palette.label;
      ctx.fillText(l.text, x + 10, y + (display ? 25 : 17));
      if (l.sizeText) { ctx.font = palette.fontMono; ctx.fillStyle = palette.labelDim; ctx.fillText(l.sizeText, x + 10, y + (display ? 43 : 30)); }
    }
    ctx.restore();
  }

  const identity = { sx: 1, sy: 1, tx: 0, ty: 0 };
  // affine map taking rect a onto rect b
  const mapRect = (a, b) => { const sx = b.w / a.w, sy = b.h / a.h; return { sx, sy, tx: b.x - a.x * sx, ty: b.y - a.y * sy }; };

  function paintLayout(v, m, alpha, clip, reveal) {
    drawBase(v, m, alpha, clip);
    drawView(v, m, alpha, clip, reveal);
    drawOver(v, m, alpha, clip, reveal);
  }

  function highlight() {
    if (!view) return;
    ctx.save();
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const top = view.cells.filter((c) => c.depth === 0);
    const cur = top[cursor];
    if (cur && focusedByKeyboard) {
      ctx.strokeStyle = palette.cursor; ctx.lineWidth = 2; ctx.shadowColor = palette.cursor; ctx.shadowBlur = 12;
      ctx.strokeRect(cur.x + 1, cur.y + 1, cur.w - 2, cur.h - 2);
      ctx.shadowBlur = 0;
    }
    if (hover) {
      ctx.strokeStyle = palette.hover; ctx.lineWidth = 1.5;
      ctx.strokeRect(hover.x + 0.75, hover.y + 0.75, hover.w - 1.5, hover.h - 1.5);
    }
    if (selected) {
      const c = view.cells.find((k) => k.node === selected);
      if (c) { ctx.strokeStyle = palette.cursor; ctx.lineWidth = 1.5; ctx.setLineDash([4, 3]); ctx.strokeRect(c.x + 0.75, c.y + 0.75, c.w - 1.5, c.h - 1.5); }
    }
    ctx.restore();
  }

  function frame(now) {
    raf = 0;
    if (!view) return;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.fillStyle = palette.ink;
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    let more = false;
    if (anim) {
      const t = Math.min(1, (now - anim.start) / anim.dur);
      const e = anim.dir > 0 ? ease(t) : 1 - ease(t);
      // Rt: the inner rect moving between its spot in the outer layout and the full canvas
      const R = anim.rect, full = { x: 0, y: 0, w: W, h: H };
      const Rt = { x: R.x * (1 - e), y: R.y * (1 - e), w: R.w + (W - R.w) * e, h: R.h + (H - R.h) * e };
      paintLayout(anim.outer, mapRect(R, Rt), 1, null, 1);
      const a = Math.min(1, Math.max(0, (e - 0.25) / 0.6));
      ctx.save(); ctx.setTransform(dpr, 0, 0, dpr, 0, 0); ctx.fillStyle = palette.ink; ctx.globalAlpha = a; ctx.fillRect(Rt.x, Rt.y, Rt.w, Rt.h); ctx.restore();
      paintLayout(anim.inner, mapRect(full, Rt), a, Rt, 1);
      if (t < 1) more = true; else anim = null;
    } else {
      if (revealT < 1) {
        revealT = Math.min(1, (now - revealStart) / 1700);
        more = revealT < 1;
      }
      paintLayout(view, identity, 1, null, revealT);
      vignette();
      if (revealT < 1) drawShuttle(revealT);
      else highlight();
    }
    if (more) raf = requestAnimationFrame(frame);
  }

  // gallery light: the cloth falls off softly toward the corners
  function vignette() {
    ctx.save(); ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const g = ctx.createRadialGradient(W * 0.42, H * 0.38, Math.min(W, H) * 0.2, W * 0.5, H * 0.5, Math.hypot(W, H) * 0.62);
    g.addColorStop(0, 'rgba(0,0,0,0)'); g.addColorStop(1, palette.vignette || 'rgba(0,0,0,.38)');
    ctx.fillStyle = g; ctx.fillRect(0, 0, W, H);
    ctx.restore();
  }

  function drawShuttle(r) {
    const weftPhase = (r - 0.42) / 0.58;
    if (weftPhase <= 0 || weftPhase >= 1) return;
    const x = W * weftPhase;
    ctx.save(); ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const g = ctx.createLinearGradient(x - 40, 0, x + 2, 0);
    g.addColorStop(0, 'rgba(0,0,0,0)'); g.addColorStop(1, palette.shuttle);
    ctx.fillStyle = g; ctx.fillRect(x - 40, 0, 42, H);
    ctx.restore();
  }

  let revealStart = 0, focusedByKeyboard = false;
  const schedule = () => { if (!raf) raf = requestAnimationFrame(frame); };

  function resize() {
    const r = canvas.getBoundingClientRect();
    const ndpr = Math.min(window.devicePixelRatio || 1, 3);
    const nw = Math.max(1, Math.round(r.width)), nh = Math.max(1, Math.round(r.height));
    if (nw === W && nh === H && ndpr === dpr && view) return;
    const dprChanged = ndpr !== dpr;
    dpr = ndpr; W = nw; H = nh;
    canvas.width = Math.round(W * dpr); canvas.height = Math.round(H * dpr);
    if (dprChanged) { tiles = new Map(); prewarm(); } // patterns are rasterised at the device pixel ratio
    if (focusNode) { view = build(focusNode); anim = null; schedule(); }
  }
  const ro = new ResizeObserver(() => resize());
  ro.observe(canvas);

  function hitTest(px, py) {
    if (!view) return null;
    for (let i = view.cells.length - 1; i >= 0; i--) {
      const c = view.cells[i];
      if (px >= c.x && px < c.x + c.w && py >= c.y && py < c.y + c.h) return c;
    }
    return null;
  }
  const topOf = (c) => { if (!c) return null; let t = c; while (t.depth > 0) { const p = view.cells.find((k) => k.depth === t.depth - 1 && !k.leaf && k.node === parentOf(t)); if (!p) break; t = p; } return t; };
  const parentOf = (c) => c.agg ? c.agg.parent : c.node.parent;

  function focus(node, opts = {}) {
    if (!node || !node.children || node === focusNode) return;
    const prev = view;
    focusNode = node;
    const next = build(node);
    cursor = 0; hover = null;
    const animate = prev && !reduced() && opts.animate !== false;
    if (animate) {
      // drilling in: the new focus is a descendant of the old one
      let inward = false; for (let p = node.parent; p; p = p.parent) if (p === prev.focus) { inward = true; break; }
      const outer = inward ? prev : next, inner = inward ? next : prev;
      // the inner focus (or its nearest ancestor that the outer layout drew)
      let target = inner.focus, cell = null;
      while (target && !(cell = outer.cells.find((c) => c.node === target))) target = target.parent;
      if (cell) anim = { outer, inner, rect: { x: cell.x, y: cell.y, w: cell.w, h: cell.h }, dir: inward ? 1 : -1, start: performance.now(), dur: 620 };
    }
    view = next;
    options.onFocus?.(node);
    schedule();
  }

  // ---------- input ----------
  let downAt = null;
  canvas.addEventListener('pointermove', (e) => {
    if (anim || revealT < 1) return;
    const r = canvas.getBoundingClientRect();
    const c = hitTest(e.clientX - r.left, e.clientY - r.top);
    if (c !== hover) { hover = c; focusedByKeyboard = false; options.onHover?.(c ? (c.node || null) : null, c, e); schedule(); }
    else if (c) options.onHover?.(c.node || null, c, e);
  });
  canvas.addEventListener('pointerleave', () => { hover = null; options.onHover?.(null, null, null); schedule(); });
  canvas.addEventListener('pointerdown', (e) => { downAt = [e.clientX, e.clientY]; });
  canvas.addEventListener('click', (e) => {
    if (anim) return;
    const r = canvas.getBoundingClientRect();
    const c = hitTest(e.clientX - r.left, e.clientY - r.top);
    if (!c) return;
    if (c.agg) { selected = null; options.onSelect?.(null, c); return; }
    // click opens the top-level region under the pointer; a click on a bare leaf selects it
    const t = topOf(c) || c;
    selected = c.node;
    options.onSelect?.(c.node, c);
    if (t.node && t.node.children && t.node.children.length && t.node.kind !== 'gap') focus(t.node);
    else schedule();
  });
  canvas.addEventListener('keydown', (e) => {
    if (!view) return;
    const top = view.cells.filter((c) => c.depth === 0);
    if (!top.length) return;
    const cur = top[Math.min(cursor, top.length - 1)];
    const dirs = { ArrowRight: [1, 0], ArrowLeft: [-1, 0], ArrowDown: [0, 1], ArrowUp: [0, -1] };
    if (dirs[e.key]) {
      e.preventDefault();
      const [dx, dy] = dirs[e.key];
      const cx = cur.x + cur.w / 2, cy = cur.y + cur.h / 2;
      let best = -1, bestScore = Infinity;
      top.forEach((c, i) => {
        if (c === cur) return;
        const vx = c.x + c.w / 2 - cx, vy = c.y + c.h / 2 - cy;
        const along = vx * dx + vy * dy;
        if (along <= 0) return;
        const across = Math.abs(vx * dy) + Math.abs(vy * dx);
        const s = along + across * 2.2;
        if (s < bestScore) { bestScore = s; best = i; }
      });
      if (best >= 0) cursor = best;
    } else if (e.key === 'Home') { cursor = 0; e.preventDefault(); }
    else if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      if (cur.node && cur.node.children && cur.node.children.length && cur.node.kind !== 'gap') focus(cur.node);
      else { selected = cur.node; options.onSelect?.(cur.node, cur); }
      focusedByKeyboard = true; schedule(); return;
    } else if (e.key === 'Backspace' || e.key === 'Escape') {
      if (focusNode?.parent) { e.preventDefault(); focus(focusNode.parent); }
      focusedByKeyboard = true; return;
    } else return;
    focusedByKeyboard = true;
    const c = top[cursor];
    options.onHover?.(c.node || null, c, null, true);
    schedule();
  });
  canvas.addEventListener('focus', () => { focusedByKeyboard = true; schedule(); });
  canvas.addEventListener('blur', () => { focusedByKeyboard = false; schedule(); });

  resize();
  prewarm();

  return {
    setRoot(n) { root = n; focusNode = null; view = null; focus(n, { animate: false }); },
    focus,
    up() { if (focusNode?.parent) focus(focusNode.parent); },
    get focused() { return focusNode; },
    get root() { return root; },
    select(n) { selected = n; schedule(); },
    setBasis(b) { if (b === basis) return; basis = b; if (focusNode) { view = build(focusNode); anim = null; schedule(); } },
    setPalette(p) { palette = p; tiles = new Map(); prewarm(); if (focusNode) { view = build(focusNode); schedule(); } },
    reveal() { if (reduced()) { revealT = 1; schedule(); return; } revealT = 0; revealStart = performance.now(); schedule(); },
    resize,
    get cellCount() { return view ? view.cells.length : 0; },
    /** Paint the settled view synchronously (for measuring inside a rAF loop). */
    renderFrame() { revealT = 1; anim = null; frame(performance.now()); },
    /** Synchronous frames with a forced GPU flush. opts.maxDepth temporarily deepens nesting. */
    benchmark(frames = 60, opts = {}) {
      const keepDepth = maxDepth;
      if (opts.maxDepth != null) maxDepth = opts.maxDepth;
      const times = [];
      const t0 = performance.now(); view = build(focusNode); const layoutMs = performance.now() - t0;
      const probe = ctx.getImageData(0, 0, 1, 1); // warm
      for (let i = 0; i < frames; i++) {
        const s = performance.now();
        ctx.setTransform(1, 0, 0, 1, 0, 0); ctx.fillStyle = palette.ink; ctx.fillRect(0, 0, canvas.width, canvas.height);
        paintLayout(view, identity, 1, null, 1); highlight();
        ctx.getImageData(0, 0, 1, 1); // force the GPU flush so the time is real
        times.push(performance.now() - s);
      }
      void probe;
      times.sort((a, b) => a - b);
      const cells = view.cells.length;
      maxDepth = keepDepth; view = build(focusNode); schedule();
      return { mean: times.reduce((a, b) => a + b, 0) / times.length, p95: times[Math.floor(times.length * 0.95)], max: times[times.length - 1], cells, layoutMs, frames, maxDepth: opts.maxDepth ?? keepDepth };
    },
    destroy() { ro.disconnect(); cancelAnimationFrame(raf); },
  };
}

/** Draw a small woven swatch (legend, inspector) with the same tile renderer. */
export function drawSwatch(canvas, palette, { meaning = null, residency = null, permission = 'none', only = null } = {}) {
  const dpr = Math.min(window.devicePixelRatio || 1, 3);
  const r = canvas.getBoundingClientRect();
  const w = Math.max(1, Math.round(r.width || canvas.width)), h = Math.max(1, Math.round(r.height || canvas.height));
  canvas.width = w * dpr; canvas.height = h * dpr;
  const g = canvas.getContext('2d');
  const warp = palette.meaning[meaning] || palette.meaning.none;
  const mode = residency == null ? 'unknown' : residency === 'cloud' ? 'hollow' : residency === 'none' ? 'none' : 'solid';
  const weft = residency === 'cloud' ? palette.residency.cloud : palette.residency[residency] || palette.unknown;
  // only: 'warp' | 'weft' | 'selvedge' isolates one channel for the legend
  const tile = weaveTile(dpr, warp, weft, only === 'warp' ? 'none' : mode, { ink: palette.ink, unknown: palette.unknown, weftOnly: only === 'weft' });
  const pat = g.createPattern(tile, 'repeat');
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  pat.setTransform(new DOMMatrix().scale(1 / dpr));
  g.fillStyle = palette.ink; g.fillRect(0, 0, w, h);
  if (only !== 'selvedge') { g.fillStyle = pat; g.fillRect(0, 0, w, h); }
  if (permission === 'protected') {
    g.strokeStyle = palette.permission; g.lineWidth = 3; g.strokeRect(1.5, 1.5, w - 3, h - 3);
    g.strokeStyle = palette.ink; g.globalAlpha = 0.7; g.setLineDash([1.4, 2.6]); g.strokeRect(1.5, 1.5, w - 3, h - 3);
  } else if (permission === 'pinned') {
    g.strokeStyle = palette.permission; g.lineWidth = 1.6; g.setLineDash([6, 4]); g.strokeRect(1.5, 1.5, w - 3, h - 3);
  } else if (permission == null) {
    g.strokeStyle = palette.unknown; g.lineWidth = 1.5; g.setLineDash([1.5, 2.5]); g.strokeRect(1.5, 1.5, w - 3, h - 3);
  }
}
