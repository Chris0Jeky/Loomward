// The Explorer find bar (docs/46 section 4.1): box text in, a `search.query` request, filter chips and
// notices out. Pure; the clock arrives as `nowMs` so relative times are testable.
//
// What the request can carry. Every engine today answers the federated fields (`terms`, `extensions`,
// `collection_id`, `under_node_id`) with capability_unavailable until `features.federated_search`, so this
// maps onto the v1 fields only: `text`, one `extension`, `min_bytes`, `max_bytes`, `modified_after`,
// `modified_before` and `kind`. Anything the grammar parsed that those cannot carry is returned as a
// notice, never dropped silently and never approximated: `in:` and `under:`, extensions after the first,
// negated terms, and a quoted phrase with a space (v1 `text` would split it into separate words).
// A quoted word without a space is sent as a plain word; v1 `text` matches inside names from three
// characters on, and the existing "Short text" coverage notice covers the shorter case.
//
// The separate Extension, Kind and Size controls are kept. One rule joins them to the box: a typed
// filter wins, and a control applies only to a field the typed query did not set. A control that is
// overridden with a different value is named in a notice.

import type { SearchRequest } from '../contracts.gen';
import { escapedName } from '../format/names';
import { parseBytes } from '../format/bytes';
import { modifiedBounds, parseQuery, type AgoUnit, type QueryIssueCode, type TimeBound } from './grammar';

export type FindRequest = Pick<SearchRequest, 'text' | 'extension' | 'min_bytes' | 'max_bytes' | 'modified_after' | 'modified_before' | 'kind'>;

/** The values of the Extension, Kind and Size controls beside the box. */
export interface FindControls { extension: string; kind: 'any' | 'file' | 'dir'; minBytes: string }

export interface FindPlan {
  request: FindRequest;
  /** Applied typed and control filters that the request carries, as plain text. */
  chips: string[];
  /** Everything typed that the request does not carry, and each issue the grammar raised. */
  notices: string[];
  /** True when the request would match the whole catalogue: nothing the engine can narrow by. */
  unconstrained: boolean;
  /** A contradictory range must be corrected before submission. */
  blocked: string | null;
}

/** One-click cleanup searches. Each one parses with no issue and is carried whole by the request. */
export const PRESETS: { label: string; query: string }[] = [
  { label: 'Files over 4 GiB', query: 'size:>4gb kind:file' },
  { label: 'Disk images over 1 GiB', query: 'ext:iso size:>1gb' },
  { label: 'Large and untouched for a year', query: 'size:>500mb modified:>1y kind:file' },
  { label: 'Old zip archives', query: 'ext:zip modified:>2y' },
];

/** Explorer opens with this query in the find bar and runs it (`?q=`, #1068); Today links the presets this way. */
export function findHref(query: string): string {
  return `#/explorer?q=${encodeURIComponent(query)}`;
}

const UNIT_WORD: Record<AgoUnit, string> = { h: 'hour', d: 'day', w: 'week', mo: 'month', y: 'year' };
const q = (s: string) => `“${escapedName(s)}”`;

/** Exact: a clean binary size reads as KiB/MiB/GiB, anything else stays in bytes. */
function size(bytes: bigint): string {
  return bytes % 1024n === 0n ? formatClean(bytes) : `${bytes} B`;
}
function formatClean(bytes: bigint): string {
  let n = bytes;
  let unit = 0;
  while (n >= 1024n && n % 1024n === 0n) {
    n /= 1024n;
    unit++;
  }
  return `${n} ${['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB', 'EiB'][unit]}`;
}

// A `>` bound arrives as the next byte up and a `<` bound as the byte below (the wire is inclusive):
// say "more than 4 GiB" when the byte above a clean size is the bound, so the chip is exact.
function minLabel(min: bigint): string {
  return min > 0n && (min - 1n) % 1024n === 0n && min % 1024n !== 0n ? `more than ${size(min - 1n)}` : `at least ${size(min)}`;
}
function maxLabel(max: bigint): string {
  return (max + 1n) % 1024n === 0n && max % 1024n !== 0n ? `less than ${size(max + 1n)}` : `at most ${size(max)}`;
}

function when(b: TimeBound, side: 'after' | 'before'): string {
  if ('at' in b) return `${side === 'after' ? 'from' : 'until'} ${b.at.slice(0, 10)} UTC`;
  const { n, unit } = b.ago;
  const days = unit === 'mo' ? n * 30 : unit === 'y' ? n * 365 : null;
  const span = `${n} ${UNIT_WORD[unit]}${n === 1 ? '' : 's'}${days === null ? '' : ` (${days} days)`}`;
  return side === 'after' ? `in the last ${span}` : `more than ${span} ago`;
}

const ISSUE: Record<QueryIssueCode, (t: string) => string> = {
  unterminated_quote: (t) => `The quote in ${t} is not closed. Its value runs to the end of the query; check the filters and notes below.`,
  bad_value: (t) => `${t} is not a valid filter, so it was not applied.`,
  negated_filter: (t) => `${t} cannot be negated, so it was not applied.`,
  too_many_terms: (t) => `${t} was not used: a query takes at most 8 words or phrases.`,
  term_too_long: (t) => `${t} was not used: a word or phrase is at most 64 characters.`,
  too_many_extensions: (t) => `${t} names more than 16 extensions, so the rest were not used.`,
};

export function planFind(input: string, controls: FindControls, nowMs: number): FindPlan {
  const p = parseQuery(input);
  const chips: string[] = [];
  const notices = p.issues.map((i) => ISSUE[i.code](q(i.token)));

  const words: string[] = [];
  for (const t of p.terms) {
    if (t.negated) notices.push(`Excluding ${q(t.text)} is not supported yet, so it was not applied.`);
    else if (/\s/.test(t.text)) notices.push(`The phrase ${q(t.text)} cannot be searched as one phrase yet, so it was not applied; type its words separately.`);
    else words.push(t.text);
  }

  const typedExt = p.extensions[0] ?? null;
  const control = controls.extension.trim().replace(/^\./, '').toLowerCase();
  const effectiveExt = typedExt ?? (control || null);
  if (effectiveExt !== null) chips.push(`Extension: .${escapedName(effectiveExt)}`);
  if (typedExt !== null) {
    if (p.extensions.length > 1) notices.push(`Only one extension can be searched at a time, so ${p.extensions.slice(1).map((e) => q(`.${e}`)).join(', ')} was not applied.`);
    if (control && control !== typedExt) notices.push(`The Extension box (${q(control)}) is ignored because the query says ext:${escapedName(typedExt)}.`);
  }

  const effectiveMin = p.min_bytes ?? (controls.minBytes || null);
  if (effectiveMin !== null) chips.push(`Size: ${minLabel(BigInt(effectiveMin))}`);
  if (p.max_bytes !== null) chips.push(`Size: ${maxLabel(BigInt(p.max_bytes))}`);
  if (controls.minBytes && p.min_bytes !== null && controls.minBytes !== p.min_bytes) {
    notices.push(`The Size box (at least ${size(parseBytes(controls.minBytes) ?? 0n)}) is ignored because the query sets its own minimum size.`);
  }

  if (p.modified_after) chips.push(`Modified ${when(p.modified_after, 'after')}`);
  if (p.modified_before) chips.push(`Modified ${when(p.modified_before, 'before')}`);
  // The engine never matches an entry with no recorded modified time against a time bound (schema): say so.
  if (p.modified_after || p.modified_before) notices.push('Entries with no recorded modified time are left out by a date filter.');
  const bounds = modifiedBounds(p, nowMs);

  const effectiveKind = p.kind !== 'any' ? p.kind : controls.kind;
  if (effectiveKind !== 'any') chips.push(`Kind: ${effectiveKind === 'dir' ? 'folders' : 'files'}`);
  if (p.kind !== 'any') {
    if (controls.kind !== 'any' && controls.kind !== p.kind) notices.push(`The Kind box is ignored because the query says kind:${p.kind}.`);
  }
  if (p.in_collection !== null) notices.push(`in:${escapedName(p.in_collection)} is not supported yet, so it was not applied.`);
  if (p.under !== null) notices.push(`under:${escapedName(p.under)} is not supported yet, so it was not applied.`);

  const request: FindRequest = {
    text: words.join(' '),
    extension: effectiveExt,
    min_bytes: effectiveMin,
    max_bytes: p.max_bytes,
    modified_after: bounds.after,
    modified_before: bounds.before,
    kind: effectiveKind,
  };
  const unconstrained = request.text === '' && request.extension === null && request.min_bytes === null && request.max_bytes === null
    && request.modified_after === null && request.modified_before === null && request.kind === 'any';
  const blocked = request.min_bytes !== null && request.max_bytes !== null && BigInt(request.min_bytes) > BigInt(request.max_bytes)
    ? 'The minimum size exceeds the maximum. Adjust the query or Size box before searching.' : null;
  if (blocked) notices.push(blocked);
  return { request, chips, notices, unconstrained, blocked };
}
