/**
 * Thread colour assignment. Pure: no DOM, no canvas.
 *
 * Three channels, three physical elements of a weave, never mixed into one fill:
 *   warp     (vertical threads)   = meaning:    a dye per label; suggestion, mixture and "none" differ in pattern
 *   weft     (horizontal threads) = residency:  a metal per tier, hot bright to cold dark; missing when unknown
 *   selvedge (stitched edge)      = permission: a stitch per state, drawn only where the state begins
 * Each function returns one channel's own colour from its own palette slot, so no output is ever a
 * blend of two channels (tests/unit/viz-colour.test.ts holds this).
 */

/** @typedef {import('./types.js').Palette} Palette */
/** @typedef {import('./types.js').ThreadsLike} ThreadsLike */

/** @param {string} hex `#rgb` or `#rrggbb` @returns {[number, number, number]} */
export function hexRgb(hex) {
  const h = hex.trim().replace('#', '');
  const full = h.length === 3 ? h.split('').map((c) => c + c).join('') : h;
  if (!/^[0-9a-fA-F]{6}$/.test(full)) throw new Error(`not a hex colour: ${hex}`);
  const v = parseInt(full, 16);
  return [(v >> 16) & 255, (v >> 8) & 255, v & 255];
}

/**
 * Lighten (k > 0) or darken (k < 0) one colour toward white or black. This is shading within a
 * thread (its round highlight), never a mix with another channel's colour.
 * @param {string} hex @param {number} k in [-1, 1] @param {number} [alpha]
 */
export function shade(hex, k, alpha = 1) {
  const [r, g, b] = hexRgb(hex);
  /** @param {number} c */
  const f = (c) => Math.round(k >= 0 ? c + (255 - c) * k : c * (1 + k));
  return `rgba(${f(r)},${f(g)},${f(b)},${alpha})`;
}

/** FNV-1a over UTF-16 code units: a stable dye for a label, independent of slice order. @param {string} s */
export function labelHash(s) {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h;
}

/**
 * @typedef {'solid' | 'suggested' | 'mixed' | 'undyed' | 'pending' | 'hidden'} WarpPattern
 *   solid: a confirmed label; suggested: dyed threads alternate with undyed ones (a suggestion is not
 *   an approval); mixed: alternating dyes; undyed: no meaning; pending/unknown: undyed with breaks.
 * @typedef {{ colour: string, alt: string | null, pattern: WarpPattern }} Warp
 */

/** @param {ThreadsLike['meaning']} m @param {Palette} p @param {boolean} [show] @returns {Warp} */
export function warpOf(m, p, show = true) {
  if (!show) return { colour: p.neutral, alt: null, pattern: 'hidden' };
  const dye = (/** @type {string} */ label) => p.dyes[labelHash(label) % p.dyes.length] ?? p.undyed;
  switch (m.state) {
    case 'labelled': return m.label ? { colour: dye(m.label), alt: null, pattern: 'solid' } : { colour: p.undyed, alt: null, pattern: 'undyed' };
    case 'suggested': return m.label ? { colour: dye(m.label), alt: p.undyed, pattern: 'suggested' } : { colour: p.undyed, alt: null, pattern: 'pending' };
    case 'mixed': {
      const a = m.label ? labelHash(m.label) % p.dyes.length : 0;
      return { colour: p.dyes[a] ?? p.undyed, alt: p.dyes[(a + 3) % p.dyes.length] ?? p.undyed, pattern: 'mixed' };
    }
    case 'pending':
    case 'unknown': return { colour: p.undyed, alt: null, pattern: 'pending' };
    default: return { colour: p.undyed, alt: null, pattern: 'undyed' };
  }
}

/**
 * @typedef {'metal' | 'hollow' | 'missing' | 'hidden'} WeftPattern
 *   metal: tier known; hollow: a cloud placeholder (logical bytes, nothing resident);
 *   missing: residency unknown, so no weft is drawn at all; hidden: the channel is switched off.
 * @typedef {{ colour: string | null, pattern: WeftPattern, tier: number | null }} Weft
 */

/** @param {ThreadsLike} t @param {Palette} p @param {boolean} [show] @returns {Weft} */
export function weftOf(t, p, show = true) {
  if (!show) return { colour: p.neutral, pattern: 'hidden', tier: null };
  if (t.permission.reason === 'cloud_placeholder') return { colour: p.hollow, pattern: 'hollow', tier: t.residency.tier };
  const tier = t.residency.tier;
  if (tier === null || t.residency.tier_basis === 'unknown') return { colour: null, pattern: 'missing', tier: null };
  return { colour: p.metals[Math.min(Math.max(tier, 0), 2)], pattern: 'metal', tier };
}

/**
 * @typedef {'band' | 'running' | 'cross' | 'double' | 'dotted'} Stitch
 *   band: denied (a stitched band); running: partial; cross: excluded; double: revoked; dotted: unknown.
 * @typedef {{ stitch: Stitch, colour: string }} Selvedge
 */

/** @type {Record<string, Stitch>} */
const STITCH = { denied: 'band', partial: 'running', excluded: 'cross', revoked: 'double', unknown: 'dotted' };

/**
 * The edge for a node, or null when there is nothing to stitch: the state is "granted", the
 * channel is off, or the node only inherits its parent's state (the stitch marks where it begins).
 * @param {ThreadsLike['permission']} perm @param {ThreadsLike['permission'] | null} parent
 * @param {Palette} p @param {boolean} [show] @returns {Selvedge | null}
 */
export function selvedgeOf(perm, parent, p, show = true) {
  if (!show || perm.state === 'granted') return null;
  if (parent && parent.state === perm.state) return null;
  const stitch = STITCH[perm.state];
  if (!stitch) return null;
  return { stitch, colour: perm.state === 'unknown' ? p.unknown : p.permission };
}

/**
 * Colours that the palette promises never to share between channels. Returns the collisions;
 * an empty list means warp, weft and selvedge are distinguishable.
 * @param {Palette} p @returns {string[]}
 */
export function paletteCollisions(p) {
  /** @type {Map<string, string>} */
  const owner = new Map();
  const out = /** @type {string[]} */ ([]);
  /** @param {string} c @param {string} ch */
  const claim = (c, ch) => {
    const k = c.trim().toLowerCase();
    const prev = owner.get(k);
    if (prev && prev !== ch) out.push(`${k} is used by ${prev} and ${ch}`);
    else owner.set(k, ch);
  };
  for (const d of [...p.dyes, p.undyed]) claim(d, 'warp');
  for (const m of [...p.metals, p.hollow]) claim(m, 'weft');
  claim(p.permission, 'selvedge');
  return out;
}

/** Batch key for one woven fill. @param {Warp} w @param {Weft} f @param {boolean} loose */
export const weaveKey = (w, f, loose) => `${w.pattern}|${w.colour}|${w.alt}|${f.pattern}|${f.colour}|${loose ? 1 : 0}`;
