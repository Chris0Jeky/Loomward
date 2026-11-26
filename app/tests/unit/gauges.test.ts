import { afterEach, describe, expect, it, vi } from 'vitest';
import { createGauge } from '../../viz/gauges.js';

type ArcCall = { x: number; y: number; r: number; start: number; end: number };

function makeRecordingContext() {
  const arcs: ArcCall[] = [];
  const texts: string[] = [];
  const ctx = {
    arcs,
    texts,
    strokeStyle: '',
    fillStyle: '',
    font: '',
    textAlign: '',
    textBaseline: '',
    lineCap: '',
    lineJoin: '',
    lineWidth: 0,
    shadowColor: '',
    shadowBlur: 0,
    arc(x: number, y: number, r: number, start: number, end: number) {
      arcs.push({ x, y, r, start, end });
    },
    fillText(text: string) {
      texts.push(String(text));
    },
    beginPath() {},
    moveTo() {},
    lineTo() {},
    stroke() {},
    save() {},
    restore() {},
    setLineDash() {},
    clearRect() {},
    setTransform() {},
  };
  return ctx;
}

type Ctx = ReturnType<typeof makeRecordingContext>;

function makeCanvas(ctx: Ctx) {
  return {
    width: 0,
    height: 0,
    getContext: (kind: string) => (kind === '2d' ? ctx : null),
    getBoundingClientRect: () => ({ width: 120, height: 120 }),
  };
}

const START = Math.PI * (150 / 180);
const SWEEP = (240 / 360) * Math.PI * 2;

describe('createGauge arc', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('clamps the arc fraction, maps the readout through format(v), and draws unknown for NaN', () => {
    vi.stubGlobal('window', { devicePixelRatio: 1 });
    vi.stubGlobal(
      'ResizeObserver',
      class {
        cb: () => void;
        constructor(cb: () => void) {
          this.cb = cb;
        }
        observe() {
          this.cb();
        }
        disconnect() {}
      },
    );

    const ctx = makeRecordingContext();
    const canvas = makeCanvas(ctx);
    const theme = {
      track: '#222',
      value: '#0f0',
      glow: '#0f0',
      text: '#fff',
      dim: '#888',
      unknown: '#888',
      fontNum: '12px sans-serif',
      fontSmall: '10px sans-serif',
    };
    const opts = {
      kind: 'arc' as const,
      unit: 'GB',
      label: 'disk',
      min: 0,
      max: 100,
      theme,
      format: (v: number) => `sample ${v}`,
    };
    const gauge = createGauge(canvas as unknown as HTMLCanvasElement, opts);

    const pushAndRead = (v: number) => {
      ctx.arcs.length = 0;
      ctx.texts.length = 0;
      gauge.push(v);
      return { arcs: [...ctx.arcs], texts: [...ctx.texts] };
    };

    // v = 0: fraction clamped at 0 (plus the 0.002 minimum sliver so the
    // stroke stays visible), readout is format(0).
    {
      const { arcs, texts } = pushAndRead(0);
      expect(arcs).toHaveLength(2);
      expect(arcs[0]!.start).toBeCloseTo(START, 5);
      expect(arcs[0]!.end - arcs[0]!.start).toBeCloseTo(SWEEP, 5);
      expect((arcs[1]!.end - arcs[1]!.start) / SWEEP).toBeCloseTo(0, 2);
      expect(texts[0]).toBe('sample 0');
    }

    // v = 50: half sweep, readout is format(50).
    {
      const { arcs, texts } = pushAndRead(50);
      expect(arcs).toHaveLength(2);
      expect((arcs[1]!.end - arcs[1]!.start) / SWEEP).toBeCloseTo(0.5, 5);
      expect(texts[0]).toBe('sample 50');
    }

    // v = 100: full sweep, readout is format(100).
    {
      const { arcs, texts } = pushAndRead(100);
      expect(arcs).toHaveLength(2);
      expect((arcs[1]!.end - arcs[1]!.start) / SWEEP).toBeCloseTo(1, 5);
      expect(texts[0]).toBe('sample 100');
    }

    // v = 200: clamped to 1 (unclamped it would span twice the sweep),
    // readout still goes through format(v).
    {
      const { arcs, texts } = pushAndRead(200);
      expect(arcs).toHaveLength(2);
      expect((arcs[1]!.end - arcs[1]!.start) / SWEEP).toBeCloseTo(1, 5);
      expect(texts[0]).toBe('sample 200');
    }

    // NaN is non-finite, stored as null: dashed track only plus 'unknown'.
    {
      ctx.arcs.length = 0;
      ctx.texts.length = 0;
      gauge.push(NaN);
      expect(ctx.arcs).toHaveLength(1);
      expect(ctx.texts).toContain('unknown');
    }

    gauge.destroy();
  });
});
