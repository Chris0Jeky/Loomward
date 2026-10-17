import { readFileSync } from 'node:fs';
import ts from 'typescript';
import { describe, expect, it, vi } from 'vitest';
import { LoomwardError } from '../../src/lib/transport/client';
import { fallbackStep, onRefetchError, resolveTrail } from '../../src/views/explorer/refetch';
import searchExample from '../../../contracts/v3/examples/commands/search.query.result.json';

/** Run the real instance-script functions/effect without a DOM or canvas. State is supplied by
 * the test; imports and rendering are excluded, so deferred replies control the interleaving. */
function viewScript(view: string, state: Record<string, unknown>, effectContaining: string) {
  const source = readFileSync(new URL(`../../src/views/${view}/View.svelte`, import.meta.url), 'utf8');
  const script = source.match(/<script lang="ts">([\s\S]*?)<\/script>/)![1]!;
  const file = ts.createSourceFile('view.ts', script, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const functions: string[] = [], names: string[] = [], effects: string[] = [];
  for (const node of file.statements) {
    if (ts.isFunctionDeclaration(node) && node.name) {
      functions.push(node.getText(file));
      names.push(node.name.text);
    } else if (ts.isVariableStatement(node)) {
      for (const decl of node.declarationList.declarations) {
        if (ts.isIdentifier(decl.name) && !(decl.name.text in state)) state[decl.name.text] = undefined;
      }
    } else if (ts.isExpressionStatement(node) && ts.isCallExpression(node.expression)
      && node.expression.expression.getText(file) === '$effect') {
      effects.push(node.expression.arguments[0]!.getText(file));
    }
  }
  const effect = effects.find(e => e.includes(effectContaining));
  if (!effect) throw new Error(`Missing ${view} effect containing ${effectContaining}`);
  const code = ts.transpileModule(`${functions.join('\n')}\nreturn { ${names.join(',')}, effect: (${effect}) };`, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None },
  }).outputText;
  return new Function('state', `with (state) { return (() => { ${code} })(); }`)(state) as {
    show(append?: boolean): Promise<void>;
    loadStarts(recovery?: unknown, refreshOnly?: boolean): Promise<boolean>;
    effect(): void;
  };
}

const untrack = <T>(fn: () => T) => fn();
const deferred = <T>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(r => { resolve = r; });
  return { promise, resolve };
};

describe('map slice installation', () => {
  it.each(['atlas', 'observatory'])('%s drops revoked hover information while refreshing a surviving selection', view => {
    const revoked = { id: 'revoked', name: 'Revoked synthetic root' };
    const oldSelection = { id: 'kept', name: 'Old name' };
    const freshSelection = { id: 'kept', name: 'Fresh name' };
    const slice = { nodes: [freshSelection] };
    const renderer = { setSlice: vi.fn(), info: vi.fn(id => id === 'kept' ? freshSelection : null), reveal: vi.fn() };
    const state = {
      untrack, tm: renderer, sb: renderer, clothSlice: slice,
      nav: { slice, here: { name: 'All roots' } },
      hovered: revoked, selected: oldSelection, tip: { name: revoked.name },
      revealed: true, shownKey: null, announceSlice: () => ({ key: {}, text: null }), say: { say: vi.fn() },
    };
    const viewCode = viewScript(view, state, '.setSlice(s)');
    viewCode.effect();
    expect(renderer.setSlice).toHaveBeenCalledWith(slice);
    expect(state.selected).toBe(freshSelection);
    expect(state.hovered).toBeNull();
    if (view === 'atlas') expect(state.tip).toBeNull();
    state.selected = revoked;
    viewCode.effect();
    expect(state.selected).toBeNull();
  });
});

function explorer(call: ReturnType<typeof vi.fn>) {
  const state = {
    session: { client: { call }, epoch: 0, handle: (e: Error) => e.message },
    untrack, tick: async () => {}, document: { activeElement: null }, HTMLElement: class {},
    LoomwardError, fallbackStep, onRefetchError, resolveTrail,
    PAGE: 50, gen: 0, startsRequest: 0, inspection: 0, abort: null, inspectAbort: null, pageGen: null,
    starts: [] as { id: string; name: string }[], trail: [] as { id: string; name: string }[],
    get here() { return this.trail.at(-1); },
    rows: [] as typeof searchExample.hits[number]['row'][], found: new Map(), seenIds: new Set(),
    searching: false, query: '', extension: '', minBytes: '', kind: 'any', basis: 'logical', sort: 'size_desc',
    next: null, total: null, error: '', busy: false,
  };
  return { state, code: viewScript('explorer', state, 'session.epoch') };
}

describe('Explorer live-index pagination', () => {
  it('updates a repeated node in place and appends new nodes without duplicate keys', async () => {
    const hit = searchExample.hits[0]!;
    const renamed = { ...hit, row: { ...hit.row, name: 'report_renamed.docx', logical_bytes: '123' }, highlights: [[0, 14]] };
    const another = { ...hit, row: { ...hit.row, node_id: 'nd_other', name: 'report_other.docx' } };
    const call = vi.fn()
      .mockResolvedValueOnce({ result: { ...searchExample, hits: [hit], next_cursor: 'next' } })
      .mockResolvedValueOnce({ result: { ...searchExample, hits: [renamed, another] } });
    const { state, code } = explorer(call);
    state.searching = true;
    state.query = 'report';
    await code.show();
    expect(state.rows).toEqual([hit.row]);
    await code.show(true);
    expect(call.mock.calls[1]![1].cursor).toBe('next');
    expect(state.rows).toEqual([renamed.row, another.row]);
    expect(new Set(state.rows.map(r => r.node_id)).size).toBe(state.rows.length);
    expect(state.found.get(hit.row.node_id)).toEqual(renamed);
  });
});

const rootA = { id: 'root_a', name: 'Synthetic A' }, rootB = { id: 'root_b', name: 'Synthetic B' };
const roots = (...items: typeof rootA[]) => ({ nodes: items.map(r => ({ node_id: r.id, name: r.name, parent: 0 })) });
const children = { generation: 'g1', items: [], next_cursor: null, total: 0, budget_hit: false };

describe('Explorer starting points after invalidation', () => {
  it.each([false, true])('removes a revoked sibling while preserving the current trail and searching=%s', async searching => {
    let revoked = false;
    const call = vi.fn(async (command: string) => ({ result: command === 'tree.slice'
      ? revoked ? roots(rootA) : roots(rootA, rootB)
      : command === 'search.query' ? searchExample : children }));
    const { state, code } = explorer(call);
    expect(await code.loadStarts()).toBe(true);
    expect(state.starts).toEqual([rootA, rootB]);
    state.trail = [rootA, { id: 'child_a', name: 'Synthetic child' }];
    state.searching = searching;
    state.query = 'report';
    state.extension = 'docx';
    const trail = [...state.trail];
    revoked = true;
    state.session.epoch++;
    code.effect();
    await vi.waitFor(() => expect(state.starts).toEqual([rootA]));
    await vi.waitFor(() => expect(state.busy).toBe(false));
    expect(state.trail).toEqual(trail);
    expect(state.searching).toBe(searching);
    expect(state.query).toBe('report');
    expect(state.extension).toBe('docx');
    expect(call.mock.calls.at(-1)![0]).toBe(searching ? 'search.query' : 'tree.children');
  });

  it('drops unavailable starting points when their refresh fails', async () => {
    let fail = false;
    const { state, code } = explorer(vi.fn(async (command: string) => {
      if (command === 'tree.slice' && fail) throw new Error('synthetic offline catalogue');
      return { result: command === 'tree.slice' ? roots(rootA, rootB) : children };
    }));
    await code.loadStarts();
    fail = true;
    code.effect();
    await vi.waitFor(() => expect(state.starts).toEqual([]));
    await vi.waitFor(() => expect(state.error).toContain('synthetic offline catalogue'));
  });

  it('does not let a delayed starting-point response rewind a newer search', async () => {
    const pending = deferred<{ result: ReturnType<typeof roots> }>();
    let delay = false;
    const { state, code } = explorer(vi.fn(async (command: string) => command === 'tree.slice'
      ? delay ? pending.promise : { result: roots(rootA, rootB) }
      : { result: command === 'search.query' ? searchExample : children }));
    await code.loadStarts();
    delay = true;
    code.effect();
    state.searching = true;
    state.query = 'newest';
    await code.show();
    pending.resolve({ result: roots(rootA) });
    await vi.waitFor(() => expect(state.starts).toEqual([rootA]));
    await vi.waitFor(() => expect(state.busy).toBe(false));
    expect(state.searching).toBe(true);
    expect(state.query).toBe('newest');
    expect(state.rows).toEqual(searchExample.hits.map(h => h.row));
  });

  it('aborts an obsolete root refresh without letting its late reply restore a revoked root', async () => {
    const pending = deferred<{ result: ReturnType<typeof roots> }>();
    const call = vi.fn()
      .mockImplementation(async (command: string) => ({ result: command === 'tree.slice' ? roots(rootA) : children }));
    const { state, code } = explorer(call);
    await code.loadStarts();
    call.mockImplementationOnce(() => pending.promise);
    const older = code.loadStarts(undefined, true);
    const signal = (call.mock.calls.filter(args => args[0] === 'tree.slice').at(-1) as unknown as [string, unknown, { signal: AbortSignal }])[2].signal;
    code.effect();
    expect(signal.aborted).toBe(true);
    pending.resolve({ result: roots(rootA, rootB) });
    expect(await older).toBe(false);
    await vi.waitFor(() => expect(state.starts).toEqual([rootA]));
  });
});
