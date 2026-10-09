/**
 * Instrument gauges on Canvas 2D. Contract: docs/41 section 13.
 *
 *   const g = createGauge(canvas, { kind: 'arc' | 'spark', unit, min, max, label })
 *   g.push(value | null)   null is drawn as "unknown" (a dashed track, no needle, no zero)
 *   g.setTheme(theme)  g.resize()  g.destroy()
 *
 * 'arc': a 240° dial with a soft glow on the value stroke and a numeric readout.
 * 'spark': a rolling line over the last `length` samples; gaps where a sample was unknown.
 */

/**
 * @typedef {object} GaugeTheme
 * @property {string} track @property {string} value @property {string} glow
 * @property {string} text @property {string} dim @property {string} unknown
 * @property {string} fontNum @property {string} fontSmall
 */

/**
 * @typedef {object} GaugeOptions
 * @property {'arc' | 'spark'} kind
 * @property {string} unit
 * @property {number} [min]
 * @property {number} [max]      for 'spark', omitted = auto-scale to the window
 * @property {number} [length]   samples kept by a sparkline (default 90)
 * @property {GaugeTheme} theme
 * @property {(v: number) => string} [format]
 */

/** @param {HTMLCanvasElement} canvas @param {GaugeOptions} o */
export function createGauge(canvas, o) {
  const g0 = canvas.getContext('2d');
  if (!g0) throw new Error('2d canvas unavailable');
  const g = g0;
  let theme = o.theme;
  const min = o.min ?? 0;
  const length = o.length ?? 90;
  const format = o.format ?? ((/** @type {number} */ v) => (Math.abs(v) >= 100 ? v.toFixed(0) : v.toFixed(1)));
  /** @type {(number | null)[]} */
  const data = [];
  let dpr = 1, W = 0, H = 0;

  function resize() {
    const r = canvas.getBoundingClientRect();
    dpr = Math.min(window.devicePixelRatio || 1, 3);
    W = Math.max(1, Math.round(r.width)); H = Math.max(1, Math.round(r.height));
    canvas.width = Math.round(W * dpr); canvas.height = Math.round(H * dpr);
    draw();
  }

  function drawArc() {
    const v = data.length ? data[data.length - 1] : null;
    const max = o.max ?? 100;
    const cx = W / 2, cy = H * 0.56, r = Math.max(10, Math.min(W / 2, H * 0.56) - 10);
    const start = Math.PI * (150 / 180), sweep = (240 / 360) * Math.PI * 2;
    g.lineCap = 'round';
    // ticks
    g.strokeStyle = theme.track; g.lineWidth = 1; g.beginPath();
    for (let i = 0; i <= 20; i++) {
      const a = start + (sweep * i) / 20, l = i % 5 ? 3 : 6;
      g.moveTo(cx + (r + 5) * Math.cos(a), cy + (r + 5) * Math.sin(a)); g.lineTo(cx + (r + 5 + l) * Math.cos(a), cy + (r + 5 + l) * Math.sin(a));
    }
    g.stroke();
    g.lineWidth = 6; g.strokeStyle = theme.track;
    if (v === null) g.setLineDash([3, 5]);
    g.beginPath(); g.arc(cx, cy, r, start, start + sweep); g.stroke();
    g.setLineDash([]);
    g.textAlign = 'center'; g.textBaseline = 'middle';
    if (v === null) {
      g.font = theme.fontSmall; g.fillStyle = theme.unknown; g.fillText('unknown', cx, cy);
      return;
    }
    const f = Math.max(0, Math.min(1, (v - min) / (max - min)));
    g.save();
    g.shadowColor = theme.glow; g.shadowBlur = 4 + f * 10;
    g.strokeStyle = theme.value; g.lineWidth = 6;
    g.beginPath(); g.arc(cx, cy, r, start, start + sweep * Math.max(f, 0.002)); g.stroke();
    g.restore();
    g.font = theme.fontNum; g.fillStyle = theme.text; g.fillText(format(v), cx, cy - 2);
    g.font = theme.fontSmall; g.fillStyle = theme.dim; g.fillText(o.unit, cx, cy + 15);
  }

  function drawSpark() {
    const known = /** @type {number[]} */ (data.filter((d) => d !== null));
    const max = o.max ?? Math.max(1, ...known) * 1.15;
    /** @param {number} i */
    const x = (i) => (i / (length - 1)) * W;
    /** @param {number} v */
    const y = (v) => H - 2 - (Math.min(v, max) - min) / (max - min) * (H - 4);
    const off = length - data.length;
    g.lineWidth = 1.4; g.strokeStyle = theme.value; g.lineJoin = 'round';
    g.beginPath();
    let pen = false;
    data.forEach((v, i) => {
      if (v === null) { pen = false; return; }
      if (pen) g.lineTo(x(i + off), y(v)); else g.moveTo(x(i + off), y(v));
      pen = true;
    });
    g.stroke();
    const last = data[data.length - 1];
    if (last !== null && last !== undefined) {
      g.fillStyle = theme.value; g.beginPath(); g.arc(x(length - 1) - 1.5, y(last), 2.2, 0, Math.PI * 2); g.fill();
    } else if (data.length) {
      g.font = theme.fontSmall; g.fillStyle = theme.unknown; g.textAlign = 'right'; g.textBaseline = 'middle'; g.fillText('unknown', W - 2, H / 2);
    }
  }

  function draw() {
    g.setTransform(1, 0, 0, 1, 0, 0);
    g.clearRect(0, 0, canvas.width, canvas.height);
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    if (o.kind === 'arc') drawArc(); else drawSpark();
  }

  const ro = new ResizeObserver(resize);
  ro.observe(canvas);
  resize();

  return {
    /** @param {number | null} v */
    push(v) { data.push(v === null || !Number.isFinite(v) ? null : v); if (data.length > length) data.shift(); draw(); },
    /** @param {GaugeTheme} t */
    setTheme(t) { theme = t; draw(); },
    resize,
    destroy() { ro.disconnect(); },
  };
}
