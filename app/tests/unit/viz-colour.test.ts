import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { labelHash, paletteCollisions, selvedgeOf, shade, warpOf, weftOf } from '../../viz/colour.js';
import type { Palette, ThreadsLike } from '../../viz/types.js';

// The palettes under test come from the real token file, so a token edit that makes two threads
// share a colour fails here.
const css = readFileSync(join(import.meta.dirname, '..', '..', 'src', 'styles', 'tokens.css'), 'utf8');
function block(selector: string): Record<string, string> {
  const start = css.indexOf(selector);
  const body = css.slice(css.indexOf('{', start) + 1, css.indexOf('}', start));
  return Object.fromEntries([...body.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map((m) => [m[1]!, m[2]!.trim()]));
}
function palette(selector: string): Palette {
  const t = { ...block(":root[data-theme='woven-atlas']"), ...block(selector) };
  const v = (k: string) => t[k] ?? (() => { throw new Error(`missing ${k}`); })();
  return {
    ink: v('--cloth-ink'), frame: v('--cloth-frame'), head: v('--cloth-head'), scrim: v('--cloth-scrim'), label: v('--cloth-label'),
    labelDim: v('--cloth-label-dim'), fringe: v('--cloth-fringe'), loose: v('--cloth-loose'), cursor: v('--cloth-cursor'), hover: v('--cloth-hover'),
    shuttle: v('--cloth-shuttle'), track: v('--cloth-track'), scale: v('--cloth-scale'),
    dyes: [1, 2, 3, 4, 5, 6, 7, 8].map((i) => v(`--dye-${i}`)), undyed: v('--undyed'), neutral: v('--neutral-thread'),
    metals: [v('--weft-hot'), v('--weft-warm'), v('--weft-cold')], hollow: v('--weft-hollow'), permission: v('--selvedge'), unknown: v('--unknown'),
    fontDisplay: '', fontHead: '', fontLabel: '', fontNum: '', fontSmall: '',
  };
}
const THEMES = { atlas: palette(":root[data-theme='woven-atlas']"), observatory: palette(":root[data-theme='observatory']") };

const threads = (m: Partial<ThreadsLike['meaning']>, tier: number | null, perm: ThreadsLike['permission']['state'], reason: string | null = null): ThreadsLike => ({
  meaning: { state: 'labelled', label: null, ...m },
  residency: { volume_id: tier === null ? null : 'v', tier, tier_basis: tier === null ? 'unknown' : 'declared' },
  permission: { state: perm, reason },
});

for (const [name, p] of Object.entries(THEMES)) {
  describe(`thread colours (${name})`, () => {
    it('the token palette gives warp, weft and selvedge disjoint colours', () => {
      expect(paletteCollisions(p)).toEqual([]);
    });

    it('never blends channels: each output comes from its own palette slot', () => {
      const warpSet = new Set([...p.dyes, p.undyed, p.neutral]);
      const weftSet = new Set([...p.metals, p.hollow, p.neutral]);
      const labels = ['Projects', 'Media', 'Invoices 2025', 'models', '<img src=x>', ''];
      const states = ['labelled', 'suggested', 'mixed', 'none', 'pending', 'unknown'] as const;
      const perms = ['granted', 'partial', 'excluded', 'denied', 'revoked', 'unknown'] as const;
      for (const label of labels) for (const state of states) for (const tier of [0, 1, 2, 5, null]) for (const perm of perms) {
        const t = threads({ state, label: label || null }, tier, perm);
        const w = warpOf(t.meaning, p);
        expect(warpSet.has(w.colour)).toBe(true);
        if (w.alt) expect(warpSet.has(w.alt)).toBe(true);
        const f = weftOf(t, p);
        if (f.colour) expect(weftSet.has(f.colour)).toBe(true);
        const s = selvedgeOf(t.permission, null, p);
        if (s) expect([p.permission, p.unknown]).toContain(s.colour);
      }
    });

    it('a dye is stable per label and independent of everything else', () => {
      const a = warpOf({ state: 'labelled', label: 'Projects' }, p).colour;
      expect(warpOf({ state: 'labelled', label: 'Projects' }, p).colour).toBe(a);
      expect(p.dyes[labelHash('Projects') % p.dyes.length]).toBe(a);
    });

    it('meaning states differ in pattern, not only colour', () => {
      const pat = (state: ThreadsLike['meaning']['state']) => warpOf({ state, label: 'Projects' }, p).pattern;
      expect(new Set([pat('labelled'), pat('suggested'), pat('mixed'), pat('none'), pat('pending')]).size).toBe(5);
      expect(warpOf({ state: 'labelled', label: 'x' }, p, false)).toEqual({ colour: p.neutral, alt: null, pattern: 'hidden' });
    });

    it('residency: hot is brighter than cold, unknown has no weft, placeholders are hollow', () => {
      const lum = (c: string) => { const m = /rgba\((\d+),(\d+),(\d+)/.exec(shade(c, 0))!; return 0.2126 * +m[1]! + 0.7152 * +m[2]! + 0.0722 * +m[3]!; };
      const [hot, warm, cold] = [0, 1, 2].map((t) => weftOf(threads({}, t, 'granted'), p).colour!);
      expect(lum(hot!)).toBeGreaterThan(lum(warm!));
      expect(lum(warm!)).toBeGreaterThan(lum(cold!));
      expect(weftOf(threads({}, null, 'granted'), p)).toEqual({ colour: null, pattern: 'missing', tier: null });
      expect(weftOf(threads({}, 0, 'granted', 'cloud_placeholder'), p).pattern).toBe('hollow');
      expect(weftOf(threads({}, 0, 'granted'), p, false).pattern).toBe('hidden');
    });

    it('permission: one stitch per state, only where the state begins', () => {
      const perms = ['partial', 'excluded', 'denied', 'revoked', 'unknown'] as const;
      const stitches = perms.map((s) => selvedgeOf({ state: s, reason: null }, null, p)!.stitch);
      expect(new Set(stitches).size).toBe(perms.length);
      expect(selvedgeOf({ state: 'granted', reason: null }, null, p)).toBeNull();
      expect(selvedgeOf({ state: 'denied', reason: null }, { state: 'denied', reason: null }, p)).toBeNull();
      expect(selvedgeOf({ state: 'denied', reason: null }, { state: 'granted', reason: null }, p)?.stitch).toBe('band');
      expect(selvedgeOf({ state: 'denied', reason: null }, null, p, false)).toBeNull();
    });
  });
}
