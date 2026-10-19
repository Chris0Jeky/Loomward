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
import type { SessionInfo } from '../../src/lib/contracts.gen';

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
  const state = {
    session, scans, forget, picker, Say, LoomwardError, TransportError,
    $state: identity, $derived: Object.assign(identity, { by: (fn: () => unknown) => fn() }),
    $effect: () => undefined, onMount: () => undefined, untrack: (fn: () => unknown) => fn(), tick: async () => undefined,
    document: { activeElement: null }, HTMLElement: class {},
    formatTime: identity, escapedName: identity,
  };
  return new Function('state', `with (state) { return (() => { ${compiled} })(); }`)(state) as {
    ask(p: { kind: 'forget'; id: string; label: string }): Promise<void>;
    confirm(): Promise<void>;
    cancel(): Promise<void>;
    stopWaiting(): Promise<void>;
    evaluate(expression: string): unknown;
  };
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
beforeEach(() => { Object.assign(forget, { pending: null, working: false, unknown: false, done: '', problem: '' }); });
afterEach(() => { session.client = null; session.info = null; vi.restoreAllMocks(); });

it.each([unknown, deadline])('remount mid-pending keeps the forget id and the Check status prompt after $name (#739)', async fault => {
  const call = client();
  vi.spyOn(session, 'handle').mockReturnValue('connection lost');
  let reject!: (error: Error) => void;
  call.mockImplementationOnce(() => new Promise((_, no) => { reject = no; }));
  const first = mount();
  await first.ask(request);
  const waiting = first.confirm();
  const id = call.mock.calls[0]![0].request_id;
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
  expect(stopped).not.toContain('Check status');
  expect(forget.pending).toBeNull();
  forget.ask({ ...request, id: 'rt_other' });
  expect(forget.pending?.id).toBe('rt_other');
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
