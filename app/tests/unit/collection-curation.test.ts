import { readFileSync } from 'node:fs';
import ts from 'typescript';
import { describe, expect, it, vi } from 'vitest';
import { LoomwardError } from '../../src/lib/transport/client';
import { escapedName } from '../../src/lib/format/names';
import { formatCount } from '../../src/lib/format/time';

// Run the view's actual handlers; the browser journey covers Svelte bindings and focus.
function curation(props: Record<string, any> = {}, transport?: ReturnType<typeof vi.fn>) {
  const source = readFileSync(new URL('../../src/views/collections/View.svelte', import.meta.url), 'utf8');
  const script = source.match(/<script lang="ts">([\s\S]*?)<\/script>/)![1]!;
  const ast = ts.createSourceFile('curation.ts', script, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const functions: string[] = [], names: string[] = [], effects: string[] = [];
  const collection = { collection_id: 'co_1', name: 'Papers', kind: 'human' };
  const page = { collection_id: 'co_1', items: [], total: 0, next_cursor: null, budget_hit: false };
  const call = transport ?? vi.fn(async (command: string) => ({ result: command === 'collections.list' ? { collections: [collection] } : page, meta: { state_rev: '5' } }));
  const result = { text: '', say(text: string) { this.text = text; }, clear() { this.text = ''; } };
  const state: Record<string, any> = {
    selected: 'co_1', collections: [collection], members: [], selectedMembers: ['om_1', 'om_2'],
    next: null, total: 2, loading: false, removing: false, creating: false, name: '', stateRev: '4', notice: '', error: '', result,
    alive: true, generation: 0, abort: null, sending: false, uncertain: '', pendingName: '', needsReread: false,
    summary: { focus: vi.fn() }, more: { focus: vi.fn() }, document: { activeElement: null, body: {} },
    tick: vi.fn(async () => {}), untrack: (fn: () => unknown) => fn(), LoomwardError, escapedName, formatCount,
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
    } else if (ts.isExpressionStatement(node) && ts.isCallExpression(node.expression) && node.expression.expression.getText(ast) === '$effect') effects.push(node.expression.arguments[0]!.getText(ast));
  }
  const change = source.match(/<select bind:value=\{selected\} onchange=\{([\s\S]*?)\} disabled=/)![1]!;
  const api = new Function('state', `with (state) { ${compile(`${functions.join('\n')}\nreturn { ${names.join(',')}, changeCollection: (${change}), effects: [${effects.join(',')}] };`)} }`)(state) as {
    remove(member?: { member_id: string }): Promise<void>; load(append?: boolean, restarted?: boolean): Promise<boolean>;
    changeCollection(event: { currentTarget: { value: string } }): void;
    invalidate(): void; effects: (() => void)[];
  };
  return { state, api, call, result };
}

const later = () => {
  let resolve!: (value: any) => void, reject!: (reason: Error) => void;
  const promise = new Promise<any>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const page = (id = 'co_1', items: any[] = [], next: string | null = null) => ({ result: { collection_id: id, items, total: items.length, next_cursor: next, budget_hit: false }, meta: { state_rev: '5' } });
const refused = (code: 'stale_generation' | 'deadline_exceeded') => new LoomwardError({ code, message: code, retryable: true, detail: null });

describe('collection curation', () => {
  it('removes the exact selected durable IDs with the displayed state revision', async () => {
    const { api, call } = curation();
    await api.remove();
    expect(call.mock.calls[0]).toMatchObject(['collections.update_members', { collection_id: 'co_1', add: [], remove: [], remove_members: ['om_1', 'om_2'] }, { expectedStateRev: '4' }]);
    expect(call.mock.calls.filter(([command]) => command === 'collections.update_members')).toHaveLength(1);
  });

  it('keeps single removal and clears the selection after a successful reread', async () => {
    const { api, call, state, result } = curation();
    await api.remove({ member_id: 'om_7' });
    expect(call.mock.calls[0]![1]).toMatchObject({ remove_members: ['om_7'] });
    expect(state.selectedMembers).toEqual([]);
    expect(result.text).toBe('Member removed. No file was changed.');
    expect(state.summary.focus).toHaveBeenCalledOnce();
  });

  it('deduplicates IDs and never claims a count of newly removed members', async () => {
    const { api, call, result } = curation({ selectedMembers: ['om_1', 'om_1'] });
    await api.remove();
    expect(call.mock.calls[0]![1]).toMatchObject({ remove_members: ['om_1'] });
    expect(result.text).toContain('Removal saved for the selection of 1 member');
    expect(result.text).not.toContain('1 member removed');
  });

  it.each([[], Array.from({ length: 501 }, (_, i) => 'om_' + (i + 1))])('rejects empty or oversized selections', async (...ids) => {
    const { api, call } = curation({ selectedMembers: ids });
    await api.remove();
    expect(call).not.toHaveBeenCalled();
  });

  it('accepts exactly 500 unique IDs in one write', async () => {
    const ids = Array.from({ length: 500 }, (_, i) => 'om_' + (i + 1));
    const { api, call } = curation({ selectedMembers: ids });
    await api.remove();
    expect(call.mock.calls[0]![1]).toMatchObject({ remove_members: ids });
    expect(call.mock.calls.filter(([c]) => c === 'collections.update_members')).toHaveLength(1);
  });

  it.each([{ loading: true }, { removing: true }, { needsReread: true }, { stateRev: null }, { selected: '' }, { collections: [{ collection_id: 'co_1', kind: 'label_view' }] }])('blocks actions in unsafe view state %j', async props => {
    const { api, call } = curation(props);
    await api.remove();
    expect(call).not.toHaveBeenCalled();
  });

  it('blocks a disconnected client even before unmount', async () => {
    const { api, call, state } = curation();
    state.session.state = 'unavailable';
    await api.remove();
    expect(call).not.toHaveBeenCalled();
  });

  it('preserves selected stable IDs while appending another member page', async () => {
    const call = vi.fn(async () => page('co_1', [{ member_id: 'om_3', entry: null, status: 'unresolved' }]));
    const { api, state } = curation({ members: [{ member_id: 'om_1' }], next: 'cursor' }, call);
    await api.load(true);
    expect(state.members.map((m: any) => m.member_id)).toEqual(['om_1', 'om_3']);
    expect(state.selectedMembers).toEqual(['om_1', 'om_2']);
    expect(call.mock.calls[0]).toMatchObject(['collections.members', { cursor: 'cursor', collection_id: 'co_1' }, { signal: expect.any(AbortSignal) }]);
  });

  it('clears selection on refresh and restarts a stale page only once', async () => {
    let reads = 0;
    const call = vi.fn(async (command: string) => {
      if (command === 'collections.list') return { result: { collections: [{ collection_id: 'co_1', kind: 'human' }] } };
      if (++reads === 1) throw refused('stale_generation');
      return page();
    });
    const { api, state } = curation({ next: 'stale' }, call);
    await api.load(true);
    expect(state.selectedMembers).toEqual([]);
    expect(state.notice).toContain('start again from the top');
    expect(reads).toBe(2);
    expect(state.needsReread).toBe(false);
  });

  it('does not repaint from an old list read after switching collections', async () => {
    const old = later(); let lists = 0;
    const call = vi.fn(async (command: string) => command === 'collections.list'
      ? ++lists === 1 ? old.promise : { result: { collections: [{ collection_id: 'co_2', kind: 'human' }] } }
      : page('co_2', [{ member_id: 'om_8' }]));
    const { api, state } = curation({}, call);
    const first = api.load();
    state.selected = 'co_2'; await api.load();
    old.resolve({ result: { collections: [{ collection_id: 'co_1', kind: 'human' }] } });
    await first;
    expect(state.selected).toBe('co_2');
    expect(state.members).toEqual([{ member_id: 'om_8' }]);
  });

  it('loads the new collection when change fires before the Svelte value binding', async () => {
    const list = later();
    const call = vi.fn(async (command: string) => command === 'collections.list' ? list.promise : page('co_2', [{ member_id: 'om_8' }]));
    const { api, state } = curation({}, call);
    api.changeCollection({ currentTarget: { value: 'co_2' } });
    state.selected = 'co_2'; // Svelte updates bind:value after the explicit listener.
    list.resolve({ result: { collections: [{ collection_id: 'co_1', kind: 'human' }, { collection_id: 'co_2', kind: 'human' }] } });
    for (let i = 0; i < 8; i++) await Promise.resolve();
    expect(call.mock.calls[1]).toMatchObject(['collections.members', { collection_id: 'co_2' }, { signal: expect.any(AbortSignal) }]);
    expect(state.members).toEqual([{ member_id: 'om_8' }]);
    expect(state.loading).toBe(false);
  });

  it('clears selection through the real session effect on engine restart', async () => {
    const { api, state } = curation();
    state.session.epoch++; state.session.connectionEpoch++;
    api.effects.forEach(effect => effect());
    expect(state.selectedMembers).toEqual([]);
    expect(state.stateRev).toBeNull();
    for (let i = 0; i < 5; i++) await Promise.resolve();
    expect(state.needsReread).toBe(false);
  });

  it('stops after a second stale page and requires another explicit refresh', async () => {
    let reads = 0;
    const call = vi.fn(async (command: string) => {
      if (command === 'collections.list') return { result: { collections: [{ collection_id: 'co_1', kind: 'human' }] } };
      reads++; throw refused('stale_generation');
    });
    const { api, state } = curation({ next: 'stale' }, call);
    await api.load(true);
    expect(reads).toBe(2); expect(state.needsReread).toBe(true);
    expect(state.selectedMembers).toEqual([]); expect(state.error).toBe('stale_generation');
  });

  it.each(['reply', 'error'])('fences an old member %s and finally after a new load starts', async outcome => {
    const old = later(), fresh = later(); let reads = 0;
    const call = vi.fn(async (command: string) => command === 'collections.list'
      ? { result: { collections: [{ collection_id: 'co_1', kind: 'human' }] } }
      : ++reads === 1 ? old.promise : fresh.promise);
    const { api, state } = curation({ next: 'cursor' }, call);
    const first = api.load(true);
    const second = api.load(); await Promise.resolve();
    if (outcome === 'reply') old.resolve(page('co_1', [{ member_id: 'om_old' }]));
    else old.reject(new Error('old failure'));
    await first;
    expect(state.error).toBe(''); expect(state.loading).toBe(true);
    fresh.resolve(page('co_1', [{ member_id: 'om_new' }])); await second;
    expect(state.members).toEqual([{ member_id: 'om_new' }]);
  });

  it.each(['collection', 'epoch', 'connection', 'client', 'unmount'])('freezes the payload and fences a late write after %s invalidation', async change => {
    const write = later(); const call = vi.fn(() => write.promise);
    const { api, state, result } = curation({}, call);
    const pending = api.remove();
    state.selectedMembers.push('om_3');
    if (change === 'collection') state.selected = 'co_2';
    if (change === 'epoch') state.session.epoch++;
    if (change === 'connection') state.session.connectionEpoch++;
    if (change === 'client') state.session.client = { call: vi.fn() };
    if (change === 'unmount') state.alive = false;
    api.invalidate();
    expect((call.mock.calls[0] as unknown as any[])[1]).toMatchObject({ collection_id: 'co_1', remove_members: ['om_1', 'om_2'] });
    expect((call.mock.calls[0] as unknown as any[])[2].signal.aborted).toBe(true);
    expect(result.text).toContain('may already have been saved');
    write.resolve(page()); await pending;
    expect(call).toHaveBeenCalledOnce();
    expect(result.text).not.toContain('Removal saved for');
    expect(state.selectedMembers).toEqual([]);
    expect(state.summary.focus).not.toHaveBeenCalled();
  });

  it('fences an old write error/finally while a newer write is pending', async () => {
    const old = later(), fresh = later(); let writes = 0;
    const call = vi.fn(async (command: string) => command === 'collections.update_members' ? ++writes === 1 ? old.promise : fresh.promise
      : command === 'collections.list' ? { result: { collections: [{ collection_id: 'co_1', name: 'Papers', kind: 'human' }] } } : page());
    const { api, state, result } = curation({}, call);
    const first = api.remove(); api.invalidate();
    await api.load(); state.selectedMembers = ['om_3'];
    const second = api.remove();
    old.reject(new Error('old failure')); await first;
    expect(state.removing).toBe(true); expect(state.error).toBe(''); expect(result.text).toBe('');
    fresh.resolve(page()); await second;
    expect(result.text).toContain('Removal saved'); expect(state.removing).toBe(false);
  });

  it.each(['stale_generation', 'deadline_exceeded', 'transport'] as const)('requires a reread after %s without replaying the mutation', async reason => {
    const call = vi.fn(async () => { throw reason === 'transport' ? new Error('connection lost') : refused(reason); });
    const { api, state, result } = curation({}, call);
    await api.remove();
    expect(state.selectedMembers).toEqual([]); expect(state.stateRev).toBeNull(); expect(state.needsReread).toBe(true);
    state.selectedMembers = ['om_1']; await api.remove();
    expect(call).toHaveBeenCalledOnce();
    if (reason === 'stale_generation') expect(result.text).toBe('');
    else expect(result.text).toContain('may already have been saved');
  });

  it('retains an uncertain outcome across a successful reread without implying rollback', async () => {
    const { api, call, state, result } = curation();
    call.mockImplementationOnce(async () => { throw refused('deadline_exceeded'); });
    await api.remove(); await api.load();
    expect(state.needsReread).toBe(false); expect(state.selectedMembers).toEqual([]);
    expect(result.text).toContain('may already have been saved');
    expect(call.mock.calls.filter(([c]) => c === 'collections.update_members')).toHaveLength(1);
  });

  it('keeps a known saved result honest if its refresh fails', async () => {
    const call = vi.fn(async (command: string) => {
      if (command === 'collections.update_members') return page();
      throw new Error('refresh failed');
    });
    const { api, state, result } = curation({}, call);
    await api.remove();
    expect(result.text).toContain('Removal saved'); expect(state.error).toBe('refresh failed');
    expect(state.needsReread).toBe(true); expect(state.summary.focus).not.toHaveBeenCalled();
  });

  it('does not move focus after the view changes during tick', async () => {
    const tick = later(); const { api, state } = curation({ tick: () => tick.promise });
    const pending = api.remove();
    for (let i = 0; i < 8; i++) await Promise.resolve();
    state.selected = 'co_2'; api.invalidate();
    tick.resolve(undefined); await pending;
    expect(state.summary.focus).not.toHaveBeenCalled();
  });
});
