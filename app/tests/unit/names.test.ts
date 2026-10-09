import { describe, expect, it } from 'vitest';
import { hasHiddenCharacters, nameSegments } from '../../src/lib/format/names';

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
