/**
 * Observatory sunburst: a radial partition of a TreeSlice that you orbit (drag, with inertia) and
 * drill (click an arc; click the centre or press Escape to go back). Canvas 2D, DPR aware.
 * Contract: docs/41 section 13 (same surface as the woven treemap).
 *
 * The same three channels on a circular loom:
 *   warp     = meaning:    arc hue plus fine radial threads; a suggestion is woven half-dyed (dashed spokes)
 *   weft     = residency:  concentric metal threads clipped to the arc; hollow (dashed) for placeholders; absent when unknown
 *   selvedge = permission: the stitch on the arc's outer rim where a state begins
 * Bytes the slice did not list are hatched loose threads; zero-area children are named at the centre.
 * The cloth is cached in a rotation-free layer, so orbiting costs one rotated blit per frame.
 *
 *   const sb = createSunburst(canvas, { palette, reducedMotion, format, rings })
 *   sb.setSlice(slice)  sb.setThreads(t)  sb.setTheme(p)  sb.setBasis(b)  sb.focus(id)  sb.resize()  sb.destroy()
 *   sb.on('select' | 'hover' | 'drill' | 'back', handler) -> unsubscribe
 *   sb.rotateBy(rad)  sb.renderFrame()
 */
import { buildTree, partition } from './layout.js';
import { shade, selvedgeOf, warpOf, weftOf } from './colour.js';
import { strokeStitch } from './weave.js';
import { nodeInfo, zeroCounts } from './woven-treemap.js';

/** @typedef {import('./types.js').Palette} Palette */
/** @typedef {import('./types.js').SliceLike} SliceLike */
/** @typedef {import('./types.js').ThreadToggles} ThreadToggles */
/** @typedef {import('./types.js').VizEvent} VizEvent */
/** @typedef {import('./layout.js').VNode} VNode */
/** @typedef {import('./colour.js').Stitch} Stitch */
/** @typedef {{ n: VNode, a0: number, a1: number, ri: number, ro: number, k: number }} Arc */

const TAU = Math.PI * 2;
const MIN_SPAN = 0.0045; // radians: below this an arc is too fine to draw and the ring track shows through
const ease = (/** @type {number} */ t) => (t >= 1 ? 1 : 1 - Math.pow(2, -10 * t));

/**
 * @param {HTMLCanvasElement} canvas
 * @param {{ palette: Palette, reducedMotion?: () => boolean, format?: (n: number) => string, rings?: number }} options
 */
export function createSunburst(canvas, options) {
  const c0 = canvas.getContext('2d');
  if (!c0) throw new Error('2d canvas unavailable');
  /** @type {CanvasRenderingContext2D} */
  let ctx = c0;
  const main = c0;
  let palette = options.palette;
  const RINGS = options.rings ?? 4;
  const fmt = options.format ?? ((/** @type {number} */ n) => String(n));
  const reduced = () => options.reducedMotion?.() ?? false;
  /** @type {ThreadToggles} */
  let show = { meaning: true, residency: true, permission: true };
  let dpr = 1, W = 0, H = 0, cx = 0, cy = 0, R = 0, r0 = 0, T = 0;
  /** @type {VNode | null} */
  let root = null;
  /** @type {Map<string, VNode>} */
  let byId = new Map();
  /** @type {Map<VNode, [number, number]>} */
  let part = new Map();
  /** @type {Map<VNode, number>} */
  let depthOf = new Map();
  let provisional = false;
  let v = { x0: 0, x1: 1, d: 0 };
  /** @type {{ from: typeof v, to: typeof v, start: number, dur: number, th0: number, th1: number, swap: null | (() => void) } | null} */
  let anim = null;
  let theta = 0, spin = 0, raf = 0, rot = 0, palVersion = 0, sliceVersion = 0;
  /** @type {VNode | null} */
  let hover = null;
  let cursor = 0, kbd = false;
  /** @type {{ x: number, y: number, a: number, moved: boolean } | null} */
  let drag = null;
  /** @type {Map<VizEvent, Set<(e: any) => void>>} */
  const handlers = new Map();
  /** @param {VizEvent} name @param {unknown} payload */
  const emit = (name, payload) => handlers.get(name)?.forEach((h) => h(payload));

  /** @param {VNode} r @param {Map<string, VNode>} ids */
  function adopt(r, ids) {
    root = r; byId = ids; part = partition(r);
    depthOf = new Map();
    /** @param {VNode} n @param {number} d */
    const walk = (n, d) => { depthOf.set(n, d); n.children.forEach((c) => walk(c, d + 1)); };
    walk(r, 0);
    sliceVersion++;
  }

  const ringR = (/** @type {number} */ k) => r0 + k * T;
  const ang = (/** @type {number} */ x) => rot - Math.PI / 2 + ((x - v.x0) / (v.x1 - v.x0)) * TAU;

  function collect() {
    /** @type {Arc[]} */
    const arcs = [];
    if (!root) return arcs;
    /** @param {VNode} n */
    const walk = (n) => {
      const p = part.get(n);
      if (!p) return;
      const [x0, x1] = p;
      if (x1 <= v.x0 || x0 >= v.x1) return;
      const k = (depthOf.get(n) ?? 0) - v.d;
      if (k > RINGS + 0.001) return;
      const a0 = ang(Math.max(x0, v.x0)), a1 = ang(Math.min(x1, v.x1));
      if (a1 - a0 < MIN_SPAN) return;
      if (k > 0) {
        const ri = Math.max(r0, ringR(k - 1)), ro = ringR(k) - 1.5;
        if (ro > ri + 0.5) arcs.push({ n, a0, a1, ri, ro, k });
      }
      for (const c of n.children) walk(c);
    };
    walk(root);
    return arcs;
  }

  /** @param {Path2D} p @param {Arc} a */
  const arcPath = (p, a) => { p.moveTo(cx + a.ri * Math.cos(a.a0), cy + a.ri * Math.sin(a.a0)); p.arc(cx, cy, a.ro, a.a0, a.a1); p.arc(cx, cy, a.ri, a.a1, a.a0, true); p.closePath(); };

  /** @type {{ key: string, half: number, warp: HTMLCanvasElement, weft: Map<string, HTMLCanvasElement> } | null} */
  let tex = null;
  function textures() {
    const key = `${W}x${H}@${dpr}|${palVersion}`;
    if (tex && tex.key === key) return tex;
    const half = R + 2, S = Math.ceil(half * 2 * dpr);
    const mk = () => {
      const c = document.createElement('canvas'); c.width = c.height = S;
      const g = /** @type {CanvasRenderingContext2D} */ (c.getContext('2d'));
      g.setTransform(dpr, 0, 0, dpr, S / 2, S / 2);
      return /** @type {[HTMLCanvasElement, CanvasRenderingContext2D]} */ ([c, g]);
    };
    const [wc, wg] = mk();
    wg.strokeStyle = 'rgba(255,255,255,.12)'; wg.lineWidth = 0.8; wg.beginPath();
    for (let i = 0; i < 320; i++) { const a = (i / 320) * TAU; wg.moveTo(r0 * Math.cos(a), r0 * Math.sin(a)); wg.lineTo(R * Math.cos(a), R * Math.sin(a)); }
    wg.stroke();
    /** @type {Map<string, HTMLCanvasElement>} */
    const weft = new Map();
    tex = { key, half, warp: wc, weft };
    return tex;
  }
  /** @param {string} colour @param {boolean} dashed */
  function weftTex(colour, dashed) {
    const t = textures();
    const k = `${colour}|${dashed}`;
    let c = t.weft.get(k);
    if (c) return c;
    const S = Math.ceil(t.half * 2 * dpr);
    c = document.createElement('canvas'); c.width = c.height = S;
    const g = /** @type {CanvasRenderingContext2D} */ (c.getContext('2d'));
    g.setTransform(dpr, 0, 0, dpr, S / 2, S / 2);
    g.strokeStyle = colour; g.globalAlpha = 0.38; g.lineWidth = 0.9;
    if (dashed) g.setLineDash([2, 3]);
    g.beginPath();
    for (let rr = r0 + 2; rr < R; rr += 3.6) { g.moveTo(rr, 0); g.arc(0, 0, rr, 0, TAU); }
    g.stroke();
    t.weft.set(k, c);
    return c;
  }

  /** @param {Arc[]} arcs @param {boolean} threads */
  function paintCloth(arcs, threads) {
    /** @type {Map<string, Path2D>} */
    const byWarp = new Map();
    /** @type {Map<string, { path: Path2D, dashed: boolean }>} */
    const byWeft = new Map();
    const all = new Path2D(), rest = new Path2D(), suggested = new Path2D(), pending = new Path2D();
    /** @type {Map<Stitch, { colour: string, path: Path2D }>} */
    const stitches = new Map();
    for (const a of arcs) {
      const src = a.n.src;
      if (!src || src.kind === 'other') { arcPath(rest, a); continue; } // unlisted or engine-folded bytes: hatched
      const warp = warpOf(src.threads.meaning, palette, show.meaning);
      let p = byWarp.get(warp.colour);
      if (!p) { p = new Path2D(); byWarp.set(warp.colour, p); }
      arcPath(p, a);
      arcPath(all, a);
      if (warp.pattern === 'suggested') arcPath(suggested, a);
      if (warp.pattern === 'pending') arcPath(pending, a);
      const weft = weftOf(src.threads, palette, show.residency);
      if (weft.colour) {
        const k = `${weft.colour}|${weft.pattern === 'hollow'}`;
        let e = byWeft.get(k);
        if (!e) { e = { path: new Path2D(), dashed: weft.pattern === 'hollow' }; byWeft.set(k, e); }
        arcPath(e.path, a);
      }
      const parent = a.n.parent?.src?.threads.permission ?? null;
      const s = selvedgeOf(src.threads.permission, parent, palette, show.permission);
      if (s) {
        let e = stitches.get(s.stitch);
        if (!e) { e = { colour: s.colour, path: new Path2D() }; stitches.set(s.stitch, e); }
        e.path.moveTo(cx + (a.ro - 1.5) * Math.cos(a.a0), cy + (a.ro - 1.5) * Math.sin(a.a0));
        e.path.arc(cx, cy, a.ro - 1.5, a.a0, a.a1);
      }
    }
    for (const [colour, path] of byWarp) {
      const g = ctx.createRadialGradient(cx, cy, r0, cx, cy, R);
      g.addColorStop(0, shade(colour, -0.35, 0.95)); g.addColorStop(1, shade(colour, 0.08, 0.95));
      ctx.fillStyle = g; ctx.fill(path);
    }
    // a suggestion is half-dyed: the ink shows through every other spoke
    ctx.save(); ctx.clip(suggested); ctx.strokeStyle = palette.ink; ctx.globalAlpha = 0.55; ctx.lineWidth = 2; ctx.beginPath();
    for (let i = 0; i < 180; i++) { const a = (i / 180) * TAU; ctx.moveTo(cx + r0 * Math.cos(a), cy + r0 * Math.sin(a)); ctx.lineTo(cx + R * Math.cos(a), cy + R * Math.sin(a)); }
    ctx.stroke(); ctx.restore();
    ctx.save(); ctx.strokeStyle = palette.unknown; ctx.setLineDash([2, 4]); ctx.lineWidth = 1; ctx.stroke(pending); ctx.restore();
    if (threads) {
      const t = textures();
      /** @param {HTMLCanvasElement} img @param {Path2D} path */
      const stamp = (img, path) => { ctx.save(); ctx.clip(path); ctx.translate(cx, cy); ctx.drawImage(img, -t.half, -t.half, t.half * 2, t.half * 2); ctx.restore(); };
      stamp(t.warp, all);
      for (const [k, e] of byWeft) stamp(weftTex(/** @type {string} */ (k.split('|')[0]), e.dashed), e.path);
    }
    ctx.save(); ctx.clip(rest);
    ctx.strokeStyle = palette.loose; ctx.lineWidth = 1; ctx.beginPath();
    for (let x = -R; x < R; x += 6) { ctx.moveTo(cx + x, cy - R); ctx.lineTo(cx + x + R, cy + R); }
    ctx.stroke(); ctx.restore();
    ctx.save(); ctx.strokeStyle = palette.unknown; ctx.setLineDash([2, 3]); ctx.lineWidth = 1; ctx.stroke(rest); ctx.restore();
    for (const [stitch, e] of stitches) strokeStitch(ctx, e.path, stitch, e.colour, palette.ink);
    if (provisional) { ctx.save(); ctx.strokeStyle = palette.permission; ctx.setLineDash([3, 5]); ctx.lineWidth = 1.2; ctx.beginPath(); ctx.arc(cx, cy, ringR(RINGS) + 10, 0, TAU); ctx.stroke(); ctx.restore(); }
    ctx.save(); ctx.strokeStyle = palette.cursor; ctx.globalAlpha = 0.28; ctx.lineWidth = 1; ctx.shadowColor = palette.cursor; ctx.shadowBlur = 16;
    ctx.beginPath(); ctx.arc(cx, cy, ringR(RINGS) + 3, 0, TAU); ctx.stroke(); ctx.restore();
  }

  /** @type {{ c: HTMLCanvasElement, key: string } | null} */
  let layer = null;
  let benchMoving = false;
  function clothLayer() {
    const moving = !!anim || benchMoving;
    const key = `${v.x0}|${v.x1}|${v.d}|${W}x${H}@${dpr}|${palVersion}|${sliceVersion}|${show.meaning}${show.residency}${show.permission}|${moving}`;
    if (layer && layer.key === key) return layer;
    if (!layer) layer = { c: document.createElement('canvas'), key: '' };
    const c = layer.c;
    if (c.width !== canvas.width || c.height !== canvas.height) { c.width = canvas.width; c.height = canvas.height; }
    ctx = /** @type {CanvasRenderingContext2D} */ (c.getContext('2d'));
    ctx.setTransform(1, 0, 0, 1, 0, 0); ctx.clearRect(0, 0, c.width, c.height);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    rot = 0;
    paintCloth(collect(), !moving);
    ctx = main;
    layer.key = key;
    return layer;
  }

  /** @type {Map<VNode, number>} */
  let widths = new Map();
  /** @param {Arc[]} arcs */
  function drawLabels(arcs) {
    ctx.textBaseline = 'middle';
    ctx.font = palette.fontLabel;
    for (const a of arcs) {
      const span = a.a1 - a.a0, th = a.ro - a.ri, rm = (a.ri + a.ro) / 2, L = span * rm;
      if (L < 12 || th < 11) continue;
      let text = a.n.name;
      const am = (a.a0 + a.a1) / 2;
      const norm = ((am % TAU) + TAU) % TAU;
      let tw = widths.get(a.n);
      if (tw === undefined) { tw = ctx.measureText(text).width; widths.set(a.n, tw); }
      if (L > 60 && th > 14 && L > tw + 14) {
        ctx.save(); ctx.translate(cx + rm * Math.cos(am), cy + rm * Math.sin(am));
        ctx.rotate(am + (norm > 0 && norm < Math.PI ? -Math.PI / 2 : Math.PI / 2));
        ctx.textAlign = 'center'; ctx.fillStyle = a.n.src ? palette.label : palette.labelDim; ctx.fillText(text, 0, 0);
        ctx.restore();
      } else if (L > 15 && th > 26) {
        while (tw > th - 8 && text.length > 3) { text = text.slice(0, -2) + '…'; tw = ctx.measureText(text).width; }
        if (tw > th - 8) continue;
        ctx.save(); ctx.translate(cx + rm * Math.cos(am), cy + rm * Math.sin(am));
        ctx.rotate(am + (norm > Math.PI / 2 && norm < (3 * Math.PI) / 2 ? Math.PI : 0));
        ctx.textAlign = 'center'; ctx.fillStyle = a.n.src ? palette.label : palette.labelDim; ctx.fillText(text, 0, 0);
        ctx.restore();
      }
    }
  }

  function drawScale() {
    // 100 ticks = 1% of the focused node each, rotating with the orbit
    const ro = ringR(RINGS) + 6;
    ctx.strokeStyle = palette.scale; ctx.lineWidth = 1;
    ctx.beginPath();
    for (let i = 0; i < 100; i++) {
      const a = theta - Math.PI / 2 + (i / 100) * TAU, len = i % 25 === 0 ? 9 : i % 5 === 0 ? 5 : 2.5;
      ctx.moveTo(cx + ro * Math.cos(a), cy + ro * Math.sin(a)); ctx.lineTo(cx + (ro + len) * Math.cos(a), cy + (ro + len) * Math.sin(a));
    }
    ctx.stroke();
    ctx.font = palette.fontSmall; ctx.fillStyle = palette.labelDim; ctx.textAlign = 'center'; ctx.textBaseline = 'middle';
    for (let i = 0; i < 4; i++) {
      const a = theta - Math.PI / 2 + (i / 4) * TAU;
      ctx.fillText(`${i * 25}%`, cx + (ro + 20) * Math.cos(a), cy + (ro + 20) * Math.sin(a));
    }
  }

  function focusNode() {
    // the node at the window's centre depth: the slice anchor unless mid-zoom
    return root;
  }

  function drawCentre() {
    const n = focusNode();
    if (!n) return;
    ctx.fillStyle = palette.frame; ctx.beginPath(); ctx.arc(cx, cy, r0 - 3, 0, TAU); ctx.fill();
    ctx.strokeStyle = palette.track; ctx.lineWidth = 1; ctx.stroke();
    ctx.textAlign = 'center'; ctx.textBaseline = 'middle';
    ctx.font = palette.fontHead; ctx.fillStyle = palette.label;
    let name = n.name;
    if (ctx.measureText(name).width > r0 * 1.6) ctx.font = palette.fontLabel;
    while (ctx.measureText(name).width > r0 * 1.6 && name.length > 3) name = name.slice(0, -2) + '…';
    ctx.fillText(name, cx, cy - 9);
    ctx.font = palette.fontNum; ctx.fillStyle = palette.labelDim;
    ctx.fillText(fmt(n.size), cx, cy + 10);
    ctx.font = palette.fontSmall;
    let line = cy + 27;
    const z = zeroCounts(n);
    if (z.denied) { ctx.fillStyle = palette.permission; ctx.fillText(`${z.denied} access denied`, cx, line); line += 14; }
    if (z.unmeasured) { ctx.fillStyle = palette.unknown; ctx.fillText(`${z.unmeasured} unmeasured`, cx, line); line += 14; }
    ctx.fillStyle = palette.labelDim;
    if (line < cy + r0 - 6) ctx.fillText('centre: back', cx, line);
  }

  function draw() {
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.fillStyle = palette.ink; ctx.fillRect(0, 0, canvas.width, canvas.height);
    if (!root) return 0;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.lineWidth = 1;
    for (let k = 1; k <= RINGS; k++) { ctx.strokeStyle = palette.track; ctx.beginPath(); ctx.arc(cx, cy, ringR(k) - 1, 0, TAU); ctx.stroke(); }
    const L = clothLayer();
    ctx.save(); ctx.translate(cx, cy); ctx.rotate(theta); ctx.drawImage(L.c, -cx, -cy, W, H); ctx.restore();
    rot = theta;
    const arcs = collect();
    drawScale();
    drawLabels(arcs);
    drawCentre();
    /** @type {[VNode, string][]} */
    const hi = [];
    if (hover) hi.push([hover, palette.hover]);
    if (kbd) { const kids = cursorKids(); const k = kids[cursor]; if (k) hi.push([k, palette.cursor]); }
    for (const [n, col] of hi) {
      const a = arcs.find((x) => x.n === n);
      if (!a) continue;
      const p = new Path2D(); arcPath(p, a);
      ctx.save(); ctx.shadowColor = col; ctx.shadowBlur = 14; ctx.strokeStyle = col; ctx.lineWidth = 1.6; ctx.stroke(p); ctx.restore();
    }
    return arcs.length;
  }

  /** @param {number} now */
  function frame(now) {
    raf = 0;
    let more = false;
    if (anim) {
      const t = Math.min(1, (now - anim.start) / anim.dur), e = ease(t);
      v = { x0: anim.from.x0 + (anim.to.x0 - anim.from.x0) * e, x1: anim.from.x1 + (anim.to.x1 - anim.from.x1) * e, d: anim.from.d + (anim.to.d - anim.from.d) * e };
      theta = anim.th0 + (anim.th1 - anim.th0) * e;
      if (t >= 1) { const swap = anim.swap; anim = null; if (swap) swap(); } else more = true;
    }
    if (!drag && Math.abs(spin) > 0.0004 && !reduced()) { theta += spin; spin *= 0.94; more = true; } else if (!drag) spin = 0;
    draw();
    if (more) raf = requestAnimationFrame(frame);
  }
  const schedule = () => { if (!raf) raf = requestAnimationFrame(frame); };

  function resize() {
    const r = canvas.getBoundingClientRect();
    dpr = Math.min(window.devicePixelRatio || 1, 3);
    W = Math.max(1, Math.round(r.width)); H = Math.max(1, Math.round(r.height));
    canvas.width = Math.round(W * dpr); canvas.height = Math.round(H * dpr);
    cx = W / 2; cy = H / 2;
    R = Math.max(40, Math.min(W, H) / 2 - 34);
    r0 = Math.max(34, R * 0.24);
    T = (R - r0) / RINGS;
    schedule();
  }
  const ro = new ResizeObserver(resize); ro.observe(canvas);

  /** @param {number} px @param {number} py @returns {{ centre: true } | { node: VNode } | null} */
  function pick(px, py) {
    const dx = px - cx, dy = py - cy, r = Math.hypot(dx, dy);
    if (r < r0) return { centre: true };
    const k = Math.ceil((r - r0) / T);
    if (k < 1 || k > RINGS || !root) return null;
    let a = Math.atan2(dy, dx) - (theta - Math.PI / 2);
    a = ((a % TAU) + TAU) % TAU;
    const x = v.x0 + (a / TAU) * (v.x1 - v.x0);
    /** @type {VNode} */
    let n = root;
    for (let i = 0; i < k; i++) {
      const c = n.children.find((c) => { const p = part.get(c); return !!p && x >= p[0] && x < p[1]; });
      if (!c) return null;
      n = c;
    }
    return { node: n };
  }
  const cursorKids = () => (root ? root.children : []);
  /** @param {MouseEvent} e */
  const local = (e) => { const r = canvas.getBoundingClientRect(); return [e.clientX - r.left, e.clientY - r.top]; };

  /** @param {VNode | null} n @param {MouseEvent | null} ev @param {boolean} viaKeyboard */
  const sendHover = (n, ev, viaKeyboard) => emit('hover', { node: n ? nodeInfo(n) : null, clientX: ev ? ev.clientX : 0, clientY: ev ? ev.clientY : 0, viaKeyboard });

  /** @param {PointerEvent} e */
  const onDown = (e) => {
    const [x, y] = local(e);
    drag = { x: /** @type {number} */ (x), y: /** @type {number} */ (y), a: Math.atan2(/** @type {number} */ (y) - cy, /** @type {number} */ (x) - cx), moved: false };
    spin = 0;
    canvas.setPointerCapture(e.pointerId);
  };
  /** @param {PointerEvent} e */
  const onMove = (e) => {
    const [x = 0, y = 0] = local(e);
    if (drag) {
      if (!drag.moved && Math.hypot(x - drag.x, y - drag.y) > 4) drag.moved = true;
      if (drag.moved) {
        const a = Math.atan2(y - cy, x - cx);
        let d = a - drag.a; if (d > Math.PI) d -= TAU; if (d < -Math.PI) d += TAU;
        theta += d; spin = d; drag.a = a;
        canvas.style.cursor = 'grabbing';
        schedule();
        return;
      }
    }
    if (anim) return;
    const p = pick(x, y);
    const n = p && 'node' in p ? p.node : null;
    if (n !== hover) { hover = n; kbd = false; schedule(); }
    canvas.style.cursor = p && 'centre' in p ? 'zoom-out' : n ? 'pointer' : 'grab';
    sendHover(n, e, false);
  };
  /** @param {PointerEvent} e */
  const onUp = (e) => {
    const d = drag; drag = null; canvas.style.cursor = '';
    if (!d) return;
    if (d.moved) { schedule(); return; }
    const [x = 0, y = 0] = local(e);
    const p = pick(x, y);
    if (!p) return;
    if ('centre' in p) { emit('back', null); return; }
    const info = nodeInfo(p.node);
    emit('select', info);
    // drill to the top-level arc under the pointer (the next slice brings its deeper rings)
    let t = p.node; while (t.parent && t.parent !== root) t = t.parent;
    const ti = nodeInfo(t);
    if (ti.drillable) emit('drill', ti);
  };
  const onLeave = () => { if (!drag) { hover = null; sendHover(null, null, false); schedule(); } };
  /** @param {KeyboardEvent} e */
  const onKey = (e) => {
    const kids = cursorKids();
    if (!kids.length) return;
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') cursor = (cursor + 1) % kids.length;
    else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') cursor = (cursor - 1 + kids.length) % kids.length;
    else if (e.key === 'Home') cursor = 0;
    else if (e.key === 'End') cursor = kids.length - 1;
    else if (e.key === ']') theta += 0.12;
    else if (e.key === '[') theta -= 0.12;
    else if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault(); kbd = true;
      const n = /** @type {VNode} */ (kids[cursor]);
      const info = nodeInfo(n);
      emit('select', info);
      if (info.drillable) emit('drill', info);
      schedule(); return;
    } else if (e.key === 'Escape' || e.key === 'Backspace') { e.preventDefault(); kbd = true; emit('back', null); return; }
    else return;
    e.preventDefault(); kbd = true;
    sendHover(kids[cursor] ?? null, null, true);
    schedule();
  };
  const onFocus = () => { kbd = true; schedule(); };
  const onBlur = () => { kbd = false; schedule(); };
  canvas.addEventListener('pointerdown', onDown);
  canvas.addEventListener('pointermove', onMove);
  canvas.addEventListener('pointerup', onUp);
  canvas.addEventListener('pointerleave', onLeave);
  canvas.addEventListener('keydown', onKey);
  canvas.addEventListener('focus', onFocus);
  canvas.addEventListener('blur', onBlur);

  resize();

  return {
    /** @param {SliceLike} s @param {{ animate?: boolean }} [opts] */
    setSlice(s, opts = {}) {
      const next = buildTree(s);
      const prevRoot = root, prevIds = byId;
      provisional = s.aggregate_state === 'provisional_live';
      cursor = 0; hover = null; widths = new Map();
      const animate = prevRoot && opts.animate !== false && !reduced();
      const inward = animate && next.root.id !== prevRoot.id ? prevIds.get(next.root.id) : undefined;
      if (inward && part.get(inward)) {
        // zoom the old cloth into the arc, then hand over to the new slice
        const [x0, x1] = /** @type {[number, number]} */ (part.get(inward));
        anim = { from: { ...v }, to: { x0, x1, d: depthOf.get(inward) ?? 1 }, start: performance.now(), dur: 650, th0: theta, th1: Math.round(theta / TAU) * TAU,
          swap: () => { adopt(next.root, next.byId); v = { x0: 0, x1: 1, d: 0 }; schedule(); } };
      } else {
        adopt(next.root, next.byId);
        const back = animate && prevRoot ? next.byId.get(prevRoot.id) : undefined;
        const bp = back ? part.get(back) : undefined;
        if (back && bp) {
          v = { x0: bp[0], x1: bp[1], d: depthOf.get(back) ?? 1 };
          anim = { from: { ...v }, to: { x0: 0, x1: 1, d: 0 }, start: performance.now(), dur: 650, th0: theta, th1: Math.round(theta / TAU) * TAU, swap: null };
        } else { v = { x0: 0, x1: 1, d: 0 }; anim = null; }
      }
      schedule();
    },
    /** @param {Partial<ThreadToggles>} t */
    setThreads(t) { show = { ...show, ...t }; schedule(); },
    /** @param {Palette} p */
    setTheme(p) { palette = p; palVersion++; widths = new Map(); schedule(); },
    /** @param {'logical' | 'allocated'} _b  sizes arrive with the next slice */
    setBasis(_b) { schedule(); },
    /** @param {string} id */
    focus(id) {
      const kids = cursorKids();
      const i = kids.findIndex((k) => k.id === id);
      if (i >= 0) { cursor = i; kbd = true; sendHover(kids[i] ?? null, null, true); schedule(); }
    },
    /** @param {number} a */
    rotateBy(a) { theta += a; schedule(); },
    resize,
    /** @param {VizEvent} name @param {(e: any) => void} handler @returns {() => void} */
    on(name, handler) {
      let set = handlers.get(name);
      if (!set) { set = new Set(); handlers.set(name, set); }
      set.add(handler);
      return () => { set.delete(handler); };
    },
    /** Paint synchronously; `rebuild` re-renders the cloth layer as a zoom frame would. @param {{ rebuild?: boolean }} [o] */
    renderFrame(o = {}) { if (o.rebuild && layer) { benchMoving = true; layer.key = ''; } const n = draw(); benchMoving = false; return n; },
    destroy() {
      ro.disconnect(); cancelAnimationFrame(raf); handlers.clear();
      canvas.removeEventListener('pointerdown', onDown); canvas.removeEventListener('pointermove', onMove); canvas.removeEventListener('pointerup', onUp);
      canvas.removeEventListener('pointerleave', onLeave); canvas.removeEventListener('keydown', onKey); canvas.removeEventListener('focus', onFocus); canvas.removeEventListener('blur', onBlur);
    },
  };
}
