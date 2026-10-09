/**
 * Resource instruments: arc gauges, sparklines and an honest memory ledger.
 * Framework-agnostic; colours come from CSS custom properties on the host elements
 * (--gauge-track, --gauge-value, --gauge-glow, --spark-line, --spark-fill, --mem-*).
 *
 * The memory ledger shows what Windows reports, and nothing that implies a cleaner:
 *   in use (working sets + kernel) | modified | standby (cache) | free,
 *   available = standby + free, and commit charge against the commit limit as a separate bar.
 *
 * Public API
 *   createGauge(host, { label, unit, max, format })   -> { update(value, caption) }
 *   createSparkline(canvas, { max, length })          -> { push(value), redraw() }
 *   createMemoryLedger(host, { format })              -> { update(ram) }
 */

const NS = 'http://www.w3.org/2000/svg';
const el = (tag, attrs = {}, parent) => {
  const e = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  parent?.appendChild(e);
  return e;
};

/** 240° arc gauge with a glowing value stroke and a monospace readout. */
export function createGauge(host, { label, unit = '%', max = 100, format = (v) => v.toFixed(0) } = {}) {
  host.classList.add('gauge');
  const svg = el('svg', { viewBox: '0 0 120 104', role: 'img', 'aria-label': label }, host);
  const r = 46, cx = 60, cy = 58, sweep = (240 / 360) * Math.PI * 2, start = Math.PI * (150 / 180);
  const pt = (a, rr = r) => [cx + rr * Math.cos(a), cy + rr * Math.sin(a)];
  const [sx, sy] = pt(start), [ex, ey] = pt(start + sweep);
  const d = `M ${sx} ${sy} A ${r} ${r} 0 1 1 ${ex} ${ey}`;
  const len = r * sweep;
  // ticks
  const ticks = el('g', { class: 'gauge-ticks' }, svg);
  for (let i = 0; i <= 20; i++) {
    const a = start + (sweep * i) / 20, [x1, y1] = pt(a, r + 6), [x2, y2] = pt(a, r + (i % 5 ? 8.5 : 11));
    el('line', { x1, y1, x2, y2 }, ticks);
  }
  el('path', { d, class: 'gauge-track', pathLength: len }, svg);
  const val = el('path', { d, class: 'gauge-value', 'stroke-dasharray': `${len} ${len}`, 'stroke-dashoffset': len }, svg);
  const num = el('text', { x: cx, y: cy + 4, class: 'gauge-num', 'text-anchor': 'middle' }, svg);
  const unitEl = el('text', { x: cx, y: cy + 17, class: 'gauge-unit', 'text-anchor': 'middle' }, svg);
  unitEl.textContent = unit;
  const cap = document.createElement('div'); cap.className = 'gauge-caption'; host.appendChild(cap);
  const name = document.createElement('div'); name.className = 'gauge-label'; name.textContent = label; host.prepend(name);
  return {
    update(v, caption) {
      const f = Math.max(0, Math.min(1, v / max));
      val.setAttribute('stroke-dashoffset', String(len * (1 - f)));
      host.style.setProperty('--level', f.toFixed(3));
      num.textContent = format(v);
      if (caption != null) cap.textContent = caption;
      svg.setAttribute('aria-label', `${label} ${format(v)} ${unit}`);
    },
  };
}

/** Sparkline on canvas: rolling window, soft fill, latest point marked. */
export function createSparkline(canvas, { max = 100, length = 90, autoMax = false } = {}) {
  const data = [];
  const ctx = canvas.getContext('2d');
  function redraw() {
    const dpr = Math.min(window.devicePixelRatio || 1, 3);
    const r = canvas.getBoundingClientRect();
    const w = Math.max(1, Math.round(r.width)), h = Math.max(1, Math.round(r.height));
    if (canvas.width !== w * dpr || canvas.height !== h * dpr) { canvas.width = w * dpr; canvas.height = h * dpr; }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    if (data.length < 2) return;
    const cs = getComputedStyle(canvas);
    const line = cs.getPropertyValue('--spark-line').trim() || '#9cf';
    const fill = cs.getPropertyValue('--spark-fill').trim() || 'rgba(150,200,255,.15)';
    const m = autoMax ? Math.max(1, ...data) * 1.15 : max;
    const x = (i) => (i / (length - 1)) * w, y = (vv) => h - 2 - (Math.min(vv, m) / m) * (h - 4);
    const off = length - data.length;
    ctx.beginPath();
    data.forEach((vv, i) => (i ? ctx.lineTo(x(i + off), y(vv)) : ctx.moveTo(x(i + off), y(vv))));
    ctx.lineWidth = 1.4; ctx.strokeStyle = line; ctx.lineJoin = 'round'; ctx.stroke();
    ctx.lineTo(x(length - 1), h); ctx.lineTo(x(off), h); ctx.closePath();
    const g = ctx.createLinearGradient(0, 0, 0, h); g.addColorStop(0, fill); g.addColorStop(1, 'rgba(0,0,0,0)');
    ctx.fillStyle = g; ctx.fill();
    const lx = x(length - 1), ly = y(data[data.length - 1]);
    ctx.fillStyle = line; ctx.beginPath(); ctx.arc(lx - 1.5, ly, 2.2, 0, Math.PI * 2); ctx.fill();
  }
  return { push(vv) { data.push(vv); if (data.length > length) data.shift(); redraw(); }, redraw };
}

/** The memory ledger: stacked physical memory plus a separate commit bar. */
export function createMemoryLedger(host, { format = (n) => String(n) } = {}) {
  host.classList.add('mem');
  host.innerHTML = `
    <div class="mem-bar" role="img" aria-label="Physical memory composition">
      <span class="mem-seg mem-inuse"></span><span class="mem-seg mem-mod"></span><span class="mem-seg mem-standby"></span><span class="mem-seg mem-free"></span>
    </div>
    <dl class="mem-keys">
      <div><dt><i class="mem-dot mem-inuse"></i>In use</dt><dd data-k="inUse"></dd></div>
      <div><dt><i class="mem-dot mem-mod"></i>Modified</dt><dd data-k="modified"></dd></div>
      <div><dt><i class="mem-dot mem-standby"></i>Standby cache</dt><dd data-k="standby"></dd></div>
      <div><dt><i class="mem-dot mem-free"></i>Free</dt><dd data-k="free"></dd></div>
    </dl>
    <div class="mem-avail"><span>Available</span><b data-k="available"></b><small>= standby + free</small></div>
    <div class="mem-commit">
      <div class="mem-commit-head"><span>Commit charge</span><b data-k="commit"></b></div>
      <div class="mem-commit-bar"><span></span></div>
    </div>`;
  const segs = host.querySelectorAll('.mem-seg');
  const commitBar = host.querySelector('.mem-commit-bar span');
  const out = (k) => host.querySelector(`[data-k="${k}"]`);
  return {
    update(ram) {
      const parts = [ram.inUse, ram.modified, ram.standby, ram.free];
      parts.forEach((p, i) => { segs[i].style.flexGrow = String(Math.max(0, p / ram.total)); });
      for (const k of ['inUse', 'modified', 'standby', 'free', 'available']) out(k).textContent = format(ram[k]);
      out('commit').textContent = `${format(ram.commit)} of ${format(ram.commitLimit)}`;
      commitBar.style.transform = `scaleX(${(ram.commit / ram.commitLimit).toFixed(4)})`;
      host.querySelector('.mem-bar').setAttribute('aria-label', `In use ${format(ram.inUse)}, standby ${format(ram.standby)}, free ${format(ram.free)} of ${format(ram.total)}`);
    },
  };
}
