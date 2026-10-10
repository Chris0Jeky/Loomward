/**
 * Cloth rasterisation: weave tiles, fringe and loose-thread tiles, and selvedge stitches.
 * Canvas only (OffscreenCanvas where available); used by the treemap, the sunburst legend and swatches.
 *
 * The cloth is a 3/1 warp-faced twill (denim's structure): the warp floats over three wefts and
 * under one, so the meaning dye dominates and the residency weft shows as a diagonal of flecks.
 */
import { shade } from './colour.js';

/** @typedef {import('./colour.js').Warp} Warp */
/** @typedef {import('./colour.js').Weft} Weft */
/** @typedef {import('./colour.js').Stitch} Stitch */
/** @typedef {OffscreenCanvas | HTMLCanvasElement} AnyCanvas */

/** CSS px per thread. */
export const PITCH = 4;
const THREADS = 8;

/** @param {number} w @param {number} h @returns {AnyCanvas} */
export function makeCanvas(w, h) {
  if (typeof OffscreenCanvas !== 'undefined') return new OffscreenCanvas(w, h);
  return Object.assign(document.createElement('canvas'), { width: w, height: h });
}

/** @param {AnyCanvas} c @returns {CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D} */
export function ctx2d(c) {
  const g = c.getContext('2d');
  if (!g) throw new Error('2d canvas unavailable');
  return /** @type {CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D} */ (g);
}

/**
 * One repeat of cloth, `THREADS` threads square, rasterised at the device pixel ratio.
 * @param {number} dpr @param {Warp} warp @param {Weft} weft
 * @param {{ ink: string, unknown: string, loose?: boolean, only?: 'warp' | 'weft' }} o
 *   loose: a provisional (live-scan) sum: every other weft is left out, so the cloth reads unfinished
 */
export function weaveTile(dpr, warp, weft, o) {
  const p = PITCH * dpr, n = THREADS, size = p * n;
  const c = makeCanvas(size, size), g = ctx2d(c);
  /** @param {number} i @param {number} j */
  const warpOn = (i, j) => (((i + j) % 4) + 4) % 4 !== 0;
  /** @param {number} k @param {number} salt */
  const jit = (k, salt) => ((((k * 73856093) ^ (salt * 19349663)) >>> 0) % 13) / 13 * 0.14 - 0.07;
  g.fillStyle = o.ink;
  g.fillRect(0, 0, size, size);
  const solidWeft = (weft.pattern === 'metal' || weft.pattern === 'hidden') && weft.colour !== null;
  const showWeft = o.only !== 'warp';
  const showWarp = o.only !== 'weft';
  /** @param {number} i */
  const warpColour = (i) => {
    if (warp.pattern === 'suggested' && i % 2 === 1) return /** @type {string} */ (warp.alt);
    if (warp.pattern === 'mixed' && i % 2 === 1) return /** @type {string} */ (warp.alt);
    return warp.colour;
  };
  for (let i = 0; i < n; i++) {
    for (let j = 0; j < n; j++) {
      const x = i * p, y = j * p;
      const looseRow = o.loose && j % 2 === 1;
      const weftHere = showWeft && !looseRow && weft.pattern !== 'missing';
      const wTop = showWarp && (warpOn(i, j) || !weftHere);
      const wc = warpColour(i);
      // pending meaning: the warp breaks every fourth crossing (the label is not there yet)
      const warpBreak = warp.pattern === 'pending' && (i * 3 + j) % 8 === 0;
      if (weftHere && solidWeft && wTop) { g.fillStyle = shade(/** @type {string} */ (weft.colour), -0.62); g.fillRect(x, y, p, p); }
      if (showWarp && (!wTop || !solidWeft)) { g.fillStyle = shade(wc, -0.66); g.fillRect(x + p * 0.18, y, p * 0.64, p); }
      if (wTop && !warpBreak) {
        const k = jit(i, 1);
        const gr = g.createLinearGradient(x, 0, x + p, 0);
        gr.addColorStop(0, shade(wc, -0.55 + k));
        gr.addColorStop(0.32, shade(wc, 0.06 + k));
        gr.addColorStop(0.5, shade(wc, 0.2 + k));
        gr.addColorStop(0.72, shade(wc, -0.04 + k));
        gr.addColorStop(1, shade(wc, -0.58 + k));
        g.fillStyle = gr;
        g.fillRect(x + p * 0.1, y, p * 0.8, p);
        if (weftHere) {
          if (!warpOn(i, j - 1)) { const s = g.createLinearGradient(0, y, 0, y + p * 0.4); s.addColorStop(0, 'rgba(0,0,0,.55)'); s.addColorStop(1, 'rgba(0,0,0,0)'); g.fillStyle = s; g.fillRect(x + p * 0.1, y, p * 0.8, p * 0.4); }
          if (!warpOn(i, j + 1)) { const s = g.createLinearGradient(0, y + p, 0, y + p * 0.6); s.addColorStop(0, 'rgba(0,0,0,.55)'); s.addColorStop(1, 'rgba(0,0,0,0)'); g.fillStyle = s; g.fillRect(x + p * 0.1, y + p * 0.6, p * 0.8, p * 0.4); }
        }
      } else if (weftHere && solidWeft) {
        const col = /** @type {string} */ (weft.colour);
        const k = jit(j, 2);
        const gr = g.createLinearGradient(0, y, 0, y + p);
        gr.addColorStop(0, shade(col, -0.55 + k));
        gr.addColorStop(0.32, shade(col, 0.04 + k));
        gr.addColorStop(0.5, shade(col, 0.16 + k));
        gr.addColorStop(0.72, shade(col, -0.06 + k));
        gr.addColorStop(1, shade(col, -0.6 + k));
        g.fillStyle = gr;
        g.fillRect(x, y + p * 0.1, p, p * 0.8);
      } else if (weftHere && weft.pattern === 'hollow') {
        g.strokeStyle = shade(/** @type {string} */ (weft.colour), 0, 0.85);
        g.lineWidth = Math.max(1, dpr * 0.8);
        g.strokeRect(x + 0.5 * g.lineWidth, y + p * 0.18, p - g.lineWidth, p * 0.64);
      } else if (showWeft && weft.pattern === 'missing' && !wTop) {
        // residency unknown: no weft, a faint mark where it would cross
        g.fillStyle = shade(o.unknown, 0, o.only === 'weft' ? 0.9 : 0.5);
        g.fillRect(x + p * 0.35, y + p * 0.42, p * 0.3, p * 0.16);
      }
    }
  }
  return c;
}

/** Diagonal fringe: siblings folded into one "smaller" cell. @param {number} dpr @param {string} colour @param {string} ink */
export function fringeTile(dpr, colour, ink) {
  const s = 6 * dpr, c = makeCanvas(s, s), g = ctx2d(c);
  g.fillStyle = ink; g.fillRect(0, 0, s, s);
  g.strokeStyle = colour; g.lineWidth = dpr * 0.9;
  g.beginPath(); g.moveTo(0, s); g.lineTo(s, 0); g.moveTo(-s / 2, s / 2); g.lineTo(s / 2, -s / 2); g.moveTo(s / 2, s * 1.5); g.lineTo(s * 1.5, s / 2); g.stroke();
  return c;
}

/** Loose, unwoven warp: bytes counted by the parent but not listed in this slice. @param {number} dpr @param {string} colour @param {string} ink */
export function looseTile(dpr, colour, ink) {
  const w = 14 * dpr, h = 28 * dpr, c = makeCanvas(w, h), g = ctx2d(c);
  g.fillStyle = ink; g.fillRect(0, 0, w, h);
  g.strokeStyle = colour; g.lineWidth = dpr; g.lineCap = 'round';
  for (const [x0, y0, y1] of [[3, 2, 12], [3, 16, 25], [10, 7, 20]]) {
    g.beginPath(); g.moveTo(x0 * dpr, y0 * dpr);
    g.bezierCurveTo((x0 + 2) * dpr, (y0 + 3) * dpr, (x0 - 2) * dpr, (y1 - 3) * dpr, (x0 + 1) * dpr, y1 * dpr);
    g.stroke();
  }
  return c;
}

/** Cross-hatch for unknown allocation (hatched, never zero). @param {number} dpr @param {string} colour */
export function hatchTile(dpr, colour) {
  const s = 7 * dpr, c = makeCanvas(s, s), g = ctx2d(c);
  g.strokeStyle = colour; g.lineWidth = dpr;
  g.beginPath(); g.moveTo(0, 0); g.lineTo(s, s); g.moveTo(s, 0); g.lineTo(0, s); g.stroke();
  return c;
}

/**
 * Stroke a selvedge along `path` (rectangles or arcs) in canvas units where 1 unit = `lw` px.
 * The five stitches differ in structure, not only colour, so permission reads without colour.
 * @param {CanvasRenderingContext2D} g @param {Path2D} path @param {Stitch} stitch
 * @param {string} colour @param {string} ink @param {number} [lw]
 */
export function strokeStitch(g, path, stitch, colour, ink, lw = 1) {
  g.save();
  g.lineCap = 'butt';
  g.strokeStyle = colour;
  if (stitch === 'band') {
    g.lineWidth = 3 * lw; g.setLineDash([]); g.stroke(path);
    g.strokeStyle = ink; g.globalAlpha *= 0.7; g.setLineDash([1.4 * lw, 2.6 * lw]); g.stroke(path);
  } else if (stitch === 'running') {
    g.lineWidth = 1.6 * lw; g.setLineDash([6 * lw, 4 * lw]); g.stroke(path);
  } else if (stitch === 'cross') {
    g.lineWidth = 3 * lw; g.setLineDash([1.2 * lw, 1.8 * lw]); g.stroke(path);
  } else if (stitch === 'double') {
    g.lineWidth = 3.2 * lw; g.setLineDash([]); g.stroke(path);
    g.strokeStyle = ink; g.lineWidth = 1.2 * lw; g.stroke(path);
  } else {
    g.lineWidth = 1.5 * lw; g.setLineDash([1.5 * lw, 2.5 * lw]); g.stroke(path);
  }
  g.restore();
}
