/**
 * Observatory sunburst: a radial partition you orbit (drag to rotate, with inertia) and zoom
 * into (click an arc; click the centre or press Escape to zoom out). Canvas 2D, DPR aware.
 *
 * Same three channels as the woven atlas, translated to a circular loom:
 *   warp     = MEANING    arc fill hue plus fine radial threads
 *   weft     = RESIDENCY  concentric threads clipped to the arc (hollow/dashed when online-only,
 *                         absent when unknown)
 *   selvedge = PERMISSION stitched rim on the arc's outer edge where a hold begins
 * Bytes with unknown contents (not yet scanned) are hatched; unmeasured items get no angle.
 *
 * Public API
 *   const sb = createSunburst(canvas, { palette, basis, rings, onHover, onSelect, onFocus, format, reducedMotion })
 *   sb.setRoot(node); sb.focus(node); sb.up(); sb.setBasis('size'|'alloc'); sb.setPalette(p)
 *   sb.rotateBy(radians)  sb.resize()  sb.benchmark(frames)  sb.destroy()
 */
import { shade } from './woven-treemap.js';

const TAU = Math.PI * 2;
const MIN_SPAN = 0.0045; // radians: below this an arc is too fine to draw (the ring track shows through)
const ease = (t) => (t >= 1 ? 1 : 1 - Math.pow(2, -10 * t));

export function createSunburst(canvas, options = {}) {
  let ctx = canvas.getContext('2d');
  let palette = options.palette;
  let basis = options.basis || 'size';
  const RINGS = options.rings ?? 4;
  const fmt = options.format || String;
  const reduced = () => options.reducedMotion?.() ?? false;
  let dpr = 1, W = 0, H = 0, cx = 0, cy = 0, R = 0, r0 = 0, T = 0;
  let root = null, focusNode = null;
  let part = new Map(); // node -> [x0, x1]
  let v = { x0: 0, x1: 1, d: 0 }; // visible fraction window and focus depth (float while animating)
  let anim = null, theta = 0, spin = 0, raf = 0;
  let hover = null, cursor = 0, kbd = false;
  let drag = null;

  const sizeOf = (n) => (basis === 'alloc' ? n.alloc : n.size) || 0;

  function partition() {
    part = new Map();
    (function walk(n, x0, x1) {
      part.set(n, [x0, x1]);
      if (!n.children) return;
      const tot = n.children.reduce((s, c) => s + sizeOf(c), 0);
      if (!tot) return;
      let x = x0;
      const kids = n.children.filter((c) => sizeOf(c) > 0).sort((a, b) => sizeOf(b) - sizeOf(a));
      for (const c of kids) { const w = ((x1 - x0) * sizeOf(c)) / tot; walk(c, x, x + w); x += w; }
    })(root, 0, 1);
  }

  const ringR = (k) => r0 + k * T; // k rings out from the hole
  let rot = 0; // rotation used by ang(): 0 while building the cached cloth layer, theta for overlays
  const ang = (x) => rot - Math.PI / 2 + ((x - v.x0) / (v.x1 - v.x0)) * TAU;

  // ---------- render ----------
  function collect() {
    const arcs = [];
    (function walk(n) {
      const p = part.get(n);
      if (!p) return;
      const [x0, x1] = p;
      if (x1 <= v.x0 || x0 >= v.x1) return;
      const k = n.depth - v.d;
      if (k > RINGS + 0.001) return;
      const a0 = ang(Math.max(x0, v.x0)), a1 = ang(Math.min(x1, v.x1));
      if (a1 - a0 < MIN_SPAN) return;
      if (k > 0) {
        const ri = Math.max(r0, ringR(k - 1)), ro = ringR(k) - 1.5;
        if (ro > ri + 0.5) arcs.push({ n, a0, a1, ri, ro, k });
      }
      if (n.children) for (const c of n.children) walk(c);
    })(root);
    return arcs;
  }

  const arcPath = (p, a) => { p.moveTo(cx + a.ri * Math.cos(a.a0), cy + a.ri * Math.sin(a.a0)); p.arc(cx, cy, a.ro, a.a0, a.a1); p.arc(cx, cy, a.ri, a.a1, a.a0, true); p.closePath(); };

  // The cloth (fills, threads, selvedges) depends on the zoom window, not on the orbit angle, so it
  // is rendered once into a layer at rotation 0 and blitted rotated: orbiting costs one drawImage.
  let layer = null, benchMoving = false;
  function clothLayer() {
    // while a zoom is moving, the thread textures are skipped and settle in on the last frame
    const moving = !!anim || benchMoving;
    const key = `${v.x0}|${v.x1}|${v.d}|${W}x${H}@${dpr}|${palVersion}|${basis}|${moving}`;
    if (layer?.key === key) return layer;
    if (!layer) layer = { c: document.createElement('canvas') };
    const c = layer.c;
    if (c.width !== canvas.width || c.height !== canvas.height) { c.width = canvas.width; c.height = canvas.height; }
    const main = ctx;
    ctx = c.getContext('2d');
    ctx.setTransform(1, 0, 0, 1, 0, 0); ctx.clearRect(0, 0, c.width, c.height);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    rot = 0;
    paintCloth(collect(), !moving);
    ctx = main;
    layer.key = key;
    return layer;
  }

  function paintCloth(arcs, threads = true) {
    const byMeaning = new Map(), byRes = new Map(), all = new Path2D(), gaps = new Path2D(), prot = new Path2D(), pin = new Path2D(), unk = new Path2D();
    for (const a of arcs) {
      if (a.n.kind === 'gap') { arcPath(gaps, a); continue; }
      const m = a.n.meaning ?? 'none';
      if (!byMeaning.has(m)) byMeaning.set(m, new Path2D());
      arcPath(byMeaning.get(m), a);
      arcPath(all, a);
      const r = a.n.residency ?? 'unknown';
      if (r !== 'unknown') { if (!byRes.has(r)) byRes.set(r, new Path2D()); arcPath(byRes.get(r), a); }
      const parentPerm = a.n.parent ? a.n.parent.permission : 'none';
      if (a.n.permission !== parentPerm) {
        const p = a.n.permission === 'protected' ? prot : a.n.permission === 'pinned' ? pin : a.n.permission == null ? unk : null;
        if (p) { p.moveTo(cx + (a.ro - 1.5) * Math.cos(a.a0), cy + (a.ro - 1.5) * Math.sin(a.a0)); p.arc(cx, cy, a.ro - 1.5, a.a0, a.a1); }
      }
    }
    // warp: hue fill, luminous outward; then fine radial threads and concentric weft threads,
    // pre-rendered once per size and drawn through the arc clip
    const t = textures();
    for (const [m, path] of byMeaning) {
      const col = palette.meaning[m] || palette.meaning.none;
      const g = ctx.createRadialGradient(cx, cy, r0, cx, cy, R);
      g.addColorStop(0, shade(col, -0.35, 0.95)); g.addColorStop(1, shade(col, 0.08, 0.95));
      ctx.fillStyle = g; ctx.fill(path);
    }
    const stamp = (img, path) => {
      ctx.save(); ctx.clip(path); ctx.translate(cx, cy);
      ctx.drawImage(img, -t.half, -t.half, t.half * 2, t.half * 2); ctx.restore();
    };
    if (threads) {
      stamp(t.warp, all);
      for (const [r, path] of byRes) stamp(t.weft(r), path);
    }
    // unknown contents: hatched, no hue
    ctx.save(); ctx.clip(gaps);
    ctx.strokeStyle = palette.loose; ctx.lineWidth = 1; ctx.beginPath();
    for (let x = -R; x < R; x += 6) { ctx.moveTo(cx + x, cy - R); ctx.lineTo(cx + x + R, cy + R); }
    ctx.stroke(); ctx.restore();
    ctx.strokeStyle = palette.unknown; ctx.setLineDash([2, 3]); ctx.lineWidth = 1; ctx.stroke(gaps); ctx.setLineDash([]);
    // selvedge on the rim
    ctx.lineCap = 'butt';
    ctx.strokeStyle = palette.permission; ctx.lineWidth = 3; ctx.stroke(prot);
    ctx.strokeStyle = palette.ink; ctx.globalAlpha = 0.7; ctx.setLineDash([1.4, 2.6]); ctx.stroke(prot); ctx.globalAlpha = 1;
    ctx.strokeStyle = palette.permission; ctx.lineWidth = 1.6; ctx.setLineDash([6, 4]); ctx.stroke(pin);
    ctx.strokeStyle = palette.unknown; ctx.lineWidth = 1.5; ctx.setLineDash([1.5, 2.5]); ctx.stroke(unk);
    ctx.setLineDash([]);
    // the instrument's bezel: one faint glowing ring (cached here because blur is costly)
    ctx.save(); ctx.strokeStyle = palette.cursor; ctx.globalAlpha = 0.28; ctx.lineWidth = 1; ctx.shadowColor = palette.cursor; ctx.shadowBlur = 16;
    ctx.beginPath(); ctx.arc(cx, cy, ringR(RINGS) + 3, 0, TAU); ctx.stroke(); ctx.restore();
  }

  function draw() {
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.fillStyle = palette.ink; ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    // instrument rings: tracks show where fine threads are too small to draw
    ctx.lineWidth = 1;
    for (let k = 1; k <= RINGS; k++) { ctx.strokeStyle = palette.track; ctx.beginPath(); ctx.arc(cx, cy, ringR(k) - 1, 0, TAU); ctx.stroke(); }
    ctx.fillStyle = palette.trackFill;
    ctx.beginPath(); ctx.arc(cx, cy, ringR(RINGS), 0, TAU); ctx.arc(cx, cy, r0, 0, TAU, true); ctx.fill();

    const L = clothLayer();
    ctx.save(); ctx.translate(cx, cy); ctx.rotate(theta); ctx.drawImage(L.c, -cx, -cy, W, H); ctx.restore();
    rot = theta;
    const arcs = collect();

    drawScale();
    drawLabels(arcs);
    drawCentre();
    // hover / keyboard cursor glow
    const hi = [];
    if (hover) hi.push([hover, palette.hover]);
    if (kbd && focusNode) { const kids = cursorKids(); if (kids[cursor]) hi.push([kids[cursor], palette.cursor]); }
    for (const [n, col] of hi) {
      const a = arcs.find((x) => x.n === n);
      if (!a) continue;
      const p = new Path2D(); arcPath(p, a);
      ctx.save(); ctx.shadowColor = col; ctx.shadowBlur = 14; ctx.strokeStyle = col; ctx.lineWidth = 1.6; ctx.stroke(p); ctx.restore();
    }
    return arcs.length;
  }

  let tex = null, palVersion = 0;
  function textures() {
    const key = `${W}x${H}@${dpr}|${palVersion}`;
    if (tex?.key === key) return tex;
    const half = R + 2, S = Math.ceil(half * 2 * dpr);
    const mk = () => {
      const c = document.createElement('canvas'); c.width = c.height = S;
      const g = c.getContext('2d'); g.setTransform(dpr, 0, 0, dpr, S / 2, S / 2); return [c, g];
    };
    const [wc, wg] = mk();
    wg.strokeStyle = 'rgba(255,255,255,.12)'; wg.lineWidth = 0.8; wg.beginPath();
    for (let i = 0; i < 320; i++) { const a = (i / 320) * TAU; wg.moveTo(r0 * Math.cos(a), r0 * Math.sin(a)); wg.lineTo(R * Math.cos(a), R * Math.sin(a)); }
    wg.stroke();
    const wefts = new Map();
    tex = {
      key, half, warp: wc,
      weft(r) {
        if (wefts.has(r)) return wefts.get(r);
        const [c, g] = mk();
        g.strokeStyle = r === 'cloud' ? palette.residency.cloud : palette.residency[r];
        g.globalAlpha = 0.38; g.lineWidth = 0.9;
        if (r === 'cloud') g.setLineDash([2, 3]);
        g.beginPath();
        for (let rr = r0 + 2; rr < R; rr += 3.6) { g.moveTo(rr, 0); g.arc(0, 0, rr, 0, TAU); }
        g.stroke();
        wefts.set(r, c); return c;
      },
    };
    return tex;
  }

  function drawScale() {
    // outer instrument scale: 100 ticks = 1% of the focused node each, rotating with the orbit
    const ro = ringR(RINGS) + 6;
    ctx.strokeStyle = palette.scale; ctx.lineWidth = 1;
    ctx.beginPath();
    for (let i = 0; i < 100; i++) {
      const a = theta - Math.PI / 2 + (i / 100) * TAU, len = i % 25 === 0 ? 9 : i % 5 === 0 ? 5 : 2.5;
      ctx.moveTo(cx + ro * Math.cos(a), cy + ro * Math.sin(a)); ctx.lineTo(cx + (ro + len) * Math.cos(a), cy + (ro + len) * Math.sin(a));
    }
    ctx.stroke();
    ctx.font = palette.fontMonoSmall; ctx.fillStyle = palette.labelDim; ctx.textAlign = 'center'; ctx.textBaseline = 'middle';
    for (let i = 0; i < 4; i++) {
      const a = theta - Math.PI / 2 + (i / 4) * TAU;
      ctx.fillText(`${i * 25}%`, cx + (ro + 20) * Math.cos(a), cy + (ro + 20) * Math.sin(a));
    }
  }

  let widths = new Map(); // label widths per node, reset with the palette (fonts live there)
  function drawLabels(arcs) {
    ctx.textBaseline = 'middle';
    for (const a of arcs) {
      const span = a.a1 - a.a0, th = a.ro - a.ri, rm = (a.ri + a.ro) / 2, L = span * rm;
      if (L < 12 || th < 11) continue;
      const big = L > 60 && th > 14;
      ctx.font = palette.fontLabel;
      let text = a.n.name;
      const am = (a.a0 + a.a1) / 2;
      const norm = ((am % TAU) + TAU) % TAU;
      let tw = widths.get(a.n);
      if (tw === undefined) { tw = ctx.measureText(text).width; widths.set(a.n, tw); }
      if (big && L > tw + 14) {
        // tangential, upright
        ctx.save(); ctx.translate(cx + rm * Math.cos(am), cy + rm * Math.sin(am));
        const flip = norm > 0 && norm < Math.PI;
        ctx.rotate(am + (flip ? -Math.PI / 2 : Math.PI / 2));
        ctx.textAlign = 'center'; ctx.fillStyle = palette.label; ctx.fillText(text, 0, 0);
        ctx.restore();
      } else if (L > 15 && th > 26) {
        // radial
        while (tw > th - 8 && text.length > 3) { text = text.slice(0, -2) + '…'; tw = ctx.measureText(text).width; }
        if (tw > th - 8) continue;
        ctx.save(); ctx.translate(cx + rm * Math.cos(am), cy + rm * Math.sin(am));
        const flip = norm > Math.PI / 2 && norm < (3 * Math.PI) / 2;
        ctx.rotate(am + (flip ? Math.PI : 0));
        ctx.textAlign = 'center'; ctx.fillStyle = palette.label; ctx.fillText(text, 0, 0);
        ctx.restore();
      }
    }
  }

  function drawCentre() {
    const n = focusNode;
    if (!n) return;
    ctx.fillStyle = palette.hole; ctx.beginPath(); ctx.arc(cx, cy, r0 - 3, 0, TAU); ctx.fill();
    ctx.strokeStyle = palette.track; ctx.lineWidth = 1; ctx.stroke();
    ctx.textAlign = 'center'; ctx.textBaseline = 'middle';
    ctx.font = palette.fontCentre; ctx.fillStyle = palette.label;
    let name = n.kind === 'workspace' ? 'All volumes' : n.name;
    if (ctx.measureText(name).width > r0 * 1.6) ctx.font = palette.fontLabel; // step down before truncating
    while (ctx.measureText(name).width > r0 * 1.6 && name.length > 3) name = name.slice(0, -2) + '…';
    ctx.fillText(name, cx, cy - 9);
    ctx.font = palette.fontMono; ctx.fillStyle = palette.labelDim;
    ctx.fillText(fmt(sizeOf(n)), cx, cy + 10);
    if (n.parent) { ctx.font = palette.fontMonoSmall; ctx.fillText('centre: zoom out', cx, cy + 27); }
  }

  // ---------- loop ----------
  function frame(now) {
    raf = 0;
    let more = false;
    if (anim) {
      const t = Math.min(1, (now - anim.start) / anim.dur), e = ease(t);
      v = { x0: anim.from.x0 + (anim.to.x0 - anim.from.x0) * e, x1: anim.from.x1 + (anim.to.x1 - anim.from.x1) * e, d: anim.from.d + (anim.to.d - anim.from.d) * e };
      theta = anim.th0 + (anim.th1 - anim.th0) * e;
      if (t >= 1) anim = null; else more = true;
    }
    if (!drag && Math.abs(spin) > 0.0004 && !reduced()) { theta += spin; spin *= 0.94; more = true; } else if (!drag) spin = 0;
    draw();
    if (more) raf = requestAnimationFrame(frame);
  }
  const schedule = () => { if (!raf) raf = requestAnimationFrame(frame); };

  function focus(n) {
    if (!n || !n.children || !part.get(n) || !n.children.some((c) => sizeOf(c) > 0)) return;
    const [x0, x1] = part.get(n);
    const to = { x0, x1, d: n.depth };
    focusNode = n; cursor = 0; hover = null;
    if (reduced()) { v = to; theta = 0; anim = null; }
    else anim = { from: { ...v }, to, start: performance.now(), dur: 750, th0: theta, th1: Math.round(theta / TAU) * TAU };
    options.onFocus?.(n);
    schedule();
  }

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

  // ---------- picking ----------
  function pick(px, py) {
    const dx = px - cx, dy = py - cy, r = Math.hypot(dx, dy);
    if (r < r0) return { centre: true };
    const k = Math.ceil((r - r0) / T);
    if (k < 1 || k > RINGS) return null;
    let a = Math.atan2(dy, dx) - (theta - Math.PI / 2);
    a = ((a % TAU) + TAU) % TAU;
    const x = v.x0 + (a / TAU) * (v.x1 - v.x0);
    let n = focusNode;
    for (let i = 0; i < k; i++) {
      const c = n.children?.find((c) => { const p = part.get(c); return p && sizeOf(c) > 0 && x >= p[0] && x < p[1]; });
      if (!c) return null;
      n = c;
    }
    return { node: n };
  }
  const cursorKids = () => (focusNode?.children || []).filter((c) => sizeOf(c) > 0).sort((a, b) => part.get(a)[0] - part.get(b)[0]);
  const local = (e) => { const r = canvas.getBoundingClientRect(); return [e.clientX - r.left, e.clientY - r.top]; };

  canvas.addEventListener('pointerdown', (e) => {
    const [x, y] = local(e);
    drag = { x, y, a: Math.atan2(y - cy, x - cx), moved: false, t: performance.now() };
    spin = 0;
    canvas.setPointerCapture(e.pointerId);
  });
  canvas.addEventListener('pointermove', (e) => {
    const [x, y] = local(e);
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
    const n = p?.node || null;
    if (n !== hover) { hover = n; kbd = false; schedule(); }
    canvas.style.cursor = p?.centre && focusNode?.parent ? 'zoom-out' : n ? 'pointer' : 'grab';
    options.onHover?.(n, null, e);
  });
  canvas.addEventListener('pointerup', (e) => {
    const d = drag; drag = null; canvas.style.cursor = '';
    if (!d) return;
    if (d.moved) { schedule(); return; }
    const [x, y] = local(e);
    const p = pick(x, y);
    if (!p) return;
    if (p.centre) { if (focusNode?.parent) focus(focusNode.parent); return; }
    options.onSelect?.(p.node);
    if (p.node.children && p.node.children.length) focus(p.node);
  });
  canvas.addEventListener('pointerleave', () => { if (!drag) { hover = null; options.onHover?.(null); schedule(); } });
  canvas.addEventListener('keydown', (e) => {
    const kids = cursorKids();
    if (!kids.length) return;
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') { cursor = (cursor + 1) % kids.length; }
    else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') { cursor = (cursor - 1 + kids.length) % kids.length; }
    else if (e.key === ']') { theta += 0.12; }
    else if (e.key === '[') { theta -= 0.12; }
    else if (e.key === 'Enter' || e.key === ' ') { const n = kids[cursor]; options.onSelect?.(n); if (n.children?.length) focus(n); e.preventDefault(); kbd = true; return; }
    else if (e.key === 'Escape' || e.key === 'Backspace') { if (focusNode?.parent) focus(focusNode.parent); e.preventDefault(); kbd = true; return; }
    else return;
    e.preventDefault(); kbd = true;
    options.onHover?.(kids[cursor], null, null, true);
    schedule();
  });
  canvas.addEventListener('focus', () => { kbd = true; schedule(); });
  canvas.addEventListener('blur', () => { kbd = false; schedule(); });

  resize();

  return {
    setRoot(n) { root = n; partition(); focusNode = null; v = { x0: 0, x1: 1, d: n.depth }; focusNode = n; options.onFocus?.(n); schedule(); },
    focus, up() { if (focusNode?.parent) focus(focusNode.parent); },
    get focused() { return focusNode; },
    setBasis(b) { if (b === basis) return; basis = b; partition(); const p = part.get(focusNode); if (p) v = { x0: p[0], x1: p[1], d: focusNode.depth }; schedule(); },
    setPalette(p) { palette = p; palVersion++; widths = new Map(); schedule(); },
    rotateBy(a) { theta += a; schedule(); },
    /** Paint synchronously (for measuring inside a rAF loop). */
    renderFrame() { return draw(); },
    resize,
    /** opts.rebuild re-renders the cloth layer every frame (zoom animation); otherwise frames are orbit frames. */
    benchmark(frames = 30, opts = {}) {
      const ts = [];
      let arcs = 0;
      for (let i = 0; i < frames; i++) {
        benchMoving = !!opts.rebuild;
        if (opts.rebuild && layer) layer.key = null;
        theta += 0.01;
        const s = performance.now(); arcs = draw(); ctx.getImageData(0, 0, 1, 1); ts.push(performance.now() - s);
      }
      benchMoving = false; schedule();
      ts.sort((a, b) => a - b);
      return { mean: ts.reduce((a, b) => a + b, 0) / ts.length, p95: ts[Math.floor(ts.length * 0.95)], arcs };
    },
    destroy() { ro.disconnect(); cancelAnimationFrame(raf); },
  };
}
