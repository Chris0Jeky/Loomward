import { describe, expect, it } from 'vitest';
import { escapedName, hasHiddenCharacters, nameSegments } from '../../src/lib/format/names';

const plain = (s: string) => nameSegments(s).filter((x) => !x.code).map((x) => x.text).join('');
const codes = (s: string) => nameSegments(s).flatMap((x) => (x.code ? [x.code] : []));

describe('visible names', () => {
  it('leaves ordinary and non-latin names alone', () => {
    for (const n of ['report 2026.docx', 'résumé.pdf', '日本語.txt', 'emoji 👍🏽.png', 'a\u0301b']) {
      expect(nameSegments(n)).toEqual([{ text: n }]);
    }
    expect(hasHiddenCharacters('plain.txt')).toBe(false);
  });

  it('exposes a right-to-left override and keeps it out of the plain text', () => {
    const n = 'invoice\u202Etxt.exe';
    expect(codes(n)).toEqual(['U+202E']);
    expect(nameSegments(n)[1]!.text).toBe('U+202E RLO');
    expect(plain(n)).toBe('invoicetxt.exe');
    expect(plain(n)).not.toMatch(/[\u202E]/);
  });

  it('exposes zero-width, bidi isolate, control and look-alike space characters', () => {
    expect(codes('a\u200Bb\u200Dc\u2060d')).toEqual(['U+200B', 'U+200D', 'U+2060']);
    expect(codes('x\u2066y\u2069')).toEqual(['U+2066', 'U+2069']);
    expect(codes('tab\there\u0000nul\u007f')).toEqual(['U+0009', 'U+0000', 'U+007F']);
    expect(codes('non\u00a0breaking\u3000wide')).toEqual(['U+00A0', 'U+3000']);
    expect(codes('tag\u{E0041}x')).toEqual(['U+E0041']);
    expect(codes('lone\ud800surrogate')).toEqual(['U+D800']);
    expect(codes('bom\ufeff')).toEqual(['U+FEFF']);
  });

  it('shows a plain space as a plain space', () => {
    expect(nameSegments('two words')).toEqual([{ text: 'two words' }]);
  });
});

describe('invisible characters that must not pass as plain text', () => {
  const cps = (s: string) => codes(s);
  it('badges every variation selector that is not an emoji presentation selector', () => {
    expect(cps('a' + String.fromCodePoint(0xfe00) + 'b' + String.fromCodePoint(0xfe0d))).toEqual(['U+FE00', 'U+FE0D']);
    expect(cps('a' + String.fromCodePoint(0xfe0f))).toEqual(['U+FE0F']); // not after an emoji base
    expect(cps('a' + String.fromCodePoint(0xfe0e))).toEqual(['U+FE0E']);
    expect(cps('x' + String.fromCodePoint(0xe0100) + String.fromCodePoint(0xe01ef))).toEqual(['U+E0100', 'U+E01EF']);
  });
  it('keeps emoji and keycap presentation selectors', () => {
    for (const n of ['I \u2764\uFE0F you', '1\uFE0F\u20E3.png', '\u2764\uFE0E.txt', '\u{1F44D}\uFE0F']) expect(codes(n)).toEqual([]);
  });
  it('badges Mongolian selectors, Hangul and other invisible fillers, invisible operators and the grapheme joiner', () => {
    const list = [0x180b, 0x180c, 0x180d, 0x180e, 0x180f, 0x115f, 0x1160, 0x3164, 0xffa0, 0x2060, 0x2061, 0x2062, 0x2063, 0x2064, 0x034f];
    for (const cp of list) expect(codes('a' + String.fromCodePoint(cp) + 'b'), cp.toString(16)).toEqual(['U+' + cp.toString(16).toUpperCase().padStart(4, '0')]);
  });
  it('gives an escaped form for accessible names', () => {
    expect(escapedName('invoice\u202Etxt.exe')).toBe('invoiceU+202E RLOtxt.exe');
    expect(escapedName('plain.txt')).toBe('plain.txt');
  });
});
