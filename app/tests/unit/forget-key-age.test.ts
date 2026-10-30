import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Client, LoomwardError } from '../../src/lib/transport/client';
import type { Transport } from '../../src/lib/transport/transport';
import type { SessionInfo } from '../../src/lib/contracts.gen';
import { session } from '../../src/lib/stores/session.svelte';
import { forget } from '../../src/lib/stores/forget.svelte';

const request = { kind: 'forget' as const, id: 'rt_fixture', label: 'Synthetic' };
const completed = { root_id: request.id, forgotten_at: '2026-10-18T00:00:00Z' };
const revoked = { kind: 'metadata_root', root_id: request.id, revoked_at: '2026-10-17T00:00:00Z', forgotten_at: null };
const deadline = new LoomwardError({ code: 'deadline_exceeded', message: 'unknown outcome', retryable: false, detail: null });
const reply = (r: { request_id: string }, result: unknown, state_rev: string | null = '42') =>
  ({ protocol: 'loomward/3', request_id: r.request_id, ok: true, result, meta: { state_rev } });

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-10-27T00:00:00Z'));
  Object.assign(forget, { pending: null, working: false, unknown: false, done: '', problem: '', stopped: false });
  session.info = { session_started_at: '2026-10-27T00:00:00Z' } as SessionInfo;
  vi.spyOn(session, 'handle').mockImplementation(e => (e as Error).message);
});
afterEach(() => {
  session.client = null; session.info = null;
  vi.restoreAllMocks(); vi.useRealTimers();
});

async function uncertain() {
  const call = vi.fn().mockRejectedValueOnce(deadline);
  session.client = new Client({ call } as unknown as Transport);
  forget.ask(request);
  await forget.confirm();
  expect(forget.unknown).toBe(true);
  return call;
}

it.each(['expiry', 'session', 'epoch', 'client'])('reads before retrying after key %s and reports a forgotten record without another mutation', async change => {
  const call = await uncertain();
  if (change === 'expiry') vi.advanceTimersByTime(600_000);
  if (change === 'session') session.info = { session_started_at: '2026-10-27T00:01:00Z' } as SessionInfo;
  if (change === 'epoch') session.connectionEpoch++;
  if (change === 'client') session.client = new Client({ call } as unknown as Transport);
  call.mockImplementation(async r => reply(r, r.command === 'grants.list'
    ? { grants: [{ kind: 'disclosure', ...completed }, { ...revoked, ...completed }] } : completed));
  await forget.confirm();
  expect(call.mock.calls.map(([r]) => r.command)).toEqual(['roots.forget', 'grants.list']);
  expect(forget.pending).toBeNull();
  expect(forget.unknown).toBe(false);
  expect(forget.done).toContain('Forgotten at');
  expect(forget.done).toContain('Backup cleanup is unverified');
});

it('keeps a retained key byte-identical and measures expiry from minting, not the last retry', async () => {
  const call = await uncertain();
  vi.advanceTimersByTime(599_999);
  call.mockRejectedValueOnce(deadline);
  await forget.confirm();
  expect(call.mock.calls[1]).toEqual(call.mock.calls[0]);
  vi.advanceTimersByTime(1);
  call.mockImplementation(async r => reply(r, { grants: [{ ...revoked, ...completed }] }));
  await forget.confirm();
  expect(call.mock.calls.map(([r]) => r.command)).toEqual(['roots.forget', 'roots.forget', 'grants.list']);
  expect(forget.pending).toBeNull();
});

it.each(['expiry', 'restart'])('mints a new key only after the %s state read, binding retries to that revision', async change => {
  const call = await uncertain();
  if (change === 'expiry') vi.advanceTimersByTime(600_000);
  else session.connectionEpoch++;
  call.mockImplementation(async r => {
    if (r.command === 'grants.list') return reply(r, { grants: [revoked] });
    throw deadline;
  });
  await forget.confirm();
  expect(call.mock.calls.map(([r]) => r.command)).toEqual(['roots.forget', 'grants.list', 'roots.forget']);
  const oldRequest = call.mock.calls[0]![0], freshRequest = call.mock.calls[2]![0];
  expect(freshRequest.request_id).not.toBe(oldRequest.request_id);
  expect(freshRequest.expected_state_rev).toBe('42');
  expect(freshRequest.payload).toEqual(oldRequest.payload);
  expect(forget.unknown).toBe(true);
  call.mockImplementation(async r => reply(r, completed));
  await forget.confirm();
  expect(call.mock.calls[3]![0]).toEqual(freshRequest);
  expect(forget.pending).toBeNull();
});

it.each(['missing', 'different root', 'disclosure', 'active', 'no revision'])('does not replay after expiry when the state read shows %s', async state => {
  const call = await uncertain();
  vi.advanceTimersByTime(600_000);
  const grants = state === 'missing' ? [] : [{ ...revoked,
    ...(state === 'different root' ? { root_id: 'rt_other' } : {}),
    ...(state === 'disclosure' ? { kind: 'disclosure' } : {}),
    ...(state === 'active' ? { revoked_at: null } : {}),
  }];
  call.mockImplementation(async r => reply(r, { grants }, state === 'no revision' ? null : '42'));
  await forget.confirm();
  expect(call.mock.calls.map(([r]) => r.command)).toEqual(['roots.forget', 'grants.list']);
  expect(forget.pending?.requestId).toBe(call.mock.calls[0]![0].request_id);
  expect(forget.unknown).toBe(true);
  expect(forget.done).toBe('');
  // The owner is told why nothing was sent, never left with a silent no-op.
  expect(forget.problem).toMatch(['active', 'no revision'].includes(state) ? /not sent again/ : /cannot tell whether/);
});

it('keeps an expired outcome unknown when the prior state read fails, then reads again', async () => {
  const call = await uncertain();
  vi.advanceTimersByTime(600_000);
  call.mockRejectedValueOnce(new LoomwardError({ code: 'internal_error', message: 'read failed', retryable: true, detail: null }));
  await forget.confirm();
  expect(call.mock.calls[1]![0].command).toBe('grants.list');
  expect(forget.pending?.requestId).toBe(call.mock.calls[0]![0].request_id);
  expect(forget.unknown).toBe(true);
  call.mockImplementation(async r => reply(r, { grants: [{ ...revoked, ...completed }] }));
  await forget.confirm();
  expect(call.mock.calls[2]![0].command).toBe('grants.list');
  expect(forget.pending).toBeNull();
});

it('ignores a prior state read from an identity that changed while it was in flight', async () => {
  const call = await uncertain();
  vi.advanceTimersByTime(600_000);
  let release!: (value: unknown) => void;
  call.mockImplementationOnce(() => new Promise(resolve => { release = resolve; }));
  const waiting = forget.confirm();
  session.connectionEpoch++;
  release(reply(call.mock.calls[1]![0], { grants: [revoked] }));
  await waiting;
  expect(call.mock.calls.map(([r]) => r.command)).toEqual(['roots.forget', 'grants.list']);
  expect(forget.unknown).toBe(true);
  expect(forget.done).toBe('');
});

it('reads state again after a state-bound send fails definitively, never sending unbound', async () => {
  const call = await uncertain();
  vi.advanceTimersByTime(600_000);
  call.mockImplementation(async r => {
    if (r.command === 'grants.list') return reply(r, { grants: [revoked] });
    throw new LoomwardError({ code: 'state_rev_changed', message: 'state changed', retryable: false, detail: null } as never);
  });
  await forget.confirm();
  expect(forget.unknown).toBe(false);
  await forget.confirm();
  const commands = call.mock.calls.map(([r]) => r.command);
  expect(commands).toEqual(['roots.forget', 'grants.list', 'roots.forget', 'grants.list', 'roots.forget']);
  for (const [r] of call.mock.calls.filter(([r]) => r.command === 'roots.forget').slice(1))
    expect(r.expected_state_rev).toBe('42');
});
