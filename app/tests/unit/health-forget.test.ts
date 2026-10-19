import { readFileSync } from 'node:fs';
import ts from 'typescript';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'svelte/server';
import View from '../../src/views/health/View.svelte';
import { session } from '../../src/lib/stores/session.svelte';
import { grantPicker as picker } from '../../src/lib/stores/grant.svelte';
import { forget } from '../../src/lib/stores/forget.svelte';
import { scans } from '../../src/lib/stores/scan.svelte';
import { Say } from '../../src/lib/ui/say.svelte';
import { LoomwardError, TransportError, Client } from '../../src/lib/transport/client';
import type { Transport } from '../../src/lib/transport/transport';
import type { EventEnvelope, SessionInfo } from '../../src/lib/contracts.gen';

const identity = <T>(value: T) => value;

/** Like view-races: run the real instance script, including fresh initial state on every mount.
 * Rendering separately exercises the actual Svelte markup without a browser DOM. */
function mount() {
  const source = readFileSync(new URL('../../src/views/health/View.svelte', import.meta.url), 'utf8');
  const script = source.match(/<script lang="ts">([\s\S]*?)<\/script>/)![1]!;
  const file = ts.createSourceFile('view.ts', script, ts.ScriptTarget.Latest, true);
  const code = file.statements.filter(node => !ts.isImportDeclaration(node)).map(node => {
    if (ts.isVariableStatement(node)) {
      const d = node.declarationList.declarations[0]!;
      if (d.initializer && ts.isCallExpression(d.initializer) && d.initializer.expression.getText(file).startsWith('$derived')) {
        return `Object.defineProperty(state, '${d.name.getText(file)}', { get: () => ${d.initializer.getText(file)} });`;
      }
    }
    return node.getText(file);
  }).join('\n');
  const compiled = ts.transpileModule(`${code}\nreturn { ask, confirm, cancel, stopWaiting, evaluate: expression => eval(expression) };`, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None },
  }).outputText;
  let cleanup: (() => void) | undefined;
  const effects: (() => void)[] = [];
  const state = {
    session, scans, forget, picker, Say, LoomwardError, TransportError,
    $state: identity, $derived: Object.assign(identity, { by: (fn: () => unknown) => fn() }),
    $effect: (fn: () => void) => { effects.push(fn); }, onMount: (fn: () => () => void) => { cleanup = fn(); }, untrack: (fn: () => unknown) => fn(), tick: async () => undefined,
    document: { activeElement: null }, HTMLElement: class {},
    formatTime: identity, escapedName: identity,
  };
  const view = new Function('state', `with (state) { return (() => { ${compiled} })(); }`)(state) as {
    ask(p: { kind: 'forget'; id: string; label: string }): Promise<void>;
    confirm(): Promise<void>;
    cancel(): Promise<void>;
    stopWaiting(): Promise<void>;
    evaluate(expression: string): unknown;
  };
  return { ...view, flushEffects: () => { for (const effect of effects) effect(); }, unmount: () => cleanup?.() };
}

function client() {
  const call = vi.fn();
  session.client = new Client({ call } as unknown as Transport);
  return call;
}
const unknown = new TransportError('connection lost');
const deadline = new LoomwardError({ code: 'deadline_exceeded', message: 'outcome unknown', retryable: false, detail: null });
const notFound = new LoomwardError({ code: 'not_found', message: 'unknown root', retryable: false, detail: null });
const request = { kind: 'forget' as const, id: 'rt_fixture', label: 'Synthetic' };
const completed = { root_id: 'rt_fixture', forgotten_at: '2026-10-18T00:00:00Z' };
const reply = (r: { request_id: string }, result: unknown) => ({ protocol: 'loomward/3', request_id: r.request_id, ok: true, result, meta: {} });
beforeEach(() => { Object.assign(forget, { pending: null, working: false, unknown: false, done: '', problem: '', stopped: false }); });
afterEach(() => { session.client = null; session.info = null; vi.restoreAllMocks(); });

it.each(['navigation', 'reconnect'])('leaving Health discards an unsubmitted forget before %s', async reason => {
  const call = client().mockImplementation(async r => reply(r, completed));
  const first = mount();
  await first.ask(request);
  expect(forget.pending?.id).toBe(request.id);
  first.unmount();
  if (reason === 'reconnect') client().mockImplementation(async r => reply(r, completed));
  const next = mount();
  expect(next.evaluate('pending')).toBeNull();
  expect(forget.unknown).toBe(false);
  expect(call).not.toHaveBeenCalled();
  next.unmount();
});

it.each(['connection epoch', 'session info', 'client'])('a mounted unsubmitted forget is discarded after its %s changes', async change => {
  const call = client().mockImplementation(async r => reply(r, { roots: [], grants: [], jobs: [] }));
  const view = mount();
  await view.ask(request);
  if (change === 'connection epoch') session.connectionEpoch++;
  if (change === 'session info') session.info = { session_started_at: '2026-10-19T01:00:00Z' } as SessionInfo;
  if (change === 'client') session.client = new Client({ call } as unknown as Transport);
  session.epoch++;
  view.flushEffects();
  expect(forget.pending).toBeNull();
  expect(forget.unknown).toBe(false);
  expect(call.mock.calls.filter(([r]) => r.command === 'roots.forget')).toHaveLength(0);
  view.unmount();
});

it('confirmation itself refuses an unsubmitted prompt from an older identity', async () => {
  const call = client().mockImplementation(async r => reply(r, completed));
  forget.ask(request);
  session.connectionEpoch++;
  await forget.confirm();
  expect(call).not.toHaveBeenCalled();
  expect(forget.pending).toBeNull();
  expect(forget.done).toBe('');
});

it('an ordinary same-engine roots refresh retains an unsubmitted prompt', async () => {
  const call = client().mockImplementation(async r => reply(r, { roots: [], grants: [], jobs: [] }));
  const view = mount();
  await view.ask(request);
  session.epoch++;
  view.flushEffects();
  expect(forget.pending?.id).toBe(request.id);
  expect(forget.pending?.requestId).toBeUndefined();
  expect(call.mock.calls.filter(([r]) => r.command === 'roots.forget')).toHaveLength(0);
  view.unmount();
});

it('a definite retry prompt is discarded when its service identity is replaced', async () => {
  const call = client().mockRejectedValueOnce(new LoomwardError({ code: 'internal_error', message: 'store error', retryable: true, detail: null }));
  forget.ask(request);
  await forget.confirm();
  expect(forget.pending?.id).toBe(request.id);
  session.info = { session_started_at: '2026-10-19T02:00:00Z' } as SessionInfo;
  forget.resume();
  expect(forget.pending).toBeNull();
  expect(forget.unknown).toBe(false);
  expect(call).toHaveBeenCalledTimes(1);
});

it.each([unknown, deadline])('remount mid-pending keeps the forget id and the Check status prompt after $name (#739)', async fault => {
  const call = client();
  vi.spyOn(session, 'handle').mockReturnValue('connection lost');
  let reject!: (error: Error) => void;
  call.mockImplementationOnce(() => new Promise((_, no) => { reject = no; }));
  const first = mount();
  await first.ask(request);
  const waiting = first.confirm();
  const id = call.mock.calls[0]![0].request_id;
  first.unmount();
  const remounted = mount();
  expect(render(View).body).toContain('Forgetting. Large roots can take a while');
  await remounted.confirm();
  expect(call).toHaveBeenCalledTimes(1);
  reject(fault);
  await waiting;
  expect(remounted.evaluate('pending?.id')).toBe('rt_fixture');
  const body = render(View).body;
  expect(body).toContain('may already have completed');
  expect(body).toContain('Check status');
  call.mockRejectedValueOnce(unknown);
  await remounted.confirm();
  expect(call.mock.calls[1]![0].request_id).toBe(id);
});

it('remount after an unknown outcome keeps the prompt and recovers with the original id', async () => {
  const call = client().mockRejectedValueOnce(deadline);
  const first = mount();
  await first.ask(request);
  await first.confirm();
  first.unmount();
  const second = mount();
  expect(second.evaluate('pending?.id')).toBe('rt_fixture');
  expect(render(View).body).toContain('Check status');
  call.mockImplementationOnce(async r => reply(r, completed));
  await forget.confirm();
  expect(call.mock.calls[1]![0].request_id).toBe(call.mock.calls[0]![0].request_id);
  expect(forget.pending).toBeNull();
  expect(render(View).body).toContain('Forgotten at');
  expect(forget.done).toContain('its own backup copies');
});

it('a definite error drops the cached id so a retry starts a new attempt (#736)', async () => {
  const call = client().mockRejectedValueOnce(new LoomwardError({ code: 'internal_error', message: 'store error', retryable: true, detail: null }));
  forget.ask(request);
  await forget.confirm();
  expect(forget.problem).toBe('store error');
  expect(forget.unknown).toBe(false);
  expect(forget.pending?.requestId).toBeUndefined();
  call.mockImplementationOnce(async r => reply(r, completed));
  await forget.confirm();
  expect(call.mock.calls[1]![0].request_id).not.toBe(call.mock.calls[0]![0].request_id);
  expect(forget.pending).toBeNull();
});

it('Keep it clears a definite forget error when the confirmation closes (#753)', async () => {
  client().mockRejectedValueOnce(new LoomwardError({ code: 'internal_error', message: 'store error', retryable: true, detail: null }));
  const view = mount();
  await view.ask(request);
  await view.confirm();
  expect(render(View).body).toContain('store error');
  await view.cancel();
  expect(forget.pending).toBeNull();
  expect(render(View).body).not.toContain('store error');
});

it('does not repeat a displayed forget receipt on a later Health visit (#753)', async () => {
  client().mockImplementationOnce(async r => reply(r, completed));
  const view = mount();
  forget.ask(request);
  await forget.confirm();
  expect(render(View).body).toContain('Forgotten at');
  view.unmount();
  mount();
  expect(render(View).body).not.toContain('Forgotten at');
});

it('clears a displayed definite error on leaving Health while retaining the retry prompt (#753)', async () => {
  client().mockRejectedValueOnce(new LoomwardError({ code: 'internal_error', message: 'store error', retryable: true, detail: null }));
  const view = mount();
  forget.ask(request);
  await forget.confirm();
  expect(render(View).body).toContain('store error');
  view.unmount();
  const next = mount();
  expect(render(View).body).not.toContain('store error');
  expect(next.evaluate('pending?.id')).toBe(request.id);
  expect(forget.pending?.requestId).toBeUndefined();
});

it('does not repeat the stopped-waiting notice on a later Health visit (#753)', async () => {
  client().mockRejectedValueOnce(deadline);
  const view = mount();
  forget.ask(request);
  await forget.confirm();
  await view.stopWaiting();
  expect(render(View).body).toContain('Stopped waiting.');
  view.unmount();
  mount();
  expect(render(View).body).not.toContain('Stopped waiting.');
  expect(forget.stopped).toBe(false);
});

it('keeps a receipt completed offscreen for the next Health visit (#753)', async () => {
  const call = client();
  let finish!: (value: unknown) => void;
  call.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  const first = mount();
  forget.ask(request);
  const waiting = forget.confirm();
  first.unmount();
  finish(reply(call.mock.calls[0]![0], completed));
  await waiting;
  const next = mount();
  expect(render(View).body).toContain('Forgotten at');
  next.unmount();
  mount();
  expect(render(View).body).not.toContain('Forgotten at');
});

it('recovers only the matching forgotten grant after a lost receipt, without claiming backup cleanup', async () => {
  const call = client().mockRejectedValueOnce(deadline).mockRejectedValueOnce(notFound)
    .mockImplementationOnce(async r => reply(r, { grants: [
      { kind: 'disclosure', grant_id: 'gr_fixture' },
      { kind: 'metadata_root', root_id: 'rt_other', forgotten_at: 'wrong' },
      { kind: 'metadata_root', ...completed },
    ] }));
  forget.ask(request);
  await forget.confirm();
  await forget.confirm();
  expect(call.mock.calls[2]![0].command).toBe('grants.list');
  expect(forget.pending).toBeNull();
  expect(forget.done).toContain('Forgotten at');
  expect(forget.done).toContain('Backup cleanup is unverified');
  expect(forget.unknown).toBe(false);
});

it('offers Stop waiting when the forgotten record is outside the bounded grants window', async () => {
  client().mockRejectedValueOnce(deadline).mockRejectedValueOnce(notFound)
    .mockImplementationOnce(async r => reply(r, { grants: [{ kind: 'metadata_root', root_id: 'rt_other', forgotten_at: completed.forgotten_at }] }));
  forget.ask(request);
  await forget.confirm();
  const id = forget.pending?.requestId;
  await forget.confirm();
  const waiting = render(View).body;
  expect(waiting).toContain('Check status');
  expect(waiting).toContain('Stop waiting');
  const view = mount();
  await view.cancel();
  await view.ask({ ...request, id: 'rt_other' });
  expect(forget.pending?.requestId).toBe(id);
  await view.stopWaiting();
  const stopped = render(View).body;
  expect(stopped).toContain('The forget outcome is unknown');
  expect(stopped).toContain('This does not cancel the request');
  // an unknown outcome is not styled as a success (invariant 4)
  expect(stopped).toMatch(/class="(?![^"]*\bok\b)[^"]*"[^>]*>Stopped waiting/);
  expect(stopped).not.toContain('Check status');
  expect(forget.pending).toBeNull();
  forget.ask({ ...request, id: 'rt_other' });
  expect(forget.pending?.id).toBe('rt_other');
  expect(forget.stopped).toBe(false);
});

it('keeps the outcome unknown if the fallback grants read fails', async () => {
  client().mockRejectedValueOnce(deadline).mockRejectedValueOnce(notFound).mockRejectedValueOnce(unknown);
  forget.ask(request);
  await forget.confirm();
  const id = forget.pending?.requestId;
  await forget.confirm();
  expect(forget.unknown).toBe(true);
  expect(forget.pending?.requestId).toBe(id);
  expect(forget.problem).toBe('');
  expect(render(View).body).toContain('Stop waiting');
});

it.each(['client', 'connection epoch', 'engine hello'] as const)('ignores a late reply after a changed %s without losing the key or clearing a newer flight', async change => {
  const a = client();
  let releaseOld!: (result: unknown) => void;
  a.mockImplementationOnce(() => new Promise(resolve => { releaseOld = resolve; }));
  forget.ask(request);
  const oldFlight = forget.confirm();
  const oldRequest = a.mock.calls[0]![0];
  const b = change === 'client' ? client() : a;
  if (change === 'connection epoch') session.connectionEpoch++;
  if (change === 'engine hello') session.info = { session_started_at: '2026-10-19T00:00:00Z' } as SessionInfo;
  mount();
  expect(forget.unknown).toBe(true);
  expect(forget.working).toBe(false);
  let rejectNew!: (error: Error) => void;
  b.mockImplementationOnce(() => new Promise((_, reject) => { rejectNew = reject; }));
  const newFlight = forget.confirm();
  const newRequest = b.mock.calls.at(-1)![0];
  expect(newRequest.request_id).toBe(oldRequest.request_id);
  releaseOld(reply(oldRequest, completed));
  await oldFlight;
  expect(forget.working).toBe(true);
  expect(forget.pending?.requestId).toBe(oldRequest.request_id);
  expect(forget.done).toBe('');
  rejectNew(unknown);
  await newFlight;
});

it.each(['stream hello', 'epoch changed', 'session info', 'client'] as const)('a mounted Health view exposes an unknown forget immediately after %s', async change => {
  const call = client();
  let release!: (value: unknown) => void;
  call.mockImplementation(r => r.command === 'roots.forget'
    ? new Promise(resolve => { release = resolve; })
    : Promise.resolve(reply(r, { roots: [], grants: [], jobs: [] })));
  const feed = (event: EventEnvelope['event'], data: Record<string, unknown>) =>
    (session as unknown as { onEvent(e: EventEnvelope): void }).onEvent({
      protocol: 'loomward/3', epoch: 'e_before', seq: 0, event, at: '2026-10-19T00:00:00Z',
      catalog_rev: null, state_rev: null, data,
    });
  feed('stream.hello', { epoch: 'e_before' });
  const view = mount();
  await view.ask(request);
  const waiting = forget.confirm();
  const sent = call.mock.calls.find(([r]) => r.command === 'roots.forget')![0];
  expect(forget.working).toBe(true);

  if (change === 'stream hello') feed('stream.hello', { epoch: 'e_after' });
  if (change === 'epoch changed') feed('stream.lagged', { reason: 'epoch_changed' });
  if (change === 'session info') session.info = { session_started_at: '2026-10-19T00:00:00Z' } as SessionInfo;
  if (change === 'client') {
    session.client = new Client({ call } as unknown as Transport);
    session.epoch++; // reconnect publishes the new client through the existing resync signal
  }
  view.flushEffects(); // the mounted instance's effects, without remounting or calling a forget action
  try {
    expect(forget.working).toBe(false);
    expect(forget.unknown).toBe(true);
    expect(forget.pending?.requestId).toBe(sent.request_id);
  } finally {
    release(reply(sent, completed));
    await waiting;
    view.unmount();
  }
  expect(forget.pending?.requestId).toBe(sent.request_id);
  expect(forget.done).toBe(''); // an old success cannot replace the uncertain outcome
});

it('a mounted Health roots refresh keeps the same-engine forget in flight', async () => {
  const call = client();
  let release!: (value: unknown) => void;
  call.mockImplementation(r => r.command === 'roots.forget'
    ? new Promise(resolve => { release = resolve; })
    : Promise.resolve(reply(r, { roots: [], grants: [], jobs: [] })));
  const view = mount();
  await view.ask(request);
  const waiting = forget.confirm();
  const sent = call.mock.calls.find(([r]) => r.command === 'roots.forget')![0];
  session.epoch++;
  view.flushEffects();
  expect(forget.working).toBe(true);
  expect(forget.unknown).toBe(false);
  expect(forget.pending?.requestId).toBe(sent.request_id);
  release(reply(sent, completed));
  await waiting;
  expect(forget.pending).toBeNull();
  expect(forget.done).toContain('Forgotten at');
  view.unmount();
});

it('accepts a same-engine reply after an ordinary roots refresh and while the view is away', async () => {
  const call = client();
  let finish!: (reply: unknown) => void;
  call.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  forget.ask(request);
  const waiting = forget.confirm();
  session.epoch++;
  finish(reply(call.mock.calls[0]![0], completed));
  await waiting;
  expect(forget.pending).toBeNull();
  expect(render(View).body).toContain('Forgotten at');
});

/** Ported from view-races (#716): the same behaviours, now against the store with a fake client. */
class FakeClient {
  nextRequestId = vi.fn(() => 'r_forget');
  constructor(readonly call: ReturnType<typeof vi.fn>) {}
}
function fake(call: ReturnType<typeof vi.fn>) {
  const c = new FakeClient(call);
  session.client = c as unknown as typeof session.client;
  const handle = vi.spyOn(session, 'handle').mockImplementation((e: unknown) => (e as Error).message);
  return { c, handle };
}
const result = { result: completed };
const forgotten = (id: string, at: string) => ({ kind: 'metadata_root', root_id: id, forgotten_at: at });

it.each([deadline, unknown])('keeps an unknown outcome pending and checks the same request id (#716)', async fault => {
  const call = vi.fn().mockRejectedValueOnce(fault).mockResolvedValue(result);
  const { c, handle } = fake(call);
  forget.ask(request);
  await forget.confirm();
  expect(call.mock.calls[0]).toEqual(['roots.forget', { root_id: 'rt_fixture' }, { deadlineMs: 60000, requestId: 'r_forget' }]);
  expect(forget.pending).not.toBeNull();
  expect(forget.unknown).toBe(true);
  expect(forget.problem).toBe('');
  expect(handle).not.toHaveBeenCalled();
  await forget.confirm();
  expect(call.mock.calls[1]).toEqual(call.mock.calls[0]);
  expect(c.nextRequestId).toHaveBeenCalledTimes(1);
  expect(forget.pending).toBeNull();
  expect(forget.done).toContain('Forgotten at');
});

it('shows a definite refusal as an error (#716)', async () => {
  fake(vi.fn().mockRejectedValue(new LoomwardError({ code: 'invalid_request', message: 'revoke it first', retryable: false, detail: null })));
  forget.ask(request);
  await forget.confirm();
  expect(forget.problem).toBe('revoke it first');
  expect(forget.unknown).toBe(false);
});

it('starts a new attempt after a definite error instead of replaying its cached answer (#716)', async () => {
  const call = vi.fn().mockRejectedValueOnce(new LoomwardError({ code: 'internal_error', message: 'store error', retryable: true, detail: null })).mockResolvedValue(result);
  const { c } = fake(call);
  forget.ask(request);
  await forget.confirm();
  expect(forget.problem).toBe('store error');
  await forget.confirm();
  expect(c.nextRequestId).toHaveBeenCalledTimes(2);
  expect(forget.done).toContain('Forgotten at');
});

it('recovers a completed forget from its anonymised record after receipt expiry or restart (#716)', async () => {
  const call = vi.fn().mockRejectedValueOnce(deadline).mockRejectedValueOnce(notFound)
    .mockResolvedValueOnce({ result: { grants: [{ kind: 'disclosure', grant_id: 'gr_disclosure' }, forgotten('rt_other', 'wrong'), forgotten('rt_fixture', completed.forgotten_at)] } })
    .mockResolvedValue(result);
  fake(call);
  forget.ask(request);
  await forget.confirm();
  await forget.confirm();
  expect(call.mock.calls[2]).toEqual(['grants.list', {}]);
  expect(forget.pending).toBeNull();
  expect(forget.done).toContain('Forgotten at 2026-10-18 00:00 UTC');
  expect(forget.done).toContain('Backup cleanup is unverified');
  expect(forget.problem).toBe('');
});

it('keeps the outcome unknown when a missing receipt has no matching forgotten record (#716)', async () => {
  fake(vi.fn().mockRejectedValueOnce(deadline).mockRejectedValueOnce(notFound)
    .mockResolvedValueOnce({ result: { grants: [forgotten('rt_other', completed.forgotten_at)] } }));
  forget.ask(request);
  await forget.confirm();
  await forget.confirm();
  expect(forget.unknown).toBe(true);
  expect(forget.pending?.id).toBe('rt_fixture');
  expect(forget.done).toBe('');
  expect(forget.problem).toBe('');
});

it('cannot replace or cancel an unknown forget and lose its action key (#716)', async () => {
  fake(vi.fn().mockRejectedValue(deadline));
  forget.ask(request);
  await forget.confirm();
  const original = forget.pending;
  forget.ask({ kind: 'forget', id: 'rt_other', label: 'Other' });
  forget.cancel();
  expect(forget.pending).toBe(original);
});

it('coalesces repeated confirmation while the same call is waiting (#716)', async () => {
  let release!: (value: typeof result) => void;
  const call = vi.fn().mockReturnValueOnce(new Promise(r => { release = r; })).mockResolvedValue(result);
  fake(call);
  forget.ask(request);
  const first = forget.confirm();
  await forget.confirm();
  expect(call).toHaveBeenCalledTimes(1);
  release(result);
  await first;
});
