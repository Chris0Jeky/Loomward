import { describe, expect, it, vi } from 'vitest';
import type { Client } from '../../src/lib/transport/client';
import { loadFootprint } from '../../src/views/collections/footprint';
import type { Footprint } from '../../src/views/collections/footprint';
import { render } from 'svelte/server';
import ProjectFootprint from '../../src/views/collections/ProjectFootprint.svelte';
import { readFileSync } from 'node:fs';
import ts from 'typescript';

function fixture() {
  const detail = { row: { node_id: 'project', kind: 'dir', coverage: 'complete', logical_bytes: '9007199254740993', allocated_bytes: null, files: 15, dirs: 1 }, root_id: 'root', identity: { observed_generation: '7' } };
  const families = { node_id: 'project', by: 'ext_family', basis: 'logical', generation: '7', complete: true, buckets: [{ key: 'code', label: 'Code', bytes: '9007199254740993', files: 15 }], other: { bytes: '0', files: 0 }, unknown: { bytes: '0', files: 0 } };
  const children = { anchor: 'project', generation: '7', items: Array.from({ length: 12 }, (_, i) => ({ node_id: `child${i}`, name: `File ${i}`, kind: 'file', coverage: 'complete', logical_bytes: String(20 - i) })), next_cursor: 'more', total: null, budget_hit: false };
  const responses = { 'node.inspect': { result: detail, meta: { catalog_rev: '8', state_rev: '9' } }, 'stats.breakdown': { result: families, meta: { catalog_rev: '8', state_rev: null } }, 'tree.children': { result: children, meta: { catalog_rev: '8', state_rev: null } } };
  const call = vi.fn(async (command: keyof typeof responses) => responses[command]);
  const client = { call } as unknown as Client;
  const ctl = new AbortController();
  const load = () => loadFootprint(client, 'project', 'root', '7', ctl.signal);
  return { detail, families, children, responses, call, ctl, load };
}

describe('recorded project footprint', () => {
  it('reads only the three bounded existing commands with one generation and signal', async () => {
    const f = fixture();
    const result = await f.load();
    expect(result).toMatchObject({ detail: f.detail, families: f.families, children: f.children });
    expect(f.call.mock.calls.map(args => args[0])).toEqual(['node.inspect', 'stats.breakdown', 'tree.children']);
    expect(f.call).toHaveBeenCalledWith('tree.children', { node_id: 'project', sort: 'size_desc', basis: 'logical', limit: 12, cursor: null }, { expectedGeneration: '7', signal: f.ctl.signal });
    for (const args of f.call.mock.calls) expect(args).toHaveLength(3);
    expect(result.detail.row.logical_bytes).toBe('9007199254740993');
    expect(result.detail.row.allocated_bytes).toBeNull();
    expect(result.children.next_cursor).toBe('more');
  });
  it.each(['partial', 'denied', 'unknown', 'stale'] as const)('keeps %s coverage and incomplete families intact', async coverage => {
    const f = fixture(); f.detail.row.coverage = coverage; f.families.complete = false;
    const result = await f.load();
    expect(result.detail.row.coverage).toBe(coverage);
    expect(result.families.complete).toBe(false);
  });
  it.each(['node', 'root', 'detail-generation', 'family-generation', 'family-node', 'child-generation', 'child-anchor', 'revision'] as const)('refuses the whole brief for a changed %s', async mismatch => {
    const f = fixture();
    if (mismatch === 'node') f.detail.row.node_id = 'replacement';
    if (mismatch === 'root') f.detail.root_id = 'other';
    if (mismatch === 'detail-generation') f.detail.identity.observed_generation = '8';
    if (mismatch === 'family-generation') f.families.generation = '8';
    if (mismatch === 'family-node') f.families.node_id = 'replacement';
    if (mismatch === 'child-generation') f.children.generation = '8';
    if (mismatch === 'child-anchor') f.children.anchor = 'replacement';
    if (mismatch === 'revision') f.responses['tree.children'].meta.catalog_rev = '10';
    await expect(f.load()).rejects.toMatchObject({ code: 'stale_generation' });
  });
  it.each(['permission_denied', 'capability_unavailable', 'not_found', 'stale_generation'])('preserves %s instead of fabricating a brief', async code => {
    const f = fixture(); f.call.mockRejectedValueOnce(Object.assign(new Error(code), { code }));
    await expect(f.load()).rejects.toMatchObject({ code });
  });
  it('discards late results even when transport ignores cancellation', async () => {
    const f = fixture(); const result = f.load(); f.ctl.abort();
    await expect(result).rejects.toMatchObject({ name: 'AbortError' });
  });
});

describe('footprint evidence wording', () => {
  const show = (f: ReturnType<typeof fixture>) => render(ProjectFootprint, { props: { data: { detail: f.detail, families: f.families, children: f.children } as unknown as Footprint, root: undefined, generation: '7' } }).body;
  it('renders exact large bytes, unknown allocation and a first-page limit without deletion claims', () => {
    const body = show(fixture());
    expect(body).toContain('9007199254740993 bytes');
    expect(body).toMatch(/Allocated size<\/dt>.*?<dd[^>]*>unknown<\/dd>/s);
    expect(body).toContain('Recorded scan finished: unknown');
    expect(body).toContain('only the returned first page');
    expect(body).toContain('safe deletion: unknown');
    expect(body).not.toContain('globally largest');
  });
  it.each(['denied', 'unknown', 'unscanned', 'stale'])('hides stale totals and lists for %s coverage', coverage => {
    const f = fixture(); f.detail.row.coverage = coverage;
    const body = show(f);
    expect(body).not.toContain('9007199254740993');
    expect(body).not.toContain('Recorded direct children');
    expect(body).toContain('unknown for this coverage state');
  });
  it('keeps partial empty distinct from complete empty', () => {
    const f = fixture(); f.detail.row.coverage = 'partial'; f.families.complete = false; f.children.items = [];
    expect(show(f)).toContain('not proof of an empty folder');
    expect(show(f)).toContain('Family coverage is incomplete');
    f.detail.row.coverage = 'complete'; f.families.complete = true; f.children.next_cursor = null as unknown as string;
    expect(show(f)).not.toContain('not proof of an empty folder');
  });
});

// Exercise the actual view's request ownership with deliberately late replies, as view-races.test.ts does.
function discovery(load: ReturnType<typeof vi.fn>) {
  const source = readFileSync(new URL('../../src/views/collections/ProjectDiscovery.svelte', import.meta.url), 'utf8');
  const script = source.match(/<script lang="ts">([\s\S]*?)<\/script>/)![1]!;
  const file = ts.createSourceFile('view.ts', script, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const functions = file.statements.filter(ts.isFunctionDeclaration).map(n => n.getText(file)).join('\n');
  const state = { session: { client: {}, epoch: 1, handle: (e: Error) => e.message }, loadFootprint: load, LoomwardError: Error,
    page: { root_id: 'root', generation: '7' }, expanded: null, footprint: null, footprintError: '', footprintAbort: null,
    sequence: 0, abort: null, adding: null, error: '', busy: false };
  const code = ts.transpileModule(`${functions}; return { expand, clear, closeFootprint };`, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  const api = new Function('state', `with (state) { return (() => { ${code} })(); }`)(state) as { expand(node: string): Promise<void>; clear(): void; closeFootprint(): void };
  return { state, api };
}

describe('explicit footprint request ownership', () => {
  it('loads only on expansion and ignores the older project response', async () => {
    const pending: ((value: unknown) => void)[] = [];
    const load = vi.fn((..._args: Parameters<typeof loadFootprint>) => new Promise(resolve => pending.push(resolve)));
    const { state, api } = discovery(load);
    expect(load).not.toHaveBeenCalled();
    const first = api.expand('first'), second = api.expand('second');
    pending[1]!({ project: 'second' }); await second;
    pending[0]!({ project: 'first' }); await first;
    expect(state.footprint).toEqual({ project: 'second' });
    expect(state.expanded).toBe('second');
    expect(load.mock.calls[0]![4].aborted).toBe(true);
  });
  it.each(['close', 'refresh', 'epoch'])('discards the whole pending brief on %s', async change => {
    let resolve!: (value: unknown) => void;
    const { state, api } = discovery(vi.fn(() => new Promise(r => { resolve = r; })));
    const pending = api.expand('first');
    if (change === 'close') api.closeFootprint();
    if (change === 'refresh') api.clear();
    if (change === 'epoch') state.session.epoch++;
    resolve({ project: 'obsolete' }); await pending;
    expect(state.footprint).toBeNull();
  });
  it('drops discovery and all actions after a refused current request', async () => {
    const { state, api } = discovery(vi.fn(async () => { throw Object.assign(new Error('revoked'), { code: 'permission_denied' }); }));
    await api.expand('first');
    expect(state.page).toBeNull();
    expect(state.footprint).toBeNull();
    expect(state.adding).toBeNull();
    expect(state.error).toContain('observations or access changed');
  });
});
