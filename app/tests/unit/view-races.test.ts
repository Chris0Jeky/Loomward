import { readFileSync } from 'node:fs';
import ts from 'typescript';
import { describe, expect, it, vi } from 'vitest';
import { LoomwardError } from '../../src/lib/transport/client';
import { fallbackStep, onRefetchError, resolveTrail } from '../../src/views/explorer/refetch';
import searchExample from '../../../contracts/v3/examples/commands/search.query.result.json';
import learningStatus from '../../../contracts/v3/examples/commands/learning.status.result.json';
import learningQueue from '../../../contracts/v3/examples/commands/learning.queue.result.json';
import alertReport from '../../../contracts/v3/examples/commands/alerts.list.result.json';
import proposal from '../../../contracts/v3/examples/commands/proposals.get.result.json';
import { formFrom, settingsFrom } from '../../src/views/alerts/model';
import { formatBytes } from '../../src/lib/format/bytes';

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
  const code = ts.transpileModule(`${functions.join('\n')}\nreturn { ${names.join(',')}, effect: (${effect}), evaluate: expression => eval(expression) };`, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None },
  }).outputText;
  return new Function('state', `with (state) { return (() => { ${code} })(); }`)(state) as {
    show(append?: boolean): Promise<void>;
    loadStarts(recovery?: unknown, refreshOnly?: boolean): Promise<boolean>;
    effect(): void;
    reload(): void;
    load(cursor?: string | null, restarted?: boolean): Promise<void>;
    loadSaved(id: string): Promise<void>;
    save(): Promise<void>;
    confirm(): Promise<void>;
    ask(pending: unknown): Promise<void>;
    cancel(): Promise<void>;
    reset(): void;
    refit(): Promise<void>;
    evaluate(expression: string): unknown;
  };
}

const untrack = <T>(fn: () => T) => fn();
const deferred = <T>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(r => { resolve = r; });
  return { promise, resolve };
};

function review(call: ReturnType<typeof vi.fn>) {
  const state = {
    session: { client: { call }, epoch: 0, handle: (e: Error) => e.message },
    untrack, LoomwardError, alive: true, generation: 0,
    status: learningStatus as typeof learningStatus | null,
    queue: learningQueue as typeof learningQueue | null,
    error: '', pending: null, selected: 'nd_old', working: false, active: false, gated: false, job: null,
  };
  return { state, code: viewScript('review', state, 'session.epoch') };
}

const staleReview = () => new LoomwardError({ code: 'stale_generation', message: 'Catalogue or feedback changed during review.', retryable: true, detail: null });

describe('Learning review refresh after concurrent feedback', () => {
  it('restarts status and queue once from the top when feedback invalidates the queue', async () => {
    const fresh = { ...learningStatus, human_events: 2, model_id: null, calibrated: false, autonomy_allowed: false };
    let queues = 0;
    const call = vi.fn(async (command: string) => {
      if (command === 'learning.queue' && ++queues === 1) throw staleReview();
      return { result: command === 'learning.status' ? fresh : learningQueue };
    });
    const { state, code } = review(call);
    await code.load('old_cursor');
    expect(state.status).toEqual(fresh);
    expect(state.queue).toEqual(learningQueue);
    expect(state.error).toBe('');
    expect(state.selected).toBe('');
    expect(call.mock.calls.map(args => args[0])).toEqual(['learning.status', 'learning.queue', 'learning.status', 'learning.queue']);
    expect((call.mock.calls.at(-1) as unknown as [string, { cursor: string | null }])[1].cursor).toBeNull();
  });

  it.each(['stale_generation', 'internal_error'] as const)('keeps %s failures honest and bounds retries', async code => {
    const failure = new LoomwardError({ code, message: 'Learning unavailable.', retryable: false, detail: null });
    const call = vi.fn(async (command: string) => {
      if (command === 'learning.queue') throw failure;
      return { result: learningStatus };
    });
    const { state, code: view } = review(call);
    await view.load();
    expect(state.status).toEqual(learningStatus);
    expect(state.queue).toBeNull();
    expect(state.error).toBe('Learning unavailable.');
    expect(call).toHaveBeenCalledTimes(code === 'stale_generation' ? 4 : 2);
  });

  it.each(['superseded', 'unmounted'])('does not retry a stale reply after the request is %s', async mode => {
    let reject!: (error: Error) => void;
    const response = new Promise<never>((_, fail) => { reject = fail; });
    const call = vi.fn(async (command: string) => command === 'learning.queue' ? response : { result: learningStatus });
    const { state, code } = review(call);
    const loading = code.load();
    if (mode === 'unmounted') state.alive = false;
    else state.generation++;
    reject(staleReview());
    await loading;
    expect(call).toHaveBeenCalledTimes(2);
    expect(state.status).toBe(learningStatus);
    expect(state.error).toBe('');
  });
});

describe('Learning resources fail independently', () => {
  it('keeps current status and refit availability when the queue exceeds its file budget', async () => {
    const fresh = { ...learningStatus, gate: null, human_events: 10001 };
    const call = vi.fn(async (command: string) => {
      if (command === 'learning.queue') throw new LoomwardError({ code: 'resource_budget', message: 'Queue exceeds 10000 files.', retryable: false, detail: null });
      if (command === 'learning.refit' || command === 'jobs.get') return { result: { job: { job_id: 'job_refit', state: 'completed' } } };
      return { result: fresh };
    });
    const { state, code } = review(call);
    await code.load();
    expect(state.status).toEqual(fresh);
    expect(state.queue).toBeNull();
    expect(state.error).toContain('Queue exceeds');
    expect(call).toHaveBeenCalledTimes(2);
    await code.refit();
    expect(call).toHaveBeenCalledWith('learning.refit', {});
    expect(call).toHaveBeenCalledWith('jobs.get', { job_id: 'job_refit' });
  });

  it('invalidates status when its own read fails', async () => {
    const { state, code } = review(vi.fn(async (command: string) => {
      if (command === 'learning.status') throw new Error('Status unavailable');
      return { result: learningQueue };
    }));
    await code.load();
    expect(state.status).toBeNull();
    expect(state.error).toBe('Status unavailable');
  });
});

describe('Saved tier scenario source', () => {
  it('labels a historical plan with its ineligible saved source while keeping the eligible selector', async () => {
    const historical = { ...proposal, summary: { ...proposal.summary, source_volume_id: 'vol_a', status: 'stale' } };
    const volumes = [
      { volume_id: 'vol_a', display_name: 'Synthetic A', source_eligible: false },
      { volume_id: 'vol_b', display_name: 'Synthetic B', source_eligible: true },
    ];
    const state = {
      session: { client: { call: vi.fn(async () => ({ result: historical })) }, handle: (e: Error) => e.message },
      untrack, savedTicket: 0, source: 'vol_b', savedDetail: null, savedError: '', assume: {},
      volume: (id: string) => volumes.find(v => v.volume_id === id),
      vname: (id: string) => volumes.find(v => v.volume_id === id)?.display_name ?? id,
      src: volumes[1], p: proposal.plan, formatBytes, say: { say: vi.fn() },
    };
    const code = viewScript('tiers', state, 'session.epoch');
    await code.loadSaved('saved_a');
    expect(state.source).toBe('vol_b');
    expect(state.savedDetail).toEqual(historical);
    const source = readFileSync(new URL('../../src/views/tiers/View.svelte', import.meta.url), 'utf8');
    const target = source.match(/<dt>Target free<\/dt><dd[^>]*>(.*?)<\/dd>/)![1]!;
    const label = target.replace(/\{([^{}]+)\}/g, (_, expression: string) => String(code.evaluate(expression)));
    expect(label).toBe(`${formatBytes(proposal.plan.target_free_bytes)} on Synthetic A`);
  });
});

function alerts() {
  const report = { ...alertReport, settings: { ...alertReport.settings, growth_bytes: String(10 * 1024 ** 3) }, defaults: { ...alertReport.defaults, growth_bytes: String(10 * 1024 ** 3) } };
  const call = vi.fn(async (command: string, settings?: unknown) => ({ result: command === 'alerts.configure' ? { ...report, settings } : report }));
  const state = {
    session: { client: { call }, epoch: 0, handle: (e: Error) => e.message },
    untrack, formFrom, settingsFrom, report: null as typeof alertReport | null,
    form: null as ReturnType<typeof formFrom> | null, generation: 0,
    loading: false, saving: false, error: '', notice: '', invalidField: '',
  };
  return { state, call, code: viewScript('alerts', state, 'session.epoch') };
}

describe('Alert threshold edits survive refresh', () => {
  it('preserves edits across scan invalidation and saves the edited threshold', async () => {
    const { state, call, code } = alerts();
    await code.load();
    expect(state.form?.growthGib).toBe('10');
    state.form!.growthGib = '20';
    code.effect();
    await vi.waitFor(() => expect(state.loading).toBe(false));
    expect(state.form?.growthGib).toBe('20');
    await code.save();
    expect(call).toHaveBeenLastCalledWith('alerts.configure', expect.objectContaining({ growth_bytes: String(20 * 1024 ** 3) }));
    expect(state.notice).toContain('Thresholds saved');
    code.reset();
    expect(state.form?.growthGib).toBe('10');
  });

  it('retains edits after a failed refresh and recovery', async () => {
    const { state, call, code } = alerts();
    await code.load();
    state.form!.growthGib = '20';
    call.mockRejectedValueOnce(new Error('Synthetic offline report'));
    await code.load();
    expect(state.report).toBeNull();
    expect(state.form?.growthGib).toBe('20');
    expect(state.error).toBe('Synthetic offline report');
    await code.load();
    expect(state.form?.growthGib).toBe('20');
  });
});

describe('Health resources refresh independently', () => {
  it.each(['health.get', 'roots.list', 'grants.list', 'scans'] as const)('applies revocation and invalidates only the failed %s resource', async failed => {
    const revokedRoots = { roots: [{ root_id: 'root_a', grant_state: 'revoked' }] };
    const revokedGrants = { grants: [{ grant_id: 'grant_a', revoked_at: '2026-10-17T12:00:00Z' }] };
    const freshHealth = { observed_at: '2026-10-17T12:00:00Z' };
    const state = {
      session: { client: { call: vi.fn(async (command: string) => {
        if (command === failed) throw new Error(`Unavailable: ${failed}`);
        return { result: command === 'roots.list' ? revokedRoots : command === 'grants.list' ? revokedGrants : freshHealth };
      }) }, epoch: 0, handle: (e: Error) => e.message },
      scans: { sync: vi.fn(async () => { if (failed === 'scans') throw new Error('Unavailable: scans'); }) },
      untrack, ticket: 0, alive: true, loadError: '',
      roots: { roots: [{ root_id: 'root_a', grant_state: 'active' }] } as typeof revokedRoots | null,
      grants: { grants: [{ grant_id: 'grant_a', revoked_at: null }] } as { grants: { grant_id: string; revoked_at: string | null }[] } | null,
      health: { observed_at: '2026-10-16T12:00:00Z' } as typeof freshHealth | null,
    };
    const code = viewScript('health', state, 'session.epoch');
    await code.load();
    expect(state.roots).toEqual(failed === 'roots.list' ? null : revokedRoots);
    expect(state.grants).toEqual(failed === 'grants.list' ? null : revokedGrants);
    expect(state.health).toEqual(failed === 'health.get' ? null : freshHealth);
    expect(state.loadError).toBe(`Unavailable: ${failed}`);
  });
});

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
    route: { search: '' },
    session: { client: { call }, epoch: 0, handle: (e: Error) => e.message },
    untrack, tick: async () => {}, document: { activeElement: null }, HTMLElement: class {},
    LoomwardError, fallbackStep, onRefetchError, resolveTrail,
    PAGE: 50, gen: 0, startsRequest: 0, inspection: 0, abort: null, inspectAbort: null, pageGen: null,
    starts: [] as { id: string; name: string }[], trail: [] as { id: string; name: string }[],
    get here() { return this.trail.at(-1); },
    rows: [] as typeof searchExample.hits[number]['row'][], found: new Map(), seenIds: new Set(),
    searching: false, query: '', extension: '', minBytes: '', kind: 'any', basis: 'logical', sort: 'size_desc',
    next: null, total: null, error: '', busy: false,
    notice: '', gone: null as { lost: string; now: string | null } | null,
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
    expect(call.mock.calls.map(args => args[0]).filter(c => c !== 'node.inspect').at(-1)).toBe(searching ? 'search.query' : 'tree.children'); // the root binding read is background bookkeeping
  });

  const inspected = (path: string) => ({ display_path: { text: path, truncated: false } });

  it('binds the root it lands on to its granted path, so a restart that re-mints every ID still restores the trail (#198)', async () => {
    const afterRestart = { id: 'root_after_restart', name: 'Synthetic A' }, other = { id: 'root_other', name: 'Synthetic A' };
    let restarted = false;
    const call = vi.fn(async (command: string, req: { node_id?: string }) => ({ result: command === 'tree.slice' ? roots(...(restarted ? [other, afterRestart] : [rootA]))
      : command === 'node.inspect' ? inspected(req.node_id === other.id ? 'synthetic/other' : 'synthetic/a')
      : { ...children, items: [{ node_id: restarted ? 'child_after' : 'child_a', name: 'Synthetic child', kind: 'dir' }] } }));
    const { state, code } = explorer(call);
    state.trail = [{ ...rootA }];
    await code.show();
    await vi.waitFor(() => expect(state.trail[0]).toMatchObject({ root: 'synthetic/a' })); // fails if the binding call is removed
    state.trail = [...state.trail, { id: 'child_a', name: 'Synthetic child' }];
    restarted = true;
    expect(await code.loadStarts({ lost: 'Synthetic child', saved: [...state.trail], focus: null })).toBe(true);
    expect(state.trail.map(c => c.id)).toEqual([afterRestart.id, 'child_after']);
    expect(state.notice).toContain('location was restored');
    expect(state.gone).toBeNull();
  });

  it('does not tell the reader a different root of the same name was restored (#198)', async () => {
    const twin = { id: 'root_twin', name: 'Synthetic A' };
    const call = vi.fn(async (command: string, req: { node_id?: string }) => ({ result: command === 'tree.slice' ? roots(twin)
      : command === 'node.inspect' ? inspected('synthetic/twin') : { ...children, items: req.node_id === twin.id ? [{ node_id: 'child_t', name: 'Synthetic child', kind: 'dir' }] : [] } }));
    const { state, code } = explorer(call);
    const saved = [{ ...rootA, root: 'synthetic/a' }, { id: 'child_a', name: 'Synthetic child' }];
    expect(await code.loadStarts({ lost: 'Synthetic child', saved, focus: null })).toBe(true);
    expect(state.trail.map(c => c.id)).toEqual([twin.id]); // the first start, not a restored location
    expect(state.notice).toBe('');
    expect(state.gone).toEqual({ lost: 'Synthetic child', now: null });
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
    fail = false;
    code.reload(); // the "Try again" button
    await vi.waitFor(() => expect(state.starts).toEqual([rootA, rootB]));
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

describe('Explorer linked node is applied once per value (#1082)', () => {
  it('does not re-inspect a linked node after a reconnect, but applies a different or re-followed link', async () => {
    const call = vi.fn(async (command: string) => ({ result: command === 'tree.slice' ? roots(rootA)
      : command === 'node.inspect' ? { display_path: { text: 'synthetic/a', truncated: false } } : children }));
    // The root binding also reads node.inspect for the starting root (background bookkeeping): count only linked nodes.
    const inspectedNodes = () => (call.mock.calls as unknown as Array<[string, { node_id: string }]>)
      .filter(args => args[0] === 'node.inspect' && args[1].node_id.startsWith('nd_')).map(args => args[1].node_id);
    const settle = () => new Promise(resolve => setTimeout(resolve, 10));
    const { state, code } = explorer(call);
    state.route.search = 'node=nd_a';
    code.effect();
    await vi.waitFor(() => expect(inspectedNodes()).toEqual(['nd_a']));
    await settle();
    state.session.epoch++; // reconnect: the reader may have inspected something else since
    code.effect();
    await settle();
    expect(inspectedNodes()).toEqual(['nd_a']);
    state.route.search = 'node=nd_b';
    code.effect();
    await vi.waitFor(() => expect(inspectedNodes()).toEqual(['nd_a', 'nd_b']));
    state.route.search = '';
    code.effect();
    await settle();
    state.route.search = 'node=nd_b';
    code.effect();
    await vi.waitFor(() => expect(inspectedNodes()).toEqual(['nd_a', 'nd_b', 'nd_b']));
  });
});

type TodayCode = { load(): Promise<void>; loadReclaim(ids: string[]): Promise<void>; effect(): void; onEvent(ev: { event: string }): void };

function today(call: ReturnType<typeof vi.fn>) {
  const state = {
    session: { client: { call }, epoch: 0, connectionEpoch: 0, handle: (e: Error) => e.message },
    scans: { settled: 0 }, untrack, alive: true, ticket: 0, reclaimTicket: 0, started: false, timer: undefined,
    JOBS_LIMIT: 100, REFRESH_MS: 250,
    roots: null as unknown, alerts: null as unknown, jobs: null as unknown, reclaim: new Map<string, unknown>(), failedReclaim: 0,
    loadProblems: [] as string[], reclaimProblems: [] as string[], loaded: false, refreshed: 0,
  };
  return { state, code: viewScript('today', state, 'session.epoch') as unknown as TodayCode };
}

const todayReply = (root: string) => async (command: string) => ({
  result: command === 'roots.list' ? { roots: [{ root_id: root }] } : command === 'jobs.list' ? { jobs: [] } : { items: [] },
});

describe('Today loads', () => {
  it('lets a stale load never overwrite a newer one', async () => {
    const old = deferred<unknown>();
    const call = vi.fn(todayReply('rt_new'));
    call.mockImplementationOnce(() => old.promise as never); // the first load's roots.list is still in flight
    const { state, code } = today(call);
    const older = code.load();
    await code.load();
    expect(state.roots).toEqual([{ root_id: 'rt_new' }]);
    old.resolve({ result: { roots: [{ root_id: 'rt_old' }] } });
    await older;
    expect(state.roots).toEqual([{ root_id: 'rt_new' }]);
    expect(state.refreshed).toBe(1);
  });

  it('drops a load that finishes after the connection changed', async () => {
    const reply = deferred<unknown>();
    const { state, code } = today(vi.fn(() => reply.promise as never));
    const loading = code.load();
    state.session.connectionEpoch++;
    reply.resolve({ result: { roots: [], jobs: [], items: [] } });
    await loading;
    expect(state.loaded).toBe(false);
    expect(state.roots).toBeNull();
  });

  it('lets a stale reclaim read never overwrite a newer one', async () => {
    const old = deferred<unknown>();
    const call = vi.fn(async () => ({ result: 'new_plan' }));
    call.mockImplementationOnce(() => old.promise as never);
    const { state, code } = today(call);
    const older = code.loadReclaim(['vo_a']);
    await code.loadReclaim(['vo_a']);
    old.resolve({ result: 'old_plan' });
    await older;
    expect([...state.reclaim]).toEqual([['vo_a', 'new_plan']]);
  });

  it('drops a reclaim read that finishes after the connection changed, though the client object is the same', async () => {
    const reply = deferred<unknown>();
    const { state, code } = today(vi.fn(() => reply.promise as never));
    const loading = code.loadReclaim(['vo_a']);
    state.session.connectionEpoch++;
    reply.resolve({ result: 'plan_from_the_old_engine' });
    await loading;
    expect(state.reclaim.size).toBe(0);
  });

  it('keeps reading during a long burst: job.state every 100 ms for 2 s still reads once per window', () => {
    vi.useFakeTimers();
    try {
      const call = vi.fn(todayReply('rt_a'));
      const { code } = today(call);
      code.effect();
      expect(call).toHaveBeenCalledTimes(3);
      for (let t = 0; t < 2000; t += 100) {
        code.onEvent({ event: 'job.state' });
        vi.advanceTimersByTime(100);
      }
      const reads = (call.mock.calls.length - 3) / 3; // reads after the first, while the burst was still running
      expect(reads).toBeGreaterThanOrEqual(5); // a trailing debounce would have read 0 times
      expect(reads).toBeLessThanOrEqual(8); // about one per window, not one per event (19)
    } finally {
      vi.useRealTimers();
    }
  });

  it('replaces reclaim errors each round instead of piling them up', async () => {
    const call = vi.fn(async () => { throw new Error('plan unreadable'); });
    const { state, code } = today(call);
    await code.loadReclaim(['vo_a']);
    await code.loadReclaim(['vo_a']);
    expect(state.reclaimProblems).toEqual(['plan unreadable']);
    call.mockImplementation(async () => ({ result: 'plan' }) as never);
    await code.loadReclaim(['vo_a']);
    expect(state.reclaimProblems).toEqual([]);
  });

  it('reads at once, then coalesces a burst of scan-end triggers into one more read', () => {
    vi.useFakeTimers();
    try {
      const call = vi.fn(todayReply('rt_a'));
      const { state, code } = today(call);
      code.effect();
      expect(call).toHaveBeenCalledTimes(3); // roots, alerts, jobs: the first read does not wait
      state.scans.settled++;
      code.effect(); // scans.settled
      code.onEvent({ event: 'job.state' }); // the same scan ending
      code.onEvent({ event: 'scan.progress' }); // not a trigger
      code.onEvent({ event: 'job.state' });
      expect(call).toHaveBeenCalledTimes(3);
      vi.advanceTimersByTime(249);
      expect(call).toHaveBeenCalledTimes(3);
      vi.advanceTimersByTime(1);
      expect(call).toHaveBeenCalledTimes(6); // one more read for the whole burst
      vi.advanceTimersByTime(1000);
      expect(call).toHaveBeenCalledTimes(6);
    } finally {
      vi.useRealTimers();
    }
  });
});
