import { afterEach, describe, expect, it, vi } from 'vitest';
import { createSunburst } from '../../viz/sunburst.js';
import type { Palette, SliceLike } from '../../viz/types.js';

const RINGS = 4;

function makeCtx() {
  const arcs: number[] = [];
  const noop = () => {};
  return {
    arcs,
    arc: (_x: number, _y: number, r: number) => { arcs.push(r); },
    beginPath: noop,
    moveTo: noop,
    lineTo: noop,
    stroke: noop,
    fill: noop,
    fillRect: noop,
    clearRect: noop,
    save: noop,
    restore: noop,
    clip: noop,
    translate: noop,
    rotate: noop,
    drawImage: noop,
    fillText: noop,
    setTransform: noop,
    setLineDash: noop,
    measureText: () => ({ width: 10 }),
  };
}

const palette: Palette = {
  ink: '#101010', frame: '#1a1a1a', head: '#222222', scrim: '#000000', label: '#ffffff',
  labelDim: '#999999', fringe: '#333333', loose: '#444444', cursor: '#ff0000', hover: '#00ff00',
  shuttle: '#0000ff', track: '#555555', scale: '#666666',
  dyes: ['#e6194b', '#3cb44b'], undyed: '#808080', neutral: '#777777',
  metals: ['#ff8c00', '#ffd700', '#4682b4'], hollow: '#888888', permission: '#00ffff', unknown: '#123456',
  fontDisplay: '12px sans-serif', fontHead: '12px sans-serif', fontLabel: '11px sans-serif',
  fontNum: '11px sans-serif', fontSmall: '10px sans-serif',
};

// An anchor with no children lets the renderer paint the centre and ring tracks.
const slice: SliceLike = {
  anchor_node_id: 'root', basis: 'logical', complete: true, truncated: false,
  nodes: [{
    node_id: 'root', parent: null, kind: 'atlas', name: 'root', depth: 0,
    size_bytes: '0', size_unknown_files: 0, logical_bytes: '0', allocated_bytes: '0',
    files: 0, dirs: 0, child_count: 0, folded_count: null, coverage: 'complete', live: false,
    threads: {
      meaning: { state: 'none', label: null },
      residency: { volume_id: null, tier: null, tier_basis: 'unknown' },
      permission: { state: 'granted', reason: null },
    },
  }],
};

function drawAtSize(width: number, height: number): number[] {
  vi.stubGlobal('window', { devicePixelRatio: 1 });
  vi.stubGlobal('requestAnimationFrame', () => 0);
  vi.stubGlobal('cancelAnimationFrame', () => {});
  vi.stubGlobal('ResizeObserver', class {
    private cb: () => void;
    constructor(cb: () => void) { this.cb = cb; }
    observe() { this.cb(); }
    disconnect() {}
  });
  vi.stubGlobal('Path2D', class { moveTo() {} arc() {} closePath() {} });
  vi.stubGlobal('document', {
    createElement: () => ({ width: 0, height: 0, getContext: () => makeCtx() }),
  });

  const main = makeCtx();
  const bounds = { left: 0, top: 0, width: 1, height: 1 };
  const canvas = Object.assign(new EventTarget(), {
    width: 0,
    height: 0,
    style: {},
    getBoundingClientRect: () => bounds,
    getContext: () => main,
  });
  const sb = createSunburst(canvas as unknown as HTMLCanvasElement, { palette, rings: RINGS });
  try {
    sb.setSlice(slice);
    bounds.width = width;
    bounds.height = height;
    sb.resize();
    sb.renderFrame();
    return main.arcs;
  } finally {
    sb.destroy();
  }
}

afterEach(() => vi.unstubAllGlobals());

describe('sunburst resize geometry', () => {
  it.each([
    { width: 200, height: 200, centre: 31, tracks: [41, 49, 57, 65] },
    { width: 100, height: 100, centre: 31, tracks: [34.5, 36, 37.5, 39] },
    { width: 400, height: 500, centre: 36.84, tracks: [70.38, 101.92, 133.46, 165] },
  ])('paints the centre and ring tracks after resizing to $width by $height', ({ width, height, centre, tracks }) => {
    const arcs = drawAtSize(width, height);
    for (const radius of [centre, ...tracks]) {
      expect(arcs).toContainEqual(expect.closeTo(radius, 8));
    }
  });
});
