/**
 * Names are untrusted text. Characters that change how a name looks without showing up (bidi
 * overrides, zero-width joiners, controls, look-alike spaces) are never put into the DOM: each one
 * becomes a visible `U+XXXX` badge, so a lookalike name cannot hide what it is.
 */

export interface NameSegment {
  text: string;
  /** Set for a hidden character: `U+202E`. `text` is then the badge label. */
  code?: string;
}

const KNOWN: Record<number, string> = {
  0x00a0: 'NBSP', 0x00ad: 'SHY', 0x061c: 'ALM', 0x200b: 'ZWSP', 0x200c: 'ZWNJ', 0x200d: 'ZWJ', 0x200e: 'LRM', 0x200f: 'RLM',
  0x2028: 'LS', 0x2029: 'PS', 0x202a: 'LRE', 0x202b: 'RLE', 0x202c: 'PDF', 0x202d: 'LRO', 0x202e: 'RLO', 0x2060: 'WJ',
  0x2066: 'LRI', 0x2067: 'RLI', 0x2068: 'FSI', 0x2069: 'PDI', 0xfeff: 'BOM', 0xfffd: 'REPLACEMENT',
};

// Control, format (includes every bidi and zero-width character and the tag block), separators, private
// use, lone surrogates, invisible fillers, every variation selector except an emoji or keycap
// presentation selector right after its base, and every space except the plain one.
const HIDDEN = /[\p{Cc}\p{Cf}\p{Cs}\p{Co}\p{Zl}\p{Zp}\u034f\u115f\u1160\u17b4\u17b5\u180b-\u180f\u3164\uffa0\u{e0100}-\u{e01ef}\ufffd]|[\ufe00-\ufe0d]|(?<![\p{Extended_Pictographic}0-9#*])[\ufe0e\ufe0f]|(?!\u0020)\p{Zs}/gu;

const hex = (cp: number) => `U+${cp.toString(16).toUpperCase().padStart(4, '0')}`;

/** Splits a name into plain runs and hidden-character badges. Joining the plain runs gives the visible text. */
export function nameSegments(name: string): NameSegment[] {
  const out: NameSegment[] = [];
  let last = 0;
  for (const m of name.matchAll(HIDDEN)) {
    const at = m.index;
    if (at > last) out.push({ text: name.slice(last, at) });
    const cp = m[0].codePointAt(0)!;
    out.push({ text: KNOWN[cp] ? `${hex(cp)} ${KNOWN[cp]}` : hex(cp), code: hex(cp) });
    last = at + m[0].length;
  }
  if (last < name.length) out.push({ text: name.slice(last) });
  return out;
}

export const hasHiddenCharacters = (name: string): boolean => nameSegments(name).some((s) => s.code);

/** The escaped form for places that cannot show badges (aria-labels, titles): hidden characters spelled out. */
export const escapedName = (name: string): string => nameSegments(name).map((s) => s.text).join('');
