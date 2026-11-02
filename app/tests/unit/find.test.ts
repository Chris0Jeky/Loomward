import { describe, expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { parseQuery } from '../../src/lib/search/grammar';
import { findHref, planFind, PRESETS, type FindControls } from '../../src/lib/search/find';

const NOW = Date.UTC(2026, 9, 30, 12, 0, 0); // 2026-10-30T12:00:00Z
const NONE: FindControls = { extension: '', kind: 'any', minBytes: '' };
const plan = (input: string, controls: Partial<FindControls> = {}) => planFind(input, { ...NONE, ...controls }, NOW);

describe('find bar: query to request', () => {
  it('maps terms, one extension, size, relative time and kind onto the v1 fields', () => {
    const p = plan('report ext:PDF size:>100mb modified:<30d kind:file');
    expect(p.request).toEqual({
      text: 'report', extension: 'pdf', min_bytes: String(100n * 2n ** 20n + 1n), max_bytes: null,
      modified_after: '2026-09-30T12:00:00.000Z', modified_before: null, kind: 'file',
    });
    expect(p.notices).toEqual(['Entries with no recorded modified time are left out by a date filter.']);
    expect(p.chips).toEqual(['Extension: .pdf', 'Size: more than 100 MiB', 'Modified in the last 30 days', 'Kind: files']);
    expect(plan('report ext:pdf').notices).toEqual([]);
  });

  it('resolves older-than against the injected now, and a fixed date to its period', () => {
    expect(plan('modified:>1y').request.modified_before).toBe('2025-10-30T12:00:00.000Z');
    const y = plan('modified:2025').request;
    expect([y.modified_after, y.modified_before]).toEqual(['2025-01-01T00:00:00.000Z', '2025-12-31T23:59:59.999Z']);
    expect(plan('modified:2025').chips).toEqual(['Modified from 2025-01-01 UTC', 'Modified until 2025-12-31 UTC']);
    expect(planFind('modified:>1y', NONE, NOW + 86_400_000).request.modified_before).toBe('2025-10-31T12:00:00.000Z');
  });

  it('explains contradictory size bounds without silently widening either one', () => {
    const p = plan('size:<1kb', { minBytes: '1048576' });
    expect(p.request).toMatchObject({ min_bytes: '1048576', max_bytes: '1023' });
    expect(p.blocked).toContain('minimum size exceeds the maximum');
    expect(p.notices).toContain(p.blocked);
    expect(plan('size:>=1023 size:<=1023').blocked).toBeNull();
    expect(plan('size:>2gb size:<1gb').blocked).not.toBeNull();
  });

  it('describes effective control filters and exact time units', () => {
    expect(plan('report', { extension: 'zip', kind: 'file', minBytes: '1048576' }).chips)
      .toEqual(['Extension: .zip', 'Size: at least 1 MiB', 'Kind: files']);
    expect(plan('modified:>1y').chips).toContain('Modified more than 1 year (365 days) ago');
    expect(plan('modified:<2mo').chips).toContain('Modified in the last 2 months (60 days)');
    expect(plan('modified:2025').chips.every(c => c.endsWith('UTC'))).toBe(true);
  });

  it('explains an unclosed filter quote without claiming it became a phrase', () => {
    const p = plan('ext:"pdf');
    expect(p.request.extension).toBe('pdf');
    expect(p.notices.join(' ')).toContain('is not closed');
    expect(p.notices.join(' ')).not.toContain('one quoted phrase');
  });

  it('keeps chip sizes exact', () => {
    expect(plan('size:>=4gb size:<=8gb').chips).toEqual(['Size: at least 4 GiB', 'Size: at most 8 GiB']);
    expect(plan('size:<1kb').chips).toEqual(['Size: less than 1 KiB']);
    expect(plan('size:>1500').chips).toEqual(['Size: at least 1501 B']);
  });

  it('shows what the request cannot carry as a notice and applies the rest', () => {
    const p = plan('ext:pdf;docx -tmp "two words" in:Tax under:Work size:>big');
    expect(p.request.extension).toBe('pdf');
    expect(p.request.text).toBe('');
    expect(p.notices).toEqual([
      '“size:>big” is not a valid filter, so it was not applied.',
      'Excluding “tmp” is not supported yet, so it was not applied.',
      'The phrase “two words” cannot be searched as one phrase yet, so it was not applied; type its words separately.',
      'Only one extension can be searched at a time, so “.docx” was not applied.',
      'in:Tax is not supported yet, so it was not applied.',
      'under:Work is not supported yet, so it was not applied.',
    ]);
  });

  it('surfaces every grammar issue code', () => {
    const many = Array.from({ length: 9 }, (_, i) => `w${i}`).join(' ');
    const cases: [string, string][] = [
      ['"abc', 'is not closed'], ['modified:soon', 'is not a valid filter'], ['-size:>1mb', 'cannot be negated'],
      [many, 'at most 8 words'], ['x'.repeat(65), 'at most 64 characters'],
      [`ext:${Array.from({ length: 17 }, (_, i) => `e${i}`).join(';')}`, 'more than 16 extensions'],
    ];
    for (const [input, text] of cases) {
      expect(parseQuery(input).issues.length, input).toBeGreaterThan(0);
      expect(plan(input).notices.join('\n'), input).toContain(text);
    }
  });

  it('sends a quoted word without a space as a word, and joins words with a space', () => {
    expect(plan('"tax" return').request.text).toBe('tax return');
  });

  it('lets a typed filter win over the controls and says so only when they differ', () => {
    const typed = plan('ext:pdf kind:dir size:>1mb', { extension: 'docx', kind: 'file', minBytes: '5' });
    expect(typed.request).toMatchObject({ extension: 'pdf', kind: 'dir', min_bytes: String(2n ** 20n + 1n) });
    expect(typed.notices).toEqual([
      'The Extension box (“docx”) is ignored because the query says ext:pdf.',
      'The Size box (at least 5 B) is ignored because the query sets its own minimum size.',
      'The Kind box is ignored because the query says kind:dir.',
    ]);
    expect(plan('ext:pdf kind:dir', { extension: '.PDF', kind: 'dir' }).notices).toEqual([]);
    const filled = plan('report', { extension: '.zip', kind: 'file', minBytes: '1048576' });
    expect(filled.request).toMatchObject({ text: 'report', extension: 'zip', kind: 'file', min_bytes: '1048576' });
    expect(filled.notices).toEqual([]);
  });

  it('is unconstrained only when nothing the engine can narrow by is left', () => {
    expect(plan('').unconstrained).toBe(true);
    expect(plan('in:Tax -tmp').unconstrained).toBe(true);
    expect(plan('kind:dir').unconstrained).toBe(false);
    expect(plan('', { extension: 'pdf' }).unconstrained).toBe(false);
  });

  it('spells hidden characters out instead of passing them into chips and notices', () => {
    const p = plan('ext:p‮df under:a​b');
    const text = [...p.chips, ...p.notices].join('\n');
    expect(text).toContain('U+202E RLO');
    expect(text).toContain('U+200B ZWSP');
    expect(text).not.toMatch(/[‮​]/);
  });
});

describe('cleanup searches', () => {
  it('parse with no issue and are carried whole by the request', () => {
    expect(PRESETS.length).toBeGreaterThanOrEqual(3);
    for (const { label, query } of PRESETS) {
      expect(parseQuery(query).issues, label).toEqual([]);
      const p = plan(query);
      expect(p.notices, label).toEqual(/modified:/.test(query) ? ['Entries with no recorded modified time are left out by a date filter.'] : []);
      expect(p.unconstrained, label).toBe(false);
      expect(p.chips.length, label).toBeGreaterThan(0);
    }
  });

  it('say what they claim: "untouched for a year" is older than a year', () => {
    const p = plan(PRESETS.find((x) => x.label.startsWith('Large and untouched'))!.query);
    expect(p.request.modified_before).toBe('2025-10-30T12:00:00.000Z');
    expect(p.request.modified_after).toBeNull();
  });
});

describe('find bar against the mock engine', () => {
  const c = new Client(createMockTransport({ now: () => new Date('2026-10-01T00:00:00Z') }));
  const run = async (input: string) => (await c.call('search.query', { root_id: null, ...plan(input).request, limit: 100, cursor: null })).result.hits;

  it('applies both size bounds and both UTC date bounds to real mock catalogue rows', async () => {
    const hits = await run('kind:file size:>=1mb size:<=1gb modified:2025');
    expect(hits.length).toBeGreaterThan(0);
    for (const { row } of hits) {
      expect(BigInt(row.logical_bytes!)).toBeGreaterThanOrEqual(1n << 20n);
      expect(BigInt(row.logical_bytes!)).toBeLessThanOrEqual(1n << 30n);
      expect(row.modified_at).not.toBeNull();
      expect(row.modified_at! >= '2025-01-01T00:00:00.000Z').toBe(true);
      expect(row.modified_at! <= '2025-12-31T23:59:59.999Z').toBe(true);
    }
  });

  it('serves a request built from the box, with no federated field in it', async () => {
    expect(Object.keys(plan('ext:txt').request).sort()).toEqual(['extension', 'kind', 'max_bytes', 'min_bytes', 'modified_after', 'modified_before', 'text']);
    const dirs = await run('kind:dir');
    expect(dirs.length).toBeGreaterThan(0);
    expect(new Set(dirs.map((h) => h.row.kind))).toEqual(new Set(['dir']));
    const txt = await run('ext:txt');
    expect(txt.length).toBeGreaterThan(0);
    expect(txt.every((h) => h.row.name.toLowerCase().endsWith('.txt'))).toBe(true);
  });
});

describe('findHref (Today links, #1068 ?q=)', () => {
  it('round-trips every preset through the Explorer route', () => {
    for (const { query } of PRESETS) {
      const href = findHref(query);
      expect(href.startsWith('#/explorer?q=')).toBe(true);
      expect(new URLSearchParams(href.split('?')[1]).get('q')).toBe(query);
      expect(href.split('?')).toHaveLength(2); // no raw '?' inside the query (the route splits on it)
    }
  });
});
