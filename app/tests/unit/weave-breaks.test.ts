import { afterEach, expect, it } from 'vitest';
import { weaveTile } from '../../viz/weave.js';

// Codex P2 on #357: a broken meaning crossing must paint neither its top nor its under strand.
class Recorder {
  fills: string[] = [];
  fillStyle: unknown = '';
  strokeStyle: unknown = '';
  lineWidth = 1;
  fillRect() { this.fills.push(typeof this.fillStyle === 'string' ? this.fillStyle : 'gradient'); }
  strokeRect() {}
  createLinearGradient() { return { addColorStop() {} }; }
}
let last: Recorder;
class FakeCanvas {
  ctx = new Recorder();
  constructor(public width: number, public height: number) { last = this.ctx; }
  getContext() { return this.ctx; }
}
const original = (globalThis as { OffscreenCanvas?: unknown }).OffscreenCanvas;
afterEach(() => { (globalThis as { OffscreenCanvas?: unknown }).OffscreenCanvas = original; });

it.each(['missing', 'hollow'] as const)('not_eligible paints no meaning strand over a %s residency weft', (pattern) => {
  (globalThis as { OffscreenCanvas?: unknown }).OffscreenCanvas = FakeCanvas;
  const ink = '#101010';
  weaveTile(1, { colour: '#888888', alt: null, pattern: 'not_eligible' }, { colour: pattern === 'hollow' ? '#445566' : null, pattern, tier: null }, { ink, unknown: '#777777' });
  // only the background fill remains: every crossing is broken, under strand included
  expect(last.fills).toEqual([ink]);
});
