// The find-bar query grammar (docs/46 section 4.1, F1): free text in, structured filters out.
//
// Mirror of crates/loomward-protocol/src/query_grammar.rs. Both are held to
// fixtures/v3/query-grammar-parity.json, so change them together. Pure and total: every input
// parses, and anything the grammar cannot honour becomes an entry in `issues`. It reads no clock
// (relative times stay symbolic until `modifiedBounds` is given a `now`) and resolves no names
// (`in:` and `under:` stay names; the caller maps them to collection_id / under_node_id).

import type { SearchTerm, Timestamp } from '../contracts.gen';

export type QueryIssueCode =
  | 'unterminated_quote'
  | 'bad_value'
  | 'negated_filter'
  | 'too_many_terms'
  | 'term_too_long'
  | 'too_many_extensions';

/** Something the grammar could not honour; the offending text is kept (cut to 64 characters). */
export interface QueryIssue {
  code: QueryIssueCode;
  token: string;
}

export type AgoUnit = 'h' | 'd' | 'w' | 'mo' | 'y';

/** One side of a modified-time range, inclusive: a fixed instant or "this long before now". */
export type TimeBound = { at: Timestamp } | { ago: { n: number; unit: AgoUnit } };

export interface ParsedQuery {
  terms: SearchTerm[];
  extensions: string[];
  min_bytes: string | null;
  max_bytes: string | null;
  modified_after: TimeBound | null;
  modified_before: TimeBound | null;
  kind: 'any' | 'file' | 'dir';
  in_collection: string | null;
  under: string | null;
  issues: QueryIssue[];
}

const KEYS = ['ext', 'size', 'modified', 'kind', 'in', 'under'];
const SIZE_SHIFT = new Map<string, number>([['', 0], ['b', 0], ['kb', 10], ['mb', 20], ['gb', 30], ['tb', 40]]);
const MAX_TERMS = 8;
const MAX_TERM_CHARS = 64;
const MAX_EXTENSIONS = 16;
const MAX_EXTENSION_CHARS = 32;
const ISSUE_TOKEN_CHARS = 64;
/** 1601-01-01T00:00:00Z, the oldest instant a Windows file time can name. */
const EPOCH_1601_MS = -11_644_473_600_000;
const DAY_MS = 86_400_000;
const U64_MAX = (1n << 64n) - 1n;
const AGO_MS: Record<AgoUnit, number> = { h: 3_600_000, d: DAY_MS, w: 7 * DAY_MS, mo: 30 * DAY_MS, y: 365 * DAY_MS };

type Cmp = 'lt' | 'le' | 'gt' | 'ge' | 'eq';

// Separators are exactly these four, so Rust and JavaScript cannot disagree about Unicode space.
const isSep = (c: string) => c === ' ' || c === '\t' || c === '\n' || c === '\r';
const isDigit = (c: string) => c >= '0' && c <= '9';
const lowerAscii = (s: string) => s.replace(/[A-Z]/g, (c) => c.toLowerCase());
const count = (s: string) => Array.from(s).length;

function splitCmp(v: string): [Cmp, string] {
  for (const [p, c] of [['>=', 'ge'], ['<=', 'le'], ['>', 'gt'], ['<', 'lt'], ['=', 'eq']] as const) {
    if (v.startsWith(p)) return [c, v.slice(p.length)];
  }
  return ['eq', v];
}

/** From `from` to the closing quote (or the end): the text, the index after the quote, closed. */
function readQuoted(cs: string[], from: number): [string, number, boolean] {
  const n = cs.indexOf('"', from);
  return n < 0 ? [cs.slice(from).join(''), cs.length, false] : [cs.slice(from, n).join(''), n + 1, true];
}

export function parseQuery(input: string): ParsedQuery {
  const cs = Array.from(input);
  const q: ParsedQuery = {
    terms: [],
    extensions: [],
    min_bytes: null,
    max_bytes: null,
    modified_after: null,
    modified_before: null,
    kind: 'any',
    in_collection: null,
    under: null,
    issues: [],
  };
  const issue = (code: QueryIssueCode, token: string[]) =>
    void q.issues.push({ code, token: token.slice(0, ISSUE_TOKEN_CHARS).join('') });

  const term = (text: string, mode: SearchTerm['mode'], negated: boolean, token: string[]) => {
    if (text === '') return;
    if (count(text) > MAX_TERM_CHARS) return issue('term_too_long', token);
    if (q.terms.length === MAX_TERMS) return issue('too_many_terms', token);
    q.terms.push({ text, mode, negated });
  };

  const ext = (value: string, token: string[]): boolean => {
    const set: string[] = [];
    for (const part of value.split(';').filter((p) => p !== '')) {
      const e = lowerAscii(part.startsWith('.') ? part.slice(1) : part);
      if (e === '' || count(e) > MAX_EXTENSION_CHARS || /[\p{Cc}\p{White_Space}\\/:*?"<>|;]/u.test(e)) return false;
      set.push(e);
    }
    if (set.length === 0) return false;
    for (const e of set) {
      if (q.extensions.includes(e)) continue;
      if (q.extensions.length === MAX_EXTENSIONS) {
        issue('too_many_extensions', token);
        break;
      }
      q.extensions.push(e);
    }
    return true;
  };

  // `size:>100mb`. Bounds are inclusive on the wire, so `>` and `<` move by one byte.
  const size = (value: string): boolean => {
    const [cmp, rest] = splitCmp(value);
    let digits = 0;
    while (digits < rest.length && isDigit(rest[digits]!)) digits++;
    const num = rest.slice(0, digits);
    const shift = SIZE_SHIFT.get(lowerAscii(rest.slice(digits))); // a Map, so `constructor` or `__proto__` is an unknown unit, not a prototype key
    if (shift === undefined || num === '' || num.length > 20) return false;
    const v = BigInt(num) << BigInt(shift);
    if (v > U64_MAX) return false;
    let min: bigint | null = null;
    let max: bigint | null = null;
    if (cmp === 'gt') {
      if (v === U64_MAX) return false;
      min = v + 1n;
    } else if (cmp === 'ge') min = v;
    else if (cmp === 'lt') {
      if (v === 0n) return false;
      max = v - 1n;
    } else if (cmp === 'le') max = v;
    else min = max = v;
    if (min !== null) q.min_bytes = min.toString();
    if (max !== null) q.max_bytes = max.toString();
    return true;
  };

  // `modified:2025`, `modified:2025-03-14`, `modified:<30d`. Relative values compare the age:
  // `<30d` is newer than 30 days, `>30d` older. A later token replaces the same side.
  const modified = (value: string): boolean => {
    const [cmp, rest] = splitCmp(value);
    let digits = 0;
    while (digits < rest.length && isDigit(rest[digits]!)) digits++;
    const unit = lowerAscii(rest.slice(digits));
    if (unit === 'h' || unit === 'd' || unit === 'w' || unit === 'mo' || unit === 'y') {
      const n = Number(rest.slice(0, digits));
      if (digits === 0 || digits > 5 || n === 0) return false;
      const ago: TimeBound = { ago: { n, unit } };
      if (cmp === 'lt' || cmp === 'le') q.modified_after = ago;
      else if (cmp === 'gt' || cmp === 'ge') q.modified_before = ago;
      else return false;
      return true;
    }
    const span = dateSpan(rest);
    if (!span) return false;
    const [start, end] = span;
    const [after, before]: [number | null, number | null] =
      cmp === 'eq' ? [start, end] : cmp === 'gt' ? [end + 1, null] : cmp === 'ge' ? [start, null] : cmp === 'lt' ? [null, start - 1] : [null, end];
    const a = after === null ? null : formatMs(after);
    const b = before === null ? null : formatMs(before);
    if ((after !== null && a === null) || (before !== null && b === null)) return false;
    if (a !== null) q.modified_after = { at: a };
    if (b !== null) q.modified_before = { at: b };
    return true;
  };

  const filter = (key: string, value: string, token: string[]) => {
    let ok = false;
    if (key === 'ext') ok = ext(value, token);
    else if (key === 'size') ok = size(value);
    else if (key === 'modified') ok = modified(value);
    else if (key === 'kind') {
      const k = lowerAscii(value);
      if (k === 'file' || k === 'dir') {
        q.kind = k;
        ok = true;
      }
    } else if (value !== '') {
      if (key === 'in') q.in_collection = value;
      else q.under = value;
      ok = true;
    }
    if (!ok) issue('bad_value', token);
  };

  let i = 0;
  while (i < cs.length) {
    if (isSep(cs[i]!)) {
      i++;
      continue;
    }
    const start = i;
    const negated = cs[i] === '-' && i + 1 < cs.length && !isSep(cs[i + 1]!);
    if (negated) i++;
    if (cs[i] === '"') {
      const [text, next, closed] = readQuoted(cs, i + 1);
      i = next;
      if (!closed) issue('unterminated_quote', cs.slice(start, i));
      term(text, 'substring', negated, cs.slice(start, i));
      continue;
    }
    // A word runs to the next separator, unless it is `key:"quoted value"`.
    let end = i;
    while (end < cs.length && !isSep(cs[end]!) && cs[end] !== ':') end++;
    const key = lowerAscii(cs.slice(i, end).join(''));
    if (end < cs.length && cs[end] === ':' && KEYS.includes(key)) {
      let value: string;
      let next: number;
      if (cs[end + 1] === '"') {
        const [v, n, closed] = readQuoted(cs, end + 2);
        if (!closed) issue('unterminated_quote', cs.slice(start, n));
        [value, next] = [v, n];
      } else {
        next = end + 1;
        while (next < cs.length && !isSep(cs[next]!)) next++;
        value = cs.slice(end + 1, next).join('');
      }
      i = next;
      if (negated) issue('negated_filter', cs.slice(start, i));
      else filter(key, value, cs.slice(start, i));
      continue;
    }
    while (end < cs.length && !isSep(cs[end]!)) end++;
    term(cs.slice(i, end).join(''), 'token', negated, cs.slice(start, end));
    i = end;
  }
  return q;
}

/** The modified range as request timestamps, relative bounds taken from `nowMs` (Unix epoch
 * milliseconds). A relative bound older than 1601 is clamped to 1601. */
export function modifiedBounds(q: ParsedQuery, nowMs: number): { after: Timestamp | null; before: Timestamp | null } {
  const resolve = (b: TimeBound | null): Timestamp | null => {
    if (b === null) return null;
    if ('at' in b) return b.at;
    return formatMs(Math.max(nowMs - b.ago.n * AGO_MS[b.ago.unit], EPOCH_1601_MS));
  };
  return { after: resolve(q.modified_after), before: resolve(q.modified_before) };
}

function daysInMonth(y: number, m: number): number {
  if (m === 2) return y % 4 === 0 && (y % 100 !== 0 || y % 400 === 0) ? 29 : 28;
  return m === 4 || m === 6 || m === 9 || m === 11 ? 30 : 31;
}

/** Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm). */
function daysFromCivil(y0: number, m: number, d: number): number {
  const y = m <= 2 ? y0 - 1 : y0;
  const era = Math.floor(y / 400);
  const yoe = y - era * 400;
  const doy = Math.floor((153 * (m > 2 ? m - 3 : m + 9) + 2) / 5) + d - 1;
  const doe = yoe * 365 + Math.floor(yoe / 4) - Math.floor(yoe / 100) + doy;
  return era * 146_097 + doe - 719_468;
}

function civilFromDays(z0: number): [number, number, number] {
  const z = z0 + 719_468;
  const era = Math.floor(z / 146_097);
  const doe = z - era * 146_097;
  const yoe = Math.floor((doe - Math.floor(doe / 1460) + Math.floor(doe / 36_524) - Math.floor(doe / 146_096)) / 365);
  const doy = doe - (365 * yoe + Math.floor(yoe / 4) - Math.floor(yoe / 100));
  const mp = Math.floor((5 * doy + 2) / 153);
  const d = doy - Math.floor((153 * mp + 2) / 5) + 1;
  const m = mp < 10 ? mp + 3 : mp - 9;
  return [yoe + era * 400 + (m <= 2 ? 1 : 0), m, d];
}

/** `YYYY`, `YYYY-MM` or `YYYY-MM-DD` (UTC) as its inclusive millisecond span. */
function dateSpan(s: string): [number, number] | null {
  const g = /^([0-9]{4})(?:-([0-9]{2})(?:-([0-9]{2}))?)?$/.exec(s);
  if (!g) return null;
  const y = Number(g[1]);
  if (y < 1601 || y > 9999) return null;
  const m = g[2] === undefined ? 1 : Number(g[2]);
  const d = g[3] === undefined ? 1 : Number(g[3]);
  if (m < 1 || m > 12 || d < 1 || d > daysInMonth(y, m)) return null;
  const start = daysFromCivil(y, m, d) * DAY_MS;
  const days =
    g[2] === undefined ? daysFromCivil(y + 1, 1, 1) - daysFromCivil(y, 1, 1) : g[3] === undefined ? daysInMonth(y, m) : 1;
  return [start, start + days * DAY_MS - 1];
}

const pad = (n: number, w: number) => String(n).padStart(w, '0');

/** `YYYY-MM-DDTHH:MM:SS.mmmZ`, or null outside years 0..=9999. */
function formatMs(ms: number): Timestamp | null {
  const [y, m, d] = civilFromDays(Math.floor(ms / DAY_MS));
  if (y < 0 || y > 9999) return null;
  const t = ((ms % DAY_MS) + DAY_MS) % DAY_MS;
  const hh = Math.floor(t / 3_600_000);
  const mi = Math.floor(t / 60_000) % 60;
  const ss = Math.floor(t / 1000) % 60;
  return `${pad(y, 4)}-${pad(m, 2)}-${pad(d, 2)}T${pad(hh, 2)}:${pad(mi, 2)}:${pad(ss, 2)}.${pad(t % 1000, 3)}Z`;
}
