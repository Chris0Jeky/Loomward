import { readFileSync } from 'node:fs';
import ts from 'typescript';
import { describe, expect, it, vi } from 'vitest';
import { LoomwardError } from '../../src/lib/transport/client';
import { ReviewQueue } from '../../src/views/review/queue.svelte';
import { fallbackStep, onRefetchError, resolveTrail } from '../../src/views/explorer/refetch';
import searchExample from '../../../contracts/v3/examples/commands/search.query.result.json';
import learningStatus from '../../../contracts/v3/examples/commands/learning.status.result.json';
import learningQueue from '../../../contracts/v3/examples/commands/learning.queue.result.json';
import alertReport from '../../../contracts/v3/examples/commands/alerts.list.result.json';
import proposal from '../../../contracts/v3/examples/commands/proposals.get.result.json';
import { formFrom, settingsFrom } from '../../src/views/alerts/model';
import { formatBytes, parseBytes } from '../../src/lib/format/bytes';
import { formatCount } from '../../src/lib/format/time';
import { isSound } from '../../src/views/tiers/plan';
import { planFind } from '../../src/lib/search/find';
import { reserveBytes, reserveDraft } from '../../src/views/tiers/reserve';
import rootList from '../../../contracts/v3/examples/commands/roots.list.result.json';
import type { Root } from '../../src/lib/contracts.gen';
import type { BackupEvidence } from '../../src/lib/contracts.gen';
import * as backupModel from '../../src/views/backups/model';
import backupList from '../../../contracts/v3/examples/commands/backups.list.result.json';

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
    commit(): boolean;
    clearSearch(): void;
    preset(text: string): void;
    loadStarts(recovery?: unknown, refreshOnly?: boolean): Promise<boolean>;
    effect(): void;
    reload(): Promise<void>;
    refresh(): Promise<void>;
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
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((r, fail) => { resolve = r; reject = fail; });
  return { promise, resolve, reject };
};

function backups(call: ReturnType<typeof vi.fn>) {
  const state = {
    ...backupModel, untrack, tick: async () => {},
    session: { client: { call }, epoch: 0, connectionEpoch: 0, info: { adapter: 'tauri' }, datasetClass: 'personal', handle: vi.fn((e: Error) => e.message) },
    alive: true, generation: 0, knownConnection: -1, knownClient: null as unknown,
    evidence: structuredClone(backupList.evidence) as BackupEvidence[], volumes: [] as unknown[],
    loading: false, error: '', notice: '', label: 'synthetic-nas', snapshot: '1'.repeat(64),
    source: '', target: '', wanted: `fixture/ledger.ods sha256:${'a'.repeat(64)}`, integrity: true,
    importing: false, forgetting: '', updating: '', invalidField: '', heading: { focus: vi.fn() }, importHeading: { focus: vi.fn() },
    get desktop(): boolean { return this.session.info.adapter === 'tauri'; },
    get personal(): boolean { return this.session.datasetClass === 'personal'; },
    get busy(): boolean { return this.loading || this.importing || !!this.forgetting; },
    get parsed(): ReturnType<typeof backupModel.parseRequired> { return backupModel.parseRequired(this.wanted); },
    get formError(): string { return !backupModel.LABEL.test(this.label) ? 'Invalid label' : !backupModel.SNAPSHOT.test(this.snapshot) ? 'Invalid snapshot' : this.parsed.errors[0] ?? ''; },
  };
  const code = viewScript('backups', state, 'session.epoch') as ReturnType<typeof viewScript> & {
    importReceipt(): Promise<void>;
    forget(e: BackupEvidence): Promise<void>;
    beginUpdate(e: BackupEvidence): Promise<void>;
    newImport(): Promise<void>;
  };
  return { state, code };
}

describe('Backups session boundaries', () => {
  it('prefills exact stored values without a call, clears volume choices, and exits to a blank new import', async () => {
    const call = vi.fn();
    const { state, code } = backups(call);
    const record = state.evidence[0]!;
    const original = structuredClone(state.evidence);
    state.source = 'vo_previous_source'; state.target = 'vo_previous_target'; state.integrity = false;
    await code.beginUpdate(record);
    expect(state.label).toBe(record.repository_label);
    expect(state.updating).toBe(record.repository_label);
    expect(state.snapshot).toBe(record.snapshot_id);
    expect(state.parsed).toEqual({ required: record.required_versions, errors: [] });
    expect([state.source, state.target]).toEqual(['', '']);
    expect(state.integrity).toBe(true);
    expect(state.importHeading.focus).toHaveBeenCalledOnce();
    expect(state.evidence).toEqual(original);
    expect(call).not.toHaveBeenCalled();
    await code.newImport();
    expect([state.updating, state.label, state.snapshot, state.wanted, state.source, state.target]).toEqual(['', '', '', '', '', '']);
    expect(state.evidence).toEqual(original);
  });

  it('refuses a lossy prefill and preserves the existing form and evidence', async () => {
    const { state, code } = backups(vi.fn());
    const record = state.evidence[0]!;
    const before = [state.label, state.snapshot, state.wanted];
    await code.beginUpdate({ ...record, required_versions: [{ relative_name: ' leading.txt', version: `sha256:${'a'.repeat(64)}` }] });
    expect(state.error).toContain('without changing');
    expect([state.label, state.snapshot, state.wanted]).toEqual(before);
    expect(state.updating).toBe('');
    expect(state.session.client.call).not.toHaveBeenCalled();
  });

  it.each(['cancelled_by_user', 'refused', 'error'])('preserves existing evidence on %s and sends the fixed label with no path', async (outcome) => {
    const call = vi.fn(async () => {
      if (outcome === 'error') throw new Error('receipt transport failed');
      return { result: { outcome, refusal: outcome === 'refused' ? 'too_large' : null } };
    });
    const { state, code } = backups(call);
    const record = state.evidence[0]!;
    const original = structuredClone(state.evidence);
    await code.beginUpdate(record);
    state.label = 'other-valid-label'; // even a changed DOM binding must not redirect the replacement
    await code.importReceipt();
    expect(call).toHaveBeenCalledExactlyOnceWith('backups.import', {
      repository_label: record.repository_label, snapshot_id: record.snapshot_id, source_volume_id: null, target_volume_id: null,
      required: record.required_versions, integrity_check: true,
    }, { deadlineMs: 60000 });
    expect(state.evidence).toEqual(original);
    expect(state.importing).toBe(false);
    expect(state.updating).toBe(record.repository_label);
    if (outcome === 'cancelled_by_user') expect(state.notice).toContain('Nothing was stored');
    else expect(state.error).not.toBe('');
  });

  it.each(['loading', 'importing', 'forgetting'] as const)('blocks overlapping form and mutation actions while %s', async (pending) => {
    const { state, code } = backups(vi.fn());
    const before = [state.label, state.snapshot, state.wanted, state.updating];
    if (pending === 'forgetting') state.forgetting = 'be_pending'; else state[pending] = true;
    await code.beginUpdate(state.evidence[0]!);
    await code.newImport();
    await code.importReceipt();
    await code.forget(state.evidence[0]!);
    expect(state.session.client.call).not.toHaveBeenCalled();
    expect([state.label, state.snapshot, state.wanted, state.updating]).toEqual(before);
  });

  it.each(['http', 'synthetic'])('does not enter update or import in a %s session', async (mode) => {
    const { state, code } = backups(vi.fn());
    if (mode === 'http') state.session.info.adapter = 'http'; else state.session.datasetClass = 'synthetic';
    await code.beginUpdate(state.evidence[0]!);
    await code.importReceipt();
    expect(state.updating).toBe('');
    expect(state.session.client.call).not.toHaveBeenCalled();
  });

  it('keeps a draft on ordinary refresh but clears personal state when the connection changes', async () => {
    const never = deferred<unknown>();
    const { state, code } = backups(vi.fn(() => never.promise));
    state.knownClient = state.session.client; state.knownConnection = 0;
    await code.beginUpdate(state.evidence[0]!);
    const before = [state.label, state.snapshot, state.wanted, state.updating];
    state.session.epoch++;
    code.effect();
    expect([state.label, state.snapshot, state.wanted, state.updating]).toEqual(before);
    state.session.connectionEpoch++; state.session.epoch++;
    state.importing = true; state.forgetting = 'be_pending';
    code.effect();
    expect([state.label, state.snapshot, state.wanted, state.updating]).toEqual(['', '', '', '']);
    expect(state.evidence).toEqual([]);
    expect(state.volumes).toEqual([]);
    expect(state.importing).toBe(false);
    expect(state.forgetting).toBe('');
  });

  it('does not populate volumes from the previous connection', async () => {
    const reply = deferred<unknown>();
    const call = vi.fn(async (command: string) => command === 'volumes.list' ? reply.promise : { result: backupList });
    const { state, code } = backups(call);
    const loading = code.load();
    await Promise.resolve();
    expect(call).toHaveBeenCalledWith('volumes.list', {});
    state.session.connectionEpoch++;
    reply.resolve({ result: { volumes: [{ volume_id: 'vo_old' }] } });
    await loading;
    expect(state.volumes).toEqual([]);
  });

  it('ignores an import reply after the connection changes, even when the client object is reused', async () => {
    const reply = deferred<unknown>();
    const call = vi.fn(async (command: string) => command === 'backups.import' ? reply.promise : { result: command === 'backups.list' ? backupList : { volumes: [] } });
    const { state, code } = backups(call);
    const importing = code.importReceipt();
    expect(call).toHaveBeenCalledTimes(1);
    state.session.connectionEpoch++;
    reply.resolve({ result: { outcome: 'imported', evidence: backupList.evidence[0], refusal: null } });
    await importing;
    expect(call).toHaveBeenCalledTimes(1);
    expect(state.notice).toBe('');
    expect(state.session.handle).not.toHaveBeenCalled();
  });

  it.each(['backups.list', 'backups.import', 'backups.forget'])('ignores late %s errors after unmount or reconnect', async (command) => {
    const reply = deferred<unknown>();
    const { state, code } = backups(vi.fn(() => reply.promise));
    const pending = command === 'backups.list' ? code.load() : command === 'backups.import' ? code.importReceipt() : code.forget(state.evidence[0]!);
    state.session.connectionEpoch++; state.alive = false;
    reply.reject(new Error('old connection failed'));
    await pending;
    expect(state.error).toBe('');
    expect(state.notice).toBe('');
    expect(state.session.handle).not.toHaveBeenCalled();
    expect(state.session.client.call).toHaveBeenCalledTimes(1);
  });

  it('does not refetch or announce a late forget in a new connection', async () => {
    const reply = deferred<unknown>();
    const { state, code } = backups(vi.fn(() => reply.promise));
    const forgetting = code.forget(state.evidence[0]!);
    state.session.connectionEpoch++;
    reply.resolve({ result: {} });
    await forgetting;
    expect(state.session.client.call).toHaveBeenCalledTimes(1);
    expect(state.notice).toBe('');
    expect(state.heading.focus).not.toHaveBeenCalled();
  });
});

function review(call: (command: string, payload: unknown) => Promise<{ result: unknown }>) {
  const state = {
    session: { client: { call: async (command: string, payload: unknown) => ({ ...await call(command, payload), meta: { state_rev: '1' } }) }, epoch: 0, handle: (e: Error) => e.message },
    untrack, LoomwardError, alive: true, generation: 0,
    status: learningStatus as typeof learningStatus | null,
    review: new ReviewQueue(), handle: (e: Error) => e.message,
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
    expect(state.review.page).toEqual(learningQueue);
    expect(state.error).toBe('');
    expect(state.selected).toBe('');
    expect(call.mock.calls.map(args => args[0])).toEqual(['learning.status', 'learning.queue', 'learning.queue', 'learning.status']);
    expect((call.mock.calls.at(-2) as unknown as [string, { cursor: string | null }])[1].cursor).toBeNull();
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
    expect(state.review.page).toBeNull();
    expect(state.review.problem).toBe('Learning unavailable.');
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
    state.review.clear();
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
    expect(state.review.page).toBeNull();
    expect(state.review.problem).toContain('Queue exceeds');
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
  it('opens a historical scenario while a tier change waits for candidates', async () => {
    const candidates = deferred<{ result: { groups: never[] } }>();
    const historical = { ...proposal, summary: { ...proposal.summary, source_volume_id: 'vol_a', status: 'stale' } };
    const volumes = [
      { volume_id: 'vol_a', source_eligible: false, declared_reserve_bytes: null },
      { volume_id: 'vol_b', source_eligible: true, declared_reserve_bytes: null },
    ];
    const call = vi.fn(async (command: string) => {
      if (command === 'volumes.declare_tier') return { result: {} };
      if (command === 'tiers.model') return { result: { volumes }, meta: { state_rev: '8' } };
      if (command === 'placement.candidates') return candidates.promise;
      if (command === 'proposals.get') return { result: historical };
      if (command === 'proposals.list') return { result: { proposals: [historical.summary] } };
      throw new Error(`Unexpected command: ${command}`);
    });
    const state = {
      session: { client: { call }, handle: (e: Error) => e.message }, untrack, reserveDraft,
      model: { volumes }, source: 'vol_b', src: volumes[1], volume: (id: string) => volumes.find(v => v.volume_id === id),
      reserves: {},
      modelStateRev: '7', modelTicket: 0, savedTicket: 0, proposalsTicket: 0, candTicket: 0, simTicket: 0,
      loading: false, declaring: false, reserving: false, simulating: false,
      savedDetail: null, savedError: '', saved: [historical.summary], outcomes: null,
      assume: {}, error: '', say: { say: vi.fn() },
    };
    const code = viewScript('tiers', state, 'session.epoch');
    const changing = code.evaluate("declareTier('vol_b', '2')");
    await vi.waitFor(() => expect(call).toHaveBeenCalledWith('placement.candidates', expect.anything()));
    expect(state.loading).toBe(true);
    expect(state.declaring).toBe(true);
    await code.loadSaved(historical.summary.proposal_id);
    expect(state.savedDetail).toEqual(historical);
    candidates.resolve({ result: { groups: [] } });
    await changing;
    expect(state.loading).toBe(false);
    expect(state.declaring).toBe(false);
    expect(state.savedDetail).toEqual(historical);
  });

  it('blocks saved reads throughout an uncertain reserve write and recovery', async () => {
    const reply = deferred<never>();
    const recovery = deferred<never>();
    const call = vi.fn((command: string) => {
      if (command === 'volumes.declare_reserve') return reply.promise;
      if (command === 'tiers.model') return recovery.promise;
      return Promise.resolve({ result: proposal });
    });
    const state = {
      session: { client: { call }, handle: (e: Error) => e.message }, untrack, reserveBytes, reserveDraft,
      modelStateRev: '7', reserves: { vo_target: { value: '60', unit: 'GiB' } },
      model: {}, modelTicket: 0, savedTicket: 0, proposalsTicket: 0, loading: false, declaring: false, reserving: false, simulating: false,
      outcomes: { old: true }, savedDetail: proposal, saved: [proposal.summary],
      error: '', say: { say: vi.fn() },
    };
    const code = viewScript('tiers', state, 'session.epoch');
    const changing = code.evaluate("declareReserve('vo_target')");
    await code.loadSaved(proposal.summary.proposal_id);
    expect(call).toHaveBeenCalledTimes(1);
    expect(state.savedDetail).toBeNull();
    reply.reject(new Error('Reply lost'));
    await vi.waitFor(() => expect(call).toHaveBeenCalledWith('tiers.model', {}));
    await code.loadSaved(proposal.summary.proposal_id);
    expect(call).toHaveBeenCalledTimes(2);
    recovery.reject(new Error('Recovery unavailable'));
    await changing;
    expect(state.reserving).toBe(false);
    expect(state.savedDetail).toBeNull();
  });

  it.each(['during submission', 'after failed recovery'])('drops a pre-reserve scenario list arriving %s', async arrival => {
    const list = deferred<{ result: { proposals: typeof proposal.summary[] } }>();
    const reply = deferred<never>();
    const call = vi.fn((command: string) => {
      if (command === 'proposals.list') return list.promise;
      if (command === 'volumes.declare_reserve') return reply.promise;
      return Promise.reject(new Error('Current preferences unavailable'));
    });
    const state = {
      session: { client: { call }, handle: (e: Error) => e.message }, untrack, reserveBytes, reserveDraft,
      modelStateRev: '7', reserves: { vo_target: { value: '60', unit: 'GiB' } },
      model: {}, modelTicket: 0, savedTicket: 0, proposalsTicket: 0, loading: false, declaring: false, reserving: false, simulating: false,
      outcomes: { old: true }, savedDetail: proposal, saved: [proposal.summary], savedError: '',
      error: '', say: { say: vi.fn() },
    };
    const code = viewScript('tiers', state, 'session.epoch');
    const listing = code.evaluate('loadProposals()');
    const changing = code.evaluate("declareReserve('vo_target')");
    if (arrival === 'after failed recovery') {
      reply.reject(new Error('Reply lost'));
      await changing;
    }
    list.resolve({ result: { proposals: [proposal.summary] } });
    await listing;
    expect(state.saved).toEqual([]);
    if (arrival === 'during submission') {
      reply.reject(new Error('Reply lost'));
      await changing;
    }
    expect(state.saved).toEqual([]);
    expect(state.modelStateRev).toBeNull();
  });

  it.each(['committed', 'deadline_exceeded', 'stale_generation'] as const)('hides old plans after a %s reserve outcome when the model refresh fails', async outcome => {
    const call = vi.fn(async (command: string) => {
      if (command === 'volumes.declare_reserve') {
        if (outcome !== 'committed') throw new LoomwardError({ code: outcome, message: outcome, retryable: false, detail: null });
        return { result: { reserve_bytes: '64424509440' } };
      }
      throw new Error('Current preferences unavailable');
    });
    const state = {
      session: { client: { call }, handle: (e: Error) => e.message }, untrack, reserveBytes, reserveDraft,
      modelStateRev: '7', reserves: { vo_target: { value: '60', unit: 'GiB' } },
      model: {}, modelTicket: 0, savedTicket: 0, proposalsTicket: 0, loading: false, declaring: false, reserving: false, simulating: false,
      outcomes: { old: true }, savedDetail: proposal, saved: [proposal.summary],
      error: '', say: { say: vi.fn() },
    };
    const code = viewScript('tiers', state, 'session.epoch');
    await code.evaluate("declareReserve('vo_target')");
    expect(state.savedDetail).toBeNull();
    expect(state.outcomes).toBeNull();
    expect(state.saved).toEqual([]);
    expect(state.modelStateRev).toBeNull();
    expect(state.error).toBe(outcome === 'committed' ? 'Current preferences unavailable' : outcome);
    expect(state.say.say).not.toHaveBeenCalledWith(expect.stringContaining('have been refreshed'));
    if (outcome === 'committed') expect(state.say.say).toHaveBeenCalledWith(expect.stringContaining('recorded'));
    else expect(state.say.say).not.toHaveBeenCalled();
  });

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

describe('Tiers volume preference notifications', () => {
  function tiers() {
    const target = { volume_id: 'vo_target', display_name: 'Target', source_eligible: true, declared_reserve_bytes: '85899345920' };
    const historical = { ...proposal, summary: { ...proposal.summary, source_volume_id: 'vo_target', status: 'stale' } };
    const call = vi.fn(async (command: string) => ({
      result: command === 'tiers.model' ? { volumes: [{ ...target, declared_reserve_bytes: '64424509440' }] }
        : command === 'placement.candidates' ? { groups: [] }
        : command === 'proposals.list' ? { proposals: [historical.summary] } : historical,
      meta: { state_rev: '8' },
    }));
    const state = {
      session: { client: { call }, connectionEpoch: 1, handle: (e: Error) => e.message }, untrack, reserveDraft,
      model: { volumes: [target] }, modelStateRev: '7', reserves: { vo_target: reserveDraft('85899345920') },
      source: 'vo_target', src: target, assume: {}, savedDetail: proposal as typeof proposal | null, saved: [proposal.summary],
      outcomes: { old: true } as { old: boolean } | null, loading: false, declaring: false, simulating: false,
      modelTicket: 0, savedTicket: 0, proposalsTicket: 0, candTicket: 0, simTicket: 0, cands: null,
      preferenceEvent: undefined as { state_rev: string | null } | undefined,
      preferenceSavedId: null as string | null,
      preferenceNotice: '', error: '', savedError: '', say: { say: vi.fn() },
      volume: () => target,
    };
    return { state, call, historical, code: viewScript('tiers', state, 'session.epoch') };
  }

  it('refreshes another client’s reserve and open saved scenario while retaining an unsaved draft', async () => {
    const { state, call, historical, code } = tiers();
    state.reserves.vo_target.value = '75';
    code.evaluate("onVolumeEvent({ event: 'volumes.changed', state_rev: '8' })");
    await code.evaluate('refreshVolumePreferences()');
    expect(state.model.volumes[0]!.declared_reserve_bytes).toBe('64424509440');
    expect(state.modelStateRev).toBe('8');
    expect(state.reserves.vo_target).toEqual({ value: '75', unit: 'GiB' });
    expect(state.savedDetail).toEqual(historical);
    expect(state.outcomes).toBeNull();
    expect(state.preferenceNotice).toContain('unsaved');
    expect(call.mock.calls.every(([command]) => !command.startsWith('volumes.declare'))).toBe(true);
  });

  it('updates untouched drafts, defers notifications during a save, and reuses a model already covering the event', async () => {
    const { state, call, code } = tiers();
    state.declaring = true;
    code.evaluate("onVolumeEvent({ event: 'volumes.changed', state_rev: '8' })");
    await code.evaluate('refreshVolumePreferences()');
    expect(call).not.toHaveBeenCalled();
    state.declaring = false;
    await code.evaluate('refreshVolumePreferences()');
    expect(state.reserves.vo_target).toEqual({ value: '60', unit: 'GiB' });
    call.mockClear();
    code.evaluate("onVolumeEvent({ event: 'volumes.changed', state_rev: '8' })");
    await code.evaluate('refreshVolumePreferences()');
    expect(call.mock.calls.some(([command]) => command === 'tiers.model')).toBe(false);
    expect(call.mock.calls.every(([command]) => !command.startsWith('volumes.declare'))).toBe(true);
  });

  it.each(['client', 'connection'] as const)('hides stale results immediately and drops a refresh from a replaced %s', async changed => {
    const { state, call, code } = tiers();
    const pending = deferred<Awaited<ReturnType<typeof call>>>();
    call.mockImplementationOnce(() => pending.promise);
    code.evaluate("onVolumeEvent({ event: 'volumes.changed', state_rev: '8' })");
    expect(state.savedDetail).toBeNull();
    expect(state.outcomes).toBeNull();
    expect(state.saved).toEqual([]);
    const refresh = code.evaluate('refreshVolumePreferences()');
    if (changed === 'client') state.session.client = { call: vi.fn() };
    else state.session.connectionEpoch++;
    pending.resolve({ result: { volumes: [] }, meta: { state_rev: '8' } } as never);
    await refresh;
    expect(state.model.volumes).toHaveLength(1);
    expect(state.modelStateRev).toBe('7');
    expect(call).toHaveBeenCalledTimes(1);
    expect(state.savedDetail).toBeNull();
  });

  it('does not reopen the old scenario when the reader chooses a source during refresh', async () => {
    const { state, call, code } = tiers();
    const pending = deferred<Awaited<ReturnType<typeof call>>>();
    call.mockImplementationOnce(() => pending.promise);
    code.evaluate("onVolumeEvent({ event: 'volumes.changed', state_rev: '8' })");
    const refresh = code.evaluate('refreshVolumePreferences()');
    code.evaluate("choose('vo_target')");
    pending.resolve({ result: state.model, meta: { state_rev: '8' } } as never);
    await refresh;
    expect(state.savedDetail).toBeNull();
    expect(call.mock.calls.some(([command]) => command === 'proposals.get')).toBe(false);
  });

  it('withholds an old saved list after a preference event', async () => {
    const { state, call, code } = tiers();
    const pending = deferred<Awaited<ReturnType<typeof call>>>();
    call.mockImplementationOnce(() => pending.promise);
    const read = code.evaluate('loadProposals()');
    code.evaluate("onVolumeEvent({ event: 'volumes.changed', state_rev: '8' })");
    pending.resolve({ result: { proposals: [proposal.summary] }, meta: { state_rev: '7' } } as never);
    await read;
    expect(state.saved).toEqual([]);
  });

  it('invalidates outcomes republished by a pending simulation even when its later model read covered the event', async () => {
    const { state, code } = tiers();
    state.savedDetail = null;
    state.simulating = true;
    code.evaluate("onVolumeEvent({ event: 'volumes.changed', state_rev: '8' })");
    state.modelStateRev = '8'; // simulation refreshes the model after its earlier plans finish
    state.outcomes = { old: true };
    state.simulating = false;
    await code.evaluate('refreshVolumePreferences()');
    expect(state.outcomes).toBeNull();
  });

  it.each(['plan', 'list'] as const)('invalidates an older displayed %s when its event arrives after a newer model read', async displayed => {
    const { state, code } = tiers();
    state.savedDetail = null;
    if (displayed === 'list') state.outcomes = null;
    state.modelStateRev = '8';
    code.evaluate("onVolumeEvent({ event: 'volumes.changed', state_rev: '8' })");
    expect(state.outcomes).toBeNull();
    await code.evaluate('refreshVolumePreferences()');
    expect(state.saved[0]?.status).toBe('stale');
  });

  it('keeps the dirty reserve draft when a saved simulation rereads another client’s changed preference', async () => {
    const { state, call, code } = tiers();
    const pending = deferred<Awaited<ReturnType<typeof call>>>();
    const read = call.getMockImplementation()!;
    call.mockImplementation(command => command === 'placement.simulate' ? pending.promise : read(command));
    Object.assign(state.src, { capacity_bytes: '214748364800' });
    Object.assign(state, {
      parseBytes, formatBytes, formatCount, isSound, pct: 20, transferGiB: 60,
      relief: 'entry_allocation_whatif', save: true,
      HEAT: ['unknown_is_ineligible', 'mtime_proxy_whatif', 'assumed_only'].map(id => ({ id, label: id })),
    });
    state.savedDetail = null;
    state.reserves.vo_target.value = '75';
    const simulation = code.evaluate('simulate()');
    code.evaluate("onVolumeEvent({ event: 'volumes.changed', state_rev: '8' })");
    pending.resolve({ result: proposal.plan, meta: { state_rev: '7' } } as never);
    await simulation;
    await code.evaluate('refreshVolumePreferences()');
    expect(state.model.volumes[0]!.declared_reserve_bytes).toBe('64424509440');
    expect(state.modelStateRev).toBe('8');
    expect(state.reserves.vo_target).toEqual({ value: '75', unit: 'GiB' });
    expect(state.preferenceNotice).toContain('unsaved');
    expect(state.outcomes).toBeNull();
  });
});

function alerts() {
  const report = { ...alertReport, settings: { ...alertReport.settings, growth_bytes: String(10 * 1024 ** 3) }, defaults: { ...alertReport.defaults, growth_bytes: String(10 * 1024 ** 3) } };
  const call = vi.fn(async (command: string, settings?: unknown) => ({ result: command === 'alerts.configure' ? { ...report, settings } : report }));
  const refreshResult = {
    text: '',
    say: vi.fn((text: string) => { refreshResult.text = text; }),
    clear: vi.fn(() => { refreshResult.text = ''; }),
  };
  const state = {
    session: { client: { call }, epoch: 0, connectionEpoch: 0, state: 'connected', handle: vi.fn((e: Error) => e.message) },
    untrack, formFrom, settingsFrom, report: null as typeof alertReport | null,
    form: null as ReturnType<typeof formFrom> | null, generation: 0,
    loading: false, saving: false, error: '', notice: '', invalidField: '', refreshResult,
    request: null as AbortController | null,
  };
  return { state, call, code: viewScript('alerts', state, 'session.epoch') };
}

describe('Alert threshold edits survive refresh', () => {
  it('offers a refresh control that reads checks without saving the draft', async () => {
    const { state, call, code } = alerts();
    await code.load();
    state.form!.growthGib = '20';
    state.form!.notifications = true;
    const draft = { ...state.form };
    call.mockClear();
    const source = readFileSync(new URL('../../src/views/alerts/View.svelte', import.meta.url), 'utf8');
    const handler = source.match(/onclick=\{([^}]+)\}>Refresh checks<\/button>/)?.[1];
    expect(handler, 'Alerts needs an explicit keyboard button to refresh the report').toBeDefined();
    (code.evaluate(`(${handler})`) as () => void)();
    await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));
    await vi.waitFor(() => expect(state.loading).toBe(false));
    expect(call.mock.calls.map(args => args[0])).toEqual(['alerts.list']);
    expect(state.form).toEqual(draft);
    expect(state.report?.settings.notifications_enabled).toBe(false);
    expect(state.notice).not.toContain('Thresholds saved');
    expect(state.refreshResult.text).toContain('recorded observations');
  });

  it('keeps the full draft separate from fresh persisted settings on repeated reads', async () => {
    const { state, call, code } = alerts();
    await code.load();
    state.form = { percent: '17', freeGib: '2.5', days: '14', growthGib: '20', notifications: true };
    const draft = { ...state.form };
    const fresh = { ...state.report!, evaluated_at: '2026-10-31T10:00:00Z', settings: { ...state.report!.settings, low_space_free_percent: 12 } };
    call.mockResolvedValue({ result: fresh });
    await code.refresh();
    await code.refresh();
    expect(state.report).toEqual(fresh);
    expect(state.form).toEqual(draft);
    expect(state.refreshResult.say).toHaveBeenCalledTimes(2);
    expect(state.refreshResult.clear).toHaveBeenCalledTimes(3);
    expect(call.mock.calls.map(args => args[0])).toEqual(['alerts.list', 'alerts.list', 'alerts.list']);
  });

  it('coalesces duplicate activation and refuses a save while a manual read is pending', async () => {
    const { state, call, code } = alerts();
    await code.load();
    const held = deferred<unknown>();
    call.mockImplementationOnce(() => held.promise as never);
    call.mockClear();
    const reading = code.refresh();
    await code.refresh();
    await code.save();
    expect(state.loading).toBe(true);
    expect(call.mock.calls.map(args => args[0])).toEqual(['alerts.list']);
    held.resolve({ result: state.report });
    await reading;
    expect(state.loading).toBe(false);
  });

  it('does not refresh while saving and permits a read once the save finishes', async () => {
    const { state, call, code } = alerts();
    await code.load();
    const held = deferred<unknown>();
    call.mockImplementationOnce(() => held.promise as never);
    call.mockClear();
    const saving = code.save();
    await code.refresh();
    expect(call.mock.calls.map(args => args[0])).toEqual(['alerts.configure']);
    held.resolve({ result: state.report });
    await saving;
    await code.refresh();
    expect(call.mock.calls.map(args => args[0])).toEqual(['alerts.configure', 'alerts.list']);
  });

  it('clears a failed report, retains the draft and only retries on explicit activation', async () => {
    const { state, call, code } = alerts();
    await code.load();
    state.form!.percent = '17';
    call.mockClear();
    call.mockRejectedValueOnce(new Error('Synthetic read failed'));
    await code.refresh();
    expect(state.report).toBeNull();
    expect(state.form!.percent).toBe('17');
    expect(state.error).toBe('Synthetic read failed');
    expect(state.refreshResult.text).toBe('');
    expect(state.loading).toBe(false);
    expect(call).toHaveBeenCalledTimes(1);
    await code.refresh();
    expect(call).toHaveBeenCalledTimes(2);
    expect(state.report).not.toBeNull();
    expect(state.form!.percent).toBe('17');
    expect(state.error).toBe('');
  });

  it.each(['epoch', 'connection', 'client', 'disconnect', 'unmount'] as const)('drops late refresh success/error/finally after %s', async invalidation => {
    for (const reject of [false, true]) {
      const { state, call, code } = alerts();
      const cleanup = code.effect() as unknown as () => void;
      await vi.waitFor(() => expect(state.loading).toBe(false));
      const held = deferred<unknown>();
      call.mockImplementationOnce(() => held.promise as never);
      const reading = code.refresh();
      const request = state.request!;
      if (invalidation === 'epoch') state.session.epoch++;
      if (invalidation === 'connection') state.session.connectionEpoch++;
      if (invalidation === 'client') state.session.client = { call: vi.fn() };
      if (invalidation === 'disconnect') state.session.state = 'unavailable';
      if (invalidation === 'unmount') { cleanup(); expect(request.signal.aborted).toBe(true); }
      const current = { ...state.report!, evaluated_at: '2026-10-31T11:00:00Z' };
      state.report = current;
      state.error = 'Current error';
      state.loading = true;
      state.refreshResult.say.mockClear();
      if (reject) held.reject(new Error('Old failure'));
      else held.resolve({ result: alertReport });
      await reading;
      expect(state.report).toBe(current);
      expect(state.error).toBe('Current error');
      expect(state.loading).toBe(true);
      expect(state.session.handle).not.toHaveBeenCalled();
      expect(state.refreshResult.say).not.toHaveBeenCalled();
    }
  });

  it('aborts an invalidated read and lets only its replacement complete the busy state', async () => {
    const { state, call, code } = alerts();
    await code.load();
    const old = deferred<unknown>(), fresh = deferred<unknown>();
    call.mockImplementationOnce(() => old.promise as never).mockImplementationOnce(() => fresh.promise as never);
    const older = code.refresh(), request = state.request!;
    state.session.epoch++;
    const newer = code.load();
    expect(request.signal.aborted).toBe(true);
    old.reject(new Error('Old failure'));
    await older;
    expect(state.loading).toBe(true);
    expect(state.error).toBe('');
    fresh.resolve({ result: alertReport });
    await newer;
    expect(state.loading).toBe(false);
    expect(state.report).toEqual(alertReport);
  });

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
    session: { client: { call }, epoch: 0, connectionEpoch: 0, state: 'connected', handle: (e: Error) => e.message },
    untrack, tick: async () => {}, document: { activeElement: null }, HTMLElement: class {},
    LoomwardError, fallbackStep, onRefetchError, resolveTrail, planFind,
    PAGE: 50, gen: 0, startsRequest: 0, inspection: 0, abort: null, inspectAbort: null, pageGen: null,
    starts: [] as { id: string; name: string }[], trail: [] as { id: string; name: string }[],
    get here() { return this.trail.at(-1); },
    rows: [] as typeof searchExample.hits[number]['row'][], found: new Map(), seenIds: new Set(),
    searching: false, query: '', extension: '', minBytes: '', kind: 'any', basis: 'logical', sort: 'size_desc',
    next: null as string | null, total: null, error: '', busy: false,
    notice: '', gone: null as { lost: string; now: string | null } | null,
    rootChoices: [] as Root[], rootChoice: '' as string | null, submittedRoot: null as Root | null,
    rootsRequest: 0, rootsAbort: null, rootsLoading: false, rootProblem: '', scopeBlocked: false,
    scopedSearchOpen: false, mountedConnection: 0, reconnectScopePending: false,
    selectionConnection: 0, selected: [] as { node_id: string; name: string }[],
  };
  return { state, code: viewScript('explorer', state, 'session.epoch') };
}

describe('Explorer applied Find plan', () => {
  it('freezes notices, filters and relative dates for paging, then replans on resubmit', async () => {
    const clock = vi.spyOn(Date, 'now').mockReturnValue(Date.UTC(2026, 10, 1));
    try {
      const call = vi.fn(async (_command: string, _request: Record<string, unknown>) => ({ result: { ...searchExample, next_cursor: 'next' } }));
      const { state, code } = explorer(call);
      state.query = 'report in:Tax modified:<1mo';
      expect(code.commit()).toBe(true);
      const shown = structuredClone(code.evaluate('shown'));
      await code.show();
      state.query = 'something else'; state.extension = 'zip'; state.minBytes = '1048576';
      clock.mockReturnValue(Date.UTC(2026, 10, 2));
      await code.show(true);
      expect(code.evaluate('shown')).toEqual(shown);
      expect(code.evaluate('shown.notices')).toContain('in:Tax is not supported yet, so it was not applied.');
      expect(call.mock.calls[1]![1]).toEqual({ ...call.mock.calls[0]![1], cursor: 'next' });
      state.query = 'report in:Tax modified:<1mo'; state.extension = ''; state.minBytes = '';
      expect(code.commit()).toBe(true);
      await code.show();
      expect(call.mock.calls[2]![1].modified_after).not.toBe(call.mock.calls[0]![1].modified_after);
    } finally { clock.mockRestore(); }
  });

  it('refuses contradictory bounds while retaining the previous applied plan', () => {
    const call = vi.fn(); const { state, code } = explorer(call);
    state.query = 'report'; expect(code.commit()).toBe(true);
    const previous = code.evaluate('shown');
    state.query = 'size:<1kb'; state.minBytes = '1048576';
    expect(code.commit()).toBe(false);
    expect(code.evaluate('findNotice')).toContain('minimum size exceeds the maximum');
    expect(code.evaluate('shown')).toBe(previous);
    expect(call).not.toHaveBeenCalled();
  });

  it('retains a rejected query-link notice after its initial folder listing', async () => {
    const call = vi.fn(async (command: string) => ({ result: command === 'roots.list' ? rootList
      : command === 'tree.slice' ? { nodes: [{ parent: 0, node_id: 'nd_start', name: 'Synthetic start' }] }
      : { generation: '1', items: [], next_cursor: null, total: 0, budget_hit: false } }));
    const { state, code } = explorer(call);
    state.route.search = 'q=in%3ATax'; code.effect();
    await vi.waitFor(() => expect(call).toHaveBeenCalledWith('tree.children', expect.anything(), expect.anything()));
    await vi.waitFor(() => expect(state.busy).toBe(false));
    expect(code.evaluate('findNotice')).toContain('no search was run');
    expect(call.mock.calls.some(([command]) => command === 'search.query')).toBe(false);
  });
});

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

const grantedA = { ...rootList.roots[0]!, root_id: 'rt_a', grant_state: 'active', display_path: { text: 'Synthetic A', truncated: false } } as Root;
const grantedB = { ...grantedA, root_id: 'rt_b', display_path: { text: 'Synthetic B', truncated: false } };

describe('Explorer submitted root scope', () => {
  it('freezes the chosen root for paging while a draft points at its same-named twin', async () => {
    const hit = searchExample.hits[0]!;
    const a = { ...hit, root_id: grantedA.root_id, row: { ...hit.row, node_id: 'nd_a', name: 'shared.pdf' } };
    const b = { ...a, root_id: grantedB.root_id, row: { ...a.row, node_id: 'nd_b' } };
    const call = vi.fn(async (_command: string, request: { root_id: string | null; cursor: string | null }) => ({
      result: { ...searchExample, hits: request.root_id === grantedA.root_id ? [a] : [a, b], next_cursor: request.cursor ? null : 'next' },
    }));
    const { state, code } = explorer(call);
    state.rootChoices = [grantedA, grantedB];
    state.rootChoice = grantedA.root_id;
    state.query = 'shared';
    expect(code.commit()).toBe(true);
    await code.show();
    expect(state.rows).toEqual([a.row]);
    state.rootChoice = grantedB.root_id;
    await code.show(true);
    expect(call.mock.calls.map(([, request]) => request.root_id)).toEqual(['rt_a', 'rt_a']);
    expect(state.submittedRoot).toEqual(grantedA);
    expect(call.mock.calls[1]![1].cursor).toBe('next');
    state.rootChoice = '';
    expect(code.commit()).toBe(true);
    await code.show();
    expect(state.rows).toEqual([a.row, b.row]);
    expect(state.submittedRoot).toBeNull();
  });

  const reply = (command: string, available = [grantedA, grantedB]) => ({ result: command === 'roots.list'
    ? { roots: available } : command === 'tree.slice' ? { nodes: [] } : searchExample });

  it.each([false, true])('refreshes a submitted search after root validation when Atlas fails (scoped=%s)', async scoped => {
    const pending = deferred<{ result: { roots: Root[] } }>();
    const call = vi.fn(async (command: string) => {
      if (command === 'tree.slice') throw new Error('Synthetic Atlas failure');
      if (command === 'roots.list') return pending.promise;
      return { result: searchExample };
    });
    const { state, code } = explorer(call);
    state.rootChoices = [grantedA]; state.rootChoice = scoped ? 'rt_a' : ''; state.query = 'report';
    expect(code.commit()).toBe(true);
    await code.show();
    const refresh = code.reload();
    expect(state.rows).toEqual([]);
    expect(call.mock.calls.filter(([command]) => command === 'search.query')).toHaveLength(1);
    pending.resolve({ result: { roots: [grantedA] } });
    await refresh;
    expect(state.starts).toEqual([]);
    expect(state.rows).toEqual(searchExample.hits.map(hit => hit.row));
    expect(state.error).toBe('');
    expect(state.busy).toBe(false);
    const searches = call.mock.calls.filter(([command]) => command === 'search.query');
    expect(searches).toHaveLength(2);
    expect((searches[1] as unknown as [string, { root_id: string | null }])[1].root_id).toBe(scoped ? 'rt_a' : null);
  });

  it('does not refresh an all-roots search when the root validation read fails', async () => {
    const call = vi.fn(async (command: string) => {
      if (command === 'roots.list') throw new Error('Synthetic root list failure');
      return reply(command);
    });
    const { state, code } = explorer(call);
    state.query = 'report'; expect(code.commit()).toBe(true);
    await code.reload();
    expect(state.rows).toEqual([]);
    expect(state.rootProblem).toBe('Synthetic root list failure');
    expect(state.busy).toBe(false);
    expect(call.mock.calls.filter(([command]) => command === 'search.query')).toEqual([]);
  });

  it('retains selection for catalogue refreshes but clears sealed IDs on a connected restart', async () => {
    const { state, code } = explorer(vi.fn(async command => reply(command)));
    const selected = [{ node_id: 'nd_old_session', name: 'shared.pdf' }];
    state.selected = selected;
    code.effect();
    await vi.waitFor(() => expect(state.rootsLoading).toBe(false));
    state.session.epoch++; code.effect();
    await vi.waitFor(() => expect(state.rootsLoading).toBe(false));
    expect(state.selected).toEqual(selected);
    state.session.connectionEpoch++; state.session.epoch++; code.effect();
    expect(state.session.state).toBe('connected');
    expect(state.selected).toEqual([]);
    await vi.waitFor(() => expect(state.rootsLoading).toBe(false));
  });

  it('offers the complete active root list, including roots beyond the Atlas sample', async () => {
    const all = Array.from({ length: 64 }, (_, i) => ({ ...grantedA, root_id: `rt_${i}` }));
    all[0] = { ...all[0]!, grant_state: 'revoked' };
    const { state, code } = explorer(vi.fn(async command => reply(command, all)));
    await code.reload();
    expect(state.rootChoices).toEqual(all.slice(1));
    state.rootChoice = 'rt_63'; state.query = 'shared';
    expect(code.commit()).toBe(true);
    expect(state.submittedRoot?.root_id).toBe('rt_63');
  });

  it('drops a late A result after B is submitted, even when abort is ignored', async () => {
    const pending = deferred<{ result: typeof searchExample }>();
    const a = searchExample.hits[0]!;
    const b = { ...a, row: { ...a.row, node_id: 'nd_b', name: 'shared.pdf' } };
    const call = vi.fn().mockReturnValueOnce(pending.promise).mockResolvedValue({ result: { ...searchExample, hits: [b] } });
    const { state, code } = explorer(call);
    state.rootChoices = [grantedA, grantedB]; state.rootChoice = 'rt_a'; state.query = 'shared';
    code.commit(); const older = code.show();
    state.rootChoice = 'rt_b'; code.commit(); await code.show();
    pending.resolve({ result: { ...searchExample, hits: [a] } }); await older;
    expect(state.rows).toEqual([b.row]);
    expect(state.submittedRoot).toEqual(grantedB);
    expect(call.mock.calls[0]![2].signal.aborted).toBe(true);
  });

  it.each(['removed', 'revoked', 'unreachable'])('clears %s scoped results and never widens a pending page', async reason => {
    const pending = deferred<{ result: typeof searchExample }>();
    let changed = false;
    const call = vi.fn(async (command: string, request: { cursor?: string | null }) => {
      if (command === 'search.query' && request.cursor) return pending.promise;
      if (changed && reason === 'unreachable' && command === 'roots.list') throw new Error('Synthetic root list failure');
      return reply(command, changed ? reason === 'revoked' ? [{ ...grantedA, grant_state: 'revoked' }, grantedB] : [grantedB] : [grantedA, grantedB]);
    });
    const { state, code } = explorer(call);
    state.rootChoices = [grantedA, grantedB]; state.rootChoice = 'rt_a'; state.query = 'shared';
    code.commit(); await code.show();
    state.next = 'next';
    const older = code.show(true);
    changed = true; state.session.epoch++;
    const reload = code.reload();
    expect(state.rows).toEqual([]);
    await reload;
    expect(state.scopeBlocked).toBe(true);
    expect(state.submittedRoot).toBeNull();
    expect(state.rootChoice).toBeNull();
    pending.resolve({ result: searchExample }); await older;
    expect(state.rows).toEqual([]);
    expect(state.found.size).toBe(0);
    expect(code.commit()).toBe(false);
    await code.show();
    expect(call.mock.calls.filter(([c]) => c === 'search.query')).toHaveLength(2);
    state.rootChoice = ''; expect(code.commit()).toBe(true); await code.show();
    expect(state.rows).toEqual(searchExample.hits.map(h => h.row));
    expect(state.scopeBlocked).toBe(false);
  });

  it('ignores an obsolete root list and blocks sealed IDs after a reconnect', async () => {
    const pending = deferred<{ result: { roots: Root[] } }>();
    let delay = true;
    const call = vi.fn(async (command: string) => command === 'roots.list' && delay ? pending.promise : reply(command, [grantedB]));
    const { state, code } = explorer(call);
    state.rootChoices = [grantedA]; state.rootChoice = 'rt_a'; state.query = 'shared'; code.commit();
    const older = code.reload();
    delay = false; state.session.connectionEpoch++; state.session.epoch++;
    await code.reload();
    pending.resolve({ result: { roots: [grantedA] } }); await older;
    expect(state.rootChoices).toEqual([grantedB]);
    expect(state.scopeBlocked).toBe(true);
    expect(call.mock.calls.filter(([c]) => c === 'search.query')).toEqual([]);
  });

  it('requires a new choice after remount and does not replay an old global link', async () => {
    const call = vi.fn(async (command: string) => reply(command));
    const { state, code } = explorer(call);
    state.rootChoices = [grantedA]; state.rootChoice = 'rt_a'; state.query = 'shared'; code.commit();
    const cleanup = viewScript('explorer', state, 'rootsRequest++').effect() as unknown as () => void;
    state.session.state = 'unavailable'; state.session.connectionEpoch++; cleanup();
    expect(state.scopedSearchOpen).toBe(true);
    // A fresh instance keeps only the boolean intent, never the old root ID or results.
    const next = explorer(call);
    next.state.scopedSearchOpen = next.state.reconnectScopePending = next.state.scopeBlocked = next.state.searching = true;
    next.state.rootChoice = null; next.state.route.search = 'q=shared';
    next.code.effect();
    await vi.waitFor(() => expect(next.state.rootsLoading).toBe(false));
    expect(call.mock.calls.filter(([c]) => c === 'search.query')).toEqual([]);
    expect(next.state.rootChoice).toBeNull();
    next.state.route.search = 'q=another'; next.code.effect();
    await vi.waitFor(() => expect(call.mock.calls.filter(([c]) => c === 'search.query')).toHaveLength(1));
    expect(next.state.submittedRoot).toBeNull();
  });

  it('keeps partial empty coverage, chosen-scope presets and clear-search honest', async () => {
    const call = vi.fn(async (command: string) => command === 'search.query'
      ? { result: { ...searchExample, hits: [], partial: true } } : reply(command));
    const { state, code } = explorer(call);
    state.rootChoices = [grantedA]; state.rootChoice = 'rt_a';
    code.preset('ext:iso size:>1gb');
    await vi.waitFor(() => expect(state.busy).toBe(false));
    expect(call.mock.calls[0]![0]).toBe('search.query');
    expect(state.submittedRoot).toEqual(grantedA);
    expect(code.evaluate('scope')).toMatchObject({ partial: true });
    expect(state.rows).toEqual([]);
    code.clearSearch();
    expect(state.searching).toBe(false);
    expect(state.scopedSearchOpen).toBe(false);
    expect(state.rootChoice).toBe('rt_a');
    state.route.search = 'q=shared'; code.effect();
    await vi.waitFor(() => expect(state.searching).toBe(true));
    expect(state.submittedRoot).toBeNull();
    expect(state.rootChoice).toBe('');
  });
});

const rootA = { id: 'root_a', name: 'Synthetic A' }, rootB = { id: 'root_b', name: 'Synthetic B' };
const roots = (...items: typeof rootA[]) => ({ nodes: items.map(r => ({ node_id: r.id, name: r.name, parent: 0 })) });
const children = { generation: 'g1', items: [], next_cursor: null, total: 0, budget_hit: false };

describe('Explorer starting points after invalidation', () => {
  it.each([false, true])('removes a revoked sibling while preserving the current trail and searching=%s', async searching => {
    let revoked = false;
    const call = vi.fn(async (command: string) => ({ result: command === 'roots.list' ? rootList : command === 'tree.slice'
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
    const call = vi.fn(async (command: string, req: { node_id?: string }) => ({ result: command === 'roots.list' ? rootList : command === 'tree.slice' ? roots(...(restarted ? [other, afterRestart] : [rootA]))
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
    const call = vi.fn(async (command: string, req: { node_id?: string }) => ({ result: command === 'roots.list' ? rootList : command === 'tree.slice' ? roots(twin)
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
      return { result: command === 'roots.list' ? rootList : command === 'tree.slice' ? roots(rootA, rootB) : children };
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

  it('preserves a folder navigation error when the Atlas refresh fails', async () => {
    const call = vi.fn(async (command: string) => {
      if (command === 'tree.slice') throw new Error('Synthetic Atlas failure');
      return { result: command === 'roots.list' ? rootList : children };
    });
    const { state, code } = explorer(call);
    state.trail = [rootA];
    await code.reload();
    expect(state.error).toBe('Synthetic Atlas failure');
    expect(state.busy).toBe(false);
    expect(state.trail).toEqual([rootA]);
    expect(call.mock.calls.filter(([command]) => command === 'tree.children' || command === 'search.query')).toEqual([]);
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
      .mockImplementation(async (command: string) => ({ result: command === 'roots.list' ? rootList : command === 'tree.slice' ? roots(rootA) : children }));
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
    const call = vi.fn(async (command: string) => ({ result: command === 'roots.list' ? rootList : command === 'tree.slice' ? roots(rootA)
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

  it('inspects the linked node again when a reconnect aborts it before it settles', async () => {
    const pending = deferred<{ result: unknown }>();
    let first = true;
    const call = vi.fn(async (command: string, params: { node_id?: string }) => {
      if (command === 'node.inspect' && params.node_id?.startsWith('nd_') && first) { first = false; return pending.promise; }
      return { result: command === 'roots.list' ? rootList : command === 'tree.slice' ? roots(rootA)
        : command === 'node.inspect' ? { display_path: { text: 'synthetic/a', truncated: false } } : children };
    });
    const inspectedNodes = () => (call.mock.calls as unknown as Array<[string, { node_id: string }]>)
      .filter(args => args[0] === 'node.inspect' && args[1].node_id.startsWith('nd_')).map(args => args[1].node_id);
    const { state, code } = explorer(call);
    state.route.search = 'node=nd_a';
    const cleanup = code.effect() as unknown as () => void;
    await vi.waitFor(() => expect(inspectedNodes()).toEqual(['nd_a']));
    cleanup(); // reconnect while node.inspect is in flight
    state.session.epoch++;
    code.effect();
    await vi.waitFor(() => expect(inspectedNodes()).toEqual(['nd_a', 'nd_a']));
    pending.resolve({ result: { display_path: { text: 'stale', truncated: false } } });
  });

  it('does not re-apply a pending link the reader superseded, even when the aborted call never settles', async () => {
    const never = new Promise<never>(() => {}); // a transport that ignores AbortSignal (Tauri)
    let first = true;
    const call = vi.fn(async (command: string, params: { node_id?: string }) => {
      if (command === 'node.inspect' && params.node_id === 'nd_a' && first) { first = false; return never; }
      return { result: command === 'roots.list' ? rootList : command === 'tree.slice' ? roots(rootA)
        : command === 'node.inspect' ? { display_path: { text: 'synthetic/b', truncated: false } } : children };
    });
    const inspectedNodes = () => (call.mock.calls as unknown as Array<[string, { node_id: string }]>)
      .filter(args => args[0] === 'node.inspect' && args[1].node_id.startsWith('nd_')).map(args => args[1].node_id);
    const { state, code } = explorer(call);
    state.route.search = 'node=nd_a';
    const cleanup = code.effect() as unknown as () => void;
    await vi.waitFor(() => expect(inspectedNodes()).toEqual(['nd_a']));
    await code.evaluate("inspect('nd_b')"); // the reader inspects another row while the link is pending
    cleanup(); // then a reconnect
    state.session.epoch++;
    code.effect();
    await new Promise(resolve => setTimeout(resolve, 10));
    expect(inspectedNodes()).toEqual(['nd_a', 'nd_b']);
  });
});

type TodayCode = { load(): Promise<void>; loadReclaim(ids: string[]): Promise<void>; loadNewSince(ids: string[]): Promise<void>; effect(): void; onEvent(ev: { event: string }): void };

function today(call: ReturnType<typeof vi.fn>) {
  const state = {
    session: { client: { call }, epoch: 0, connectionEpoch: 0, handle: (e: Error) => e.message },
    scans: { settled: 0 }, untrack, alive: true, ticket: 0, reclaimTicket: 0, newSinceTicket: 0, started: false, timer: undefined,
    JOBS_LIMIT: 100, REFRESH_MS: 250,
    roots: null as unknown, alerts: null as unknown, jobs: null as unknown, reclaim: new Map<string, unknown>(), failedReclaim: 0,
    loadProblems: [] as string[], reclaimProblems: [] as string[], loaded: false, refreshed: 0,
    newSince: new Map<string, unknown>(), newSinceRead: false, failedNewSince: 0, newSinceProblems: [] as string[],
    askedKey: null as string | null, askedConnection: -1, newSinceKey: null as string | null,
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

  it('lets a stale new-since read never overwrite a newer one, and asks for the baseline default with a one-row limit', async () => {
    const old = deferred<unknown>();
    const call = vi.fn(async () => ({ result: 'new_answer' }));
    call.mockImplementationOnce(() => old.promise as never);
    const { state, code } = today(call);
    const older = code.loadNewSince(['rt_a']);
    await code.loadNewSince(['rt_a']);
    old.resolve({ result: 'old_answer' });
    await older;
    expect([...state.newSince]).toEqual([['rt_a', 'new_answer']]);
    expect(state.newSinceRead).toBe(true);
    expect(call).toHaveBeenLastCalledWith('changes.new_since', { root_id: 'rt_a', baseline_run: null, limit: 1 });
  });

  it('re-reads what is new only when a folder scan or the connection changes, never on a plain tree invalidation', () => {
    const call = vi.fn(async () => ({ result: 'answer' }));
    const { state } = today(call);
    const { effect } = viewScript('today', state, 'newSinceKey') as unknown as { effect(): void };
    effect(); // folders not read yet: nothing to ask
    expect(call).not.toHaveBeenCalled();
    state.newSinceKey = 'rt_a|complete|1|t1';
    effect();
    expect(call).toHaveBeenCalledTimes(1);
    for (let n = 0; n < 5; n++) { state.session.epoch++; effect(); } // tree.invalidated bumps the epoch about twice a second per root
    expect(call).toHaveBeenCalledTimes(1);
    state.newSinceKey = 'rt_a|scanning|1|t1'; // a scan starts
    effect();
    expect(call).toHaveBeenCalledTimes(2);
    state.session.epoch++; effect();
    expect(call).toHaveBeenCalledTimes(2);
    state.newSinceKey = 'rt_a|complete|2|t2'; // and ends
    effect();
    expect(call).toHaveBeenCalledTimes(3);
    state.session.connectionEpoch++; // a new connection or a restarted engine
    state.session.epoch++; effect();
    expect(call).toHaveBeenCalledTimes(4);
  });

  it('drops a new-since read that finishes after the connection changed, and stays unread', async () => {
    const reply = deferred<unknown>();
    const { state, code } = today(vi.fn(() => reply.promise as never));
    const loading = code.loadNewSince(['rt_a']);
    state.session.connectionEpoch++;
    reply.resolve({ result: 'answer_from_the_old_engine' });
    await loading;
    expect(state.newSince.size).toBe(0);
    expect(state.newSinceRead).toBe(false);
  });

  it('keeps a folder whose new-since read failed as unread, replaces the errors each round and reads nothing for no folders', async () => {
    const call = vi.fn(async (_c: string, p: { root_id: string }) => {
      if (p.root_id === 'rt_bad') throw new Error('new files unreadable');
      return { result: 'answer' };
    });
    const { state, code } = today(call as never);
    await code.loadNewSince(['rt_a', 'rt_bad']);
    await code.loadNewSince(['rt_a', 'rt_bad']);
    expect([...state.newSince]).toEqual([['rt_a', 'answer'], ['rt_bad', null]]);
    expect(state.failedNewSince).toBe(1);
    expect(state.newSinceProblems).toEqual(['new files unreadable']);
    call.mockClear();
    await code.loadNewSince([]);
    expect(call).not.toHaveBeenCalled();
    expect(state.newSince.size).toBe(0);
    expect(state.failedNewSince).toBe(0);
    expect(state.newSinceProblems).toEqual([]);
    expect(state.newSinceRead).toBe(true);
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
