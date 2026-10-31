import { readFileSync } from 'node:fs';
import ts from 'typescript';
import { describe, expect, it, vi } from 'vitest';
import { escapedName } from '../../src/lib/format/names';

// Exercise the component's real action functions; browser proof covers Svelte/DOM reactivity.
function membership(props: Record<string, unknown> = {}, call: ReturnType<typeof vi.fn> = vi.fn(async () => ({ result: { member_count: 2 }, meta: { state_rev: '4' } }))) {
  const source = readFileSync(new URL('../../src/lib/ui/AddToCollection.svelte', import.meta.url), 'utf8');
  const script = source.match(/<script lang="ts">([\s\S]*?)<\/script>/)![1]!;
  const ast = ts.createSourceFile('membership.ts', script, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const functions: string[] = [], names: string[] = [], effects: string[] = [];
  const result = { text: '', clear() { this.text = ''; }, say(text: string) { this.text = text; } };
  const state: Record<string, any> = {
    nodeId: 'nd_A', nodeIds: undefined, expectedGeneration: '7', contextKey: 0,
    selected: 'co_1', collections: [{ collection_id: 'co_1', name: 'Papers', kind: 'human' }],
    busy: false, sending: false, error: '', result, alive: true, generation: 0, request: null,
    onAdded: vi.fn(), untrack: (fn: () => unknown) => fn(), escapedName,
    session: { client: { call }, epoch: 1, connectionEpoch: 1, state: 'connected', handle: (e: Error) => e.message },
    ...props,
  };
  const compile = (text: string) => ts.transpileModule(text, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None } }).outputText;
  for (const node of ast.statements) {
    if (ts.isFunctionDeclaration(node) && node.name) { functions.push(node.getText(ast)); names.push(node.name.text); }
    else if (ts.isVariableStatement(node)) for (const decl of node.declarationList.declarations) {
      if (!ts.isIdentifier(decl.name)) continue;
      if (decl.initializer && ts.isCallExpression(decl.initializer) && decl.initializer.expression.getText(ast) === '$derived') {
        const evaluate = new Function('state', `with (state) { ${compile(`return (${decl.initializer.arguments[0]!.getText(ast)});`)} }`);
        Object.defineProperty(state, decl.name.text, { get: () => evaluate(state), configurable: true });
      } else if (!(decl.name.text in state)) state[decl.name.text] = undefined;
    }
    else if (ts.isExpressionStatement(node) && ts.isCallExpression(node.expression) && node.expression.expression.getText(ast) === '$effect') effects.push(node.expression.arguments[0]!.getText(ast));
  }
  const api = new Function('state', `with (state) { ${compile(`${functions.join('\n')}\nreturn { ${names.join(',')}, effects: [${effects.join(',')}] };`)} }`)(state) as {
    add(): Promise<void>; invalidate(): void; effects: (() => void)[];
  };
  return { state, api, call, result };
}

const later = () => {
  let resolve!: (value: any) => void;
  const promise = new Promise<any>(r => { resolve = r; });
  return { promise, resolve };
};
const response = { result: { member_count: 2 }, meta: { state_rev: '4' } };

describe('selected rows into a collection', () => {
  it('preserves a single inspected/project node and its observed generation', async () => {
    const { api, call } = membership();
    await api.add();
    expect(call.mock.calls).toHaveLength(2);
    expect(call.mock.calls[1]).toMatchObject(['collections.update_members', { collection_id: 'co_1', add: ['nd_A'], remove: [] }, { expectedStateRev: '4', expectedGeneration: '7' }]);
  });

  it('sends the exact unique selected set in one revision-bound write', async () => {
    const { api, call } = membership({ nodeIds: ['nd_A', 'nd_B', 'nd_A'], expectedGeneration: undefined });
    await api.add();
    expect(call.mock.calls).toHaveLength(2);
    expect(call.mock.calls[1]).toMatchObject(['collections.update_members', { collection_id: 'co_1', add: ['nd_A', 'nd_B'], remove: [] }, { expectedStateRev: '4' }]);
    expect((call.mock.calls[1] as any[])[2].expectedGeneration).toBeUndefined();
  });

  it.each([[[]], [Array.from({ length: 501 }, (_, i) => `nd_${i}`)]])('does not submit an empty or oversized selection', async nodeIds => {
    const { api, call } = membership({ nodeIds, expectedGeneration: undefined });
    await api.add();
    expect(call).not.toHaveBeenCalled();
  });

  it('accepts exactly 500 selected IDs', async () => {
    const nodeIds = Array.from({ length: 500 }, (_, i) => `nd_${i}`);
    const { api, call } = membership({ nodeIds, expectedGeneration: undefined });
    await api.add();
    expect((call.mock.calls[1] as any[])[1].add).toEqual(nodeIds);
  });

  it.each(['selection', 'target', 'navigation', 'epoch', 'connection', 'client', 'unmount'])('does not write after %s changes during the revision read', async change => {
    const read = later();
    const call = vi.fn(async () => read.promise);
    const { api, state } = membership({ nodeIds: ['nd_A', 'nd_B'], expectedGeneration: undefined }, call);
    const pending = api.add();
    if (change === 'selection') state.nodeIds = ['nd_C'];
    if (change === 'target') state.selected = 'co_2';
    if (change === 'navigation') state.contextKey++;
    if (change === 'epoch') state.session.epoch++;
    if (change === 'connection') state.session.connectionEpoch++;
    if (change === 'client') state.session.client = { call: vi.fn() };
    if (change === 'unmount') state.alive = false;
    read.resolve(response);
    await pending;
    expect(call).toHaveBeenCalledTimes(1);
    expect(state.result.text).toBe('');
  });

  it.each(['changed', 'cleared'])('keeps a frozen payload and an honest unknown outcome when selection is %s after submission', async change => {
    const write = later();
    const call = vi.fn(async (command: string) => command === 'collections.members' ? response : write.promise);
    const { api, state, result } = membership({ nodeIds: ['nd_A', 'nd_B'], expectedGeneration: undefined }, call);
    const pending = api.add();
    await Promise.resolve();
    expect(call).toHaveBeenCalledTimes(2);
    if (change === 'cleared') state.nodeIds = [];
    else { state.nodeIds.push('nd_C'); state.selected = 'co_2'; }
    api.effects.forEach(effect => effect());
    expect((call.mock.calls[1] as any[])[1]).toEqual({ collection_id: 'co_1', add: ['nd_A', 'nd_B'], remove: [] });
    expect((call.mock.calls[1] as any[])[2].signal.aborted).toBe(true);
    expect(result.text).toContain('may already have been saved');
    write.resolve(response);
    await pending;
    expect(result.text).not.toContain('Saved selection');
    expect(state.onAdded).not.toHaveBeenCalled();
    expect(state.busy).toBe(false);
  });

  it('does not let an older response clear the busy state or result of a newer submission', async () => {
    const first = later(), second = later();
    let writes = 0;
    const call = vi.fn(async (command: string) => command === 'collections.members' ? response : ++writes === 1 ? first.promise : second.promise);
    const { api, state, result } = membership({ nodeIds: ['nd_A'], expectedGeneration: undefined }, call);
    const old = api.add(); await Promise.resolve();
    state.nodeIds = ['nd_B']; api.effects.forEach(effect => effect());
    const fresh = api.add(); await Promise.resolve();
    first.resolve(response); await old;
    expect(state.busy).toBe(true);
    expect(result.text).toBe('');
    second.resolve(response); await fresh;
    expect(state.busy).toBe(false);
    expect(result.text).toBe('Saved selection of 1 row to “Papers”. No file was changed.');
    expect(state.onAdded).toHaveBeenCalledTimes(1);
  });

  it.each(['state revision changed', 'node replaced', 'root revoked', 'durable identity unavailable'])('reports %s without retry or partial writes', async reason => {
    const call = vi.fn(async (command: string) => {
      if (command === 'collections.update_members') throw new Error(reason);
      return response;
    });
    const { api, state, result } = membership({ nodeIds: ['nd_A', 'nd_B'], expectedGeneration: undefined }, call);
    await api.add();
    expect(call).toHaveBeenCalledTimes(2);
    expect(state.error).toBe(reason);
    expect(result.text).toBe('');
    expect(state.onAdded).not.toHaveBeenCalled();
    expect(state.busy).toBe(false);
  });

  it('refuses an unknown read revision before writing', async () => {
    const call = vi.fn(async () => ({ result: {}, meta: { state_rev: null } }));
    const { api, state } = membership({ nodeIds: ['nd_A', 'nd_B'], expectedGeneration: undefined }, call);
    await api.add();
    expect(call).toHaveBeenCalledTimes(1);
    expect(state.error).toContain('revision is unknown');
  });

  it('does not claim new members on re-add and keeps hostile collection names as display text', async () => {
    const { api, state, result, call } = membership({ nodeIds: ['nd_A', 'nd_B'], expectedGeneration: undefined,
      collections: [{ collection_id: 'co_1', name: '<img>\u202e', kind: 'human' }] });
    await api.add(); await api.add();
    expect(call).toHaveBeenCalledTimes(4);
    expect(result.text).toContain('Saved selection of 2 rows');
    expect(result.text).toContain('U+202E RLO');
    expect(result.text).not.toContain('2 new');
    expect(state.onAdded).toHaveBeenCalledTimes(2);
  });

  it('keeps both existing single-node callers generation-bound', () => {
    const explorer = readFileSync(new URL('../../src/views/explorer/View.svelte', import.meta.url), 'utf8');
    const projects = readFileSync(new URL('../../src/views/collections/ProjectDiscovery.svelte', import.meta.url), 'utf8');
    expect(explorer).toContain('<AddToCollection nodeId={detail.row.node_id} expectedGeneration={detailGeneration}');
    expect(projects).toContain('<AddToCollection nodeId={candidate.node_id} expectedGeneration={page.generation}');
  });
});
