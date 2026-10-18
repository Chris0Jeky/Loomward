import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ErrorCode, EventEnvelope, Root, SessionInfo } from '../../src/lib/contracts.gen';
import { grantPicker, PICKER_DEADLINE_MS } from '../../src/lib/stores/grant.svelte';
import { session } from '../../src/lib/stores/session.svelte';
import { Client } from '../../src/lib/transport/client';
import { TransportError, type Transport } from '../../src/lib/transport/transport';

const root = (id: string, path = 'D:\\Photos'): Root => ({ root_id: id, display_path: { text: path, truncated: false }, grant_state: 'active' } as Root);

type Reply = unknown | { error: ErrorCode; message?: string } | Error;
/** A client whose commands answer from `replies` (a function runs per call), recording each request. */
function fake(replies: Record<string, Reply | (() => Promise<Reply>)>) {
  const seen: { command: string; deadline_ms?: number; request_id: string }[] = [];
  const transport = {
    call: async (r: { command: string; request_id: string; deadline_ms?: number }) => {
      seen.push({ command: r.command, deadline_ms: r.deadline_ms, request_id: r.request_id });
      const raw = replies[r.command];
      const reply = typeof raw === 'function' ? await (raw as () => Promise<Reply>)() : raw;
      if (reply instanceof Error) throw reply;
      if (reply && typeof reply === 'object' && 'error' in reply) {
        const e = reply as { error: ErrorCode; message?: string };
        return { protocol: 'loomward/3', request_id: r.request_id, ok: false, error: { code: e.error, message: e.message ?? e.error, retryable: false, detail: null } };
      }
      return { protocol: 'loomward/3', request_id: r.request_id, ok: true, result: reply, meta: {} };
    },
  } as unknown as Transport;
  session.client = new Client(transport);
  return seen;
}
const granted = (r: Root) => ({ outcome: 'granted', root: r, refusal: null });
const feed = (event: EventEnvelope['event'], data: Record<string, unknown> = {}) =>
  (session as unknown as { onEvent(e: EventEnvelope): void }).onEvent({
    protocol: 'loomward/3', epoch: 'picker_stream', seq: 1, event,
    at: '2026-10-17T00:00:00Z', catalog_rev: null, state_rev: null, data,
  });

describe('"Add a folder…" state machine', () => {
  beforeEach(() => { grantPicker.phase = 'idle'; grantPicker.text = ''; grantPicker.rootId = null; });
  afterEach(() => { session.client = null; session.info = null; vi.useRealTimers(); });

  it.each(['granted', 'deadline'] as const)('ignores an old client’s late %s after a session change', async (outcome) => {
    let release!: (r: Reply) => void;
    const a = fake({ 'roots.request_grant': () => new Promise((r) => { release = r; }) });
    const pending = grantPicker.ask(['a_old']);
    const b = fake({ 'roots.list': { roots: [root('b_old')] } });
    session.epoch++;
    release(outcome === 'granted' ? granted(root('a_new')) : { error: 'deadline_exceeded' });
    await pending;
    expect(a.map((r) => r.command)).toEqual(['roots.request_grant']);
    expect(b).toHaveLength(0);
    expect(grantPicker).toMatchObject({ phase: 'idle', text: '', rootId: null });
  });

  it('ignores a late grant after a reconnect or engine epoch change on the same client', async () => {
    let release!: (r: Reply) => void;
    fake({ 'roots.request_grant': () => new Promise((r) => { release = r; }) });
    const pending = grantPicker.ask(['a_old']);
    feed('stream.hello', { epoch: 'picker_a' });
    feed('stream.hello', { epoch: 'picker_b' });
    release(granted(root('a_new')));
    await pending;
    expect(grantPicker).toMatchObject({ phase: 'idle', text: '', rootId: null });
  });

  it('keeps a valid grant receipt when its roots.changed event arrives before the reply', async () => {
    let release!: (r: Reply) => void;
    fake({ 'roots.request_grant': () => new Promise((r) => { release = r; }) });
    const pending = grantPicker.ask([]);
    feed('roots.changed');
    release(granted(root('new')));
    await pending;
    expect(grantPicker).toMatchObject({ phase: 'granted', rootId: 'new' });
  });

  it('rejects an old receipt as soon as stream.lagged announces an engine epoch change', async () => {
    let release!: (r: Reply) => void;
    fake({ 'roots.request_grant': () => new Promise((r) => { release = r; }) });
    const pending = grantPicker.ask([]);
    feed('stream.lagged', { reason: 'epoch_changed' });
    release(granted(root('old_engine')));
    await pending;
    expect(grantPicker).toMatchObject({ phase: 'idle', text: '', rootId: null });
  });

  it('accepts a same-session late reply after leaving the page while it was waiting', async () => {
    vi.useFakeTimers();
    let release!: (r: Reply) => void;
    fake({ 'roots.request_grant': () => new Promise((r) => { release = r; }) });
    const pending = grantPicker.ask([]);
    grantPicker.dismiss();
    vi.advanceTimersByTime(PICKER_DEADLINE_MS + 1);
    release(granted(root('late')));
    await pending;
    grantPicker.resume();
    expect(grantPicker).toMatchObject({ phase: 'granted', rootId: 'late' });
  });

  it('retires a completed offscreen receipt before showing Health in another session', async () => {
    let release!: (r: Reply) => void;
    fake({ 'roots.request_grant': () => new Promise((r) => { release = r; }) });
    const pending = grantPicker.ask([]);
    grantPicker.dismiss();
    release(granted(root('a_new')));
    await pending;
    expect(grantPicker.phase).toBe('granted');
    fake({});
    session.epoch++;
    grantPicker.resume();
    expect(grantPicker).toMatchObject({ phase: 'idle', text: '', rootId: null });
  });

  it('drops a personal-session receipt when the desktop adopts a synthetic dataset', async () => {
    let release!: (r: Reply) => void;
    fake({ 'roots.request_grant': () => new Promise((r) => { release = r; }) });
    session.info = { dataset_class: 'personal' } as SessionInfo;
    const pending = grantPicker.ask([]);
    session.info = { dataset_class: 'synthetic' } as SessionInfo;
    release(granted(root('personal')));
    await pending;
    expect(grantPicker).toMatchObject({ phase: 'idle', text: '', rootId: null });
  });

  it('ignores a roots reread that settles after the session changes', async () => {
    let release!: (r: Reply) => void;
    let reread!: () => void;
    const started = new Promise<void>((r) => { reread = r; });
    fake({ 'roots.request_grant': { error: 'deadline_exceeded' }, 'roots.list': () => {
      reread();
      return new Promise((r) => { release = r; });
    } });
    const pending = grantPicker.ask(['a_old']);
    await started;
    fake({});
    session.epoch++;
    release({ roots: [root('a_new')] });
    await pending;
    expect(grantPicker).toMatchObject({ phase: 'idle', text: '', rootId: null });
  });

  it('expires an unresolved baseline before an unrelated root arrives', async () => {
    vi.useFakeTimers();
    fake({ 'roots.request_grant': { error: 'deadline_exceeded' }, 'roots.list': { roots: [] } });
    await grantPicker.ask([]);
    vi.advanceTimersByTime(PICKER_DEADLINE_MS);
    grantPicker.sync([root('unrelated')]);
    expect(grantPicker).toMatchObject({ phase: 'idle', text: '', rootId: null });
  });

  it('retires an expired fallback read instead of leaving the button waiting', async () => {
    vi.useFakeTimers();
    let release!: (r: Reply) => void;
    let reread!: () => void;
    const started = new Promise<void>((r) => { reread = r; });
    fake({ 'roots.request_grant': { error: 'deadline_exceeded' }, 'roots.list': () => {
      reread();
      return new Promise((r) => { release = r; });
    } });
    const pending = grantPicker.ask([]);
    await started;
    vi.advanceTimersByTime(PICKER_DEADLINE_MS);
    release({ roots: [root('unrelated')] });
    await pending;
    expect(grantPicker).toMatchObject({ phase: 'idle', text: '', rootId: null });
  });

  it('retires an unresolved baseline when leaving the page', async () => {
    fake({ 'roots.request_grant': { error: 'deadline_exceeded' }, 'roots.list': { roots: [] } });
    await grantPicker.ask([]);
    grantPicker.dismiss();
    grantPicker.sync([root('unrelated')]);
    expect(grantPicker).toMatchObject({ phase: 'idle', text: '', rootId: null });
  });

  it('does not select an arbitrary receipt when multiple roots appeared after a timeout', async () => {
    fake({ 'roots.request_grant': { error: 'deadline_exceeded' }, 'roots.list': { roots: [root('one'), root('two')] } });
    await grantPicker.ask([]);
    expect(grantPicker.phase).not.toBe('granted');
    expect(grantPicker.rootId).toBeNull();
  });

  it('waits with the longest deadline, then grants and points at the new root', async () => {
    let release!: (r: unknown) => void;
    const seen = fake({ 'roots.request_grant': () => new Promise((r) => { release = r; }) });
    const p = grantPicker.ask([]);
    await Promise.resolve();
    expect(grantPicker.phase).toBe('waiting');
    expect(grantPicker.text).toBe('Waiting for you to choose a folder in the Windows dialog.');
    expect(seen).toMatchObject([{ command: 'roots.request_grant', deadline_ms: PICKER_DEADLINE_MS }]);
    expect(PICKER_DEADLINE_MS).toBe(60_000);
    release(granted(root('rt_new')));
    await p;
    expect(grantPicker).toMatchObject({ phase: 'granted', rootId: 'rt_new' });
    expect(grantPicker.text).toContain('Added D:\\Photos');
  });

  it('opens one dialog at a time and uses a fresh request id per click', async () => {
    let release!: (r: unknown) => void;
    const seen = fake({ 'roots.request_grant': () => new Promise((r) => { release = r; }) });
    const first = grantPicker.ask([]);
    await Promise.resolve();
    await grantPicker.ask([]); // a second click while waiting sends nothing
    expect(seen).toHaveLength(1);
    release({ outcome: 'cancelled_by_user', root: null, refusal: null });
    await first;
    const again = grantPicker.ask([]);
    await Promise.resolve();
    expect(seen).toHaveLength(2);
    expect(seen[1]!.request_id).not.toBe(seen[0]!.request_id);
    release({ outcome: 'cancelled_by_user', root: null, refusal: null });
    await again;
  });

  it('a reloaded page never reuses a request id, so the service cannot replay an old grant outcome', async () => {
    // The service's idempotency cache outlives a webview reload; a reload builds a new Client.
    const ids: string[] = [];
    const transport = { call: async (r: { request_id: string }) => { ids.push(r.request_id); return { protocol: 'loomward/3', request_id: r.request_id, ok: true, result: {}, meta: {} }; } } as unknown as Transport;
    await new Client(transport).call('roots.request_grant', { purpose: 'metadata_scan' });
    await new Client(transport).call('roots.request_grant', { purpose: 'metadata_scan' });
    expect(ids[1]).not.toBe(ids[0]);
    expect(ids.every((id) => /^[A-Za-z0-9_-]{1,64}$/.test(id))).toBe(true);
  });

  it('cancelled by the owner is neutral: no root, no failure', async () => {
    fake({ 'roots.request_grant': { outcome: 'cancelled_by_user', root: null, refusal: null } });
    await grantPicker.ask([]);
    expect(grantPicker).toMatchObject({ phase: 'cancelled', text: 'No folder was added.', rootId: null });
  });

  it('shows a refusal as plain text, including the unverified-identity one', async () => {
    fake({ 'roots.request_grant': { outcome: 'refused', root: null, refusal: 'identity_unavailable' } });
    await grantPicker.ask([]);
    expect(grantPicker.phase).toBe('refused');
    expect(grantPicker.text).toMatch(/could not verify that folder/);
    fake({ 'roots.request_grant': { outcome: 'refused', root: null, refusal: 'already_granted' } });
    await grantPicker.ask([]);
    expect(grantPicker.text).toMatch(/already granted/);
    fake({ 'roots.request_grant': { outcome: 'refused', root: null, refusal: 'some_future_reason' } });
    await grantPicker.ask([]);
    expect(grantPicker.text).toBe('The folder was refused: some future reason.');
  });

  it('on deadline_exceeded it reads the roots instead of reopening the picker: a root that landed counts as granted', async () => {
    const seen = fake({ 'roots.request_grant': { error: 'deadline_exceeded' }, 'roots.list': { roots: [root('rt_old', 'C:\\Old'), root('rt_new', 'E:\\New')] } });
    await grantPicker.ask(['rt_old']);
    expect(seen.map((s) => s.command)).toEqual(['roots.request_grant', 'roots.list']);
    expect(grantPicker).toMatchObject({ phase: 'granted', rootId: 'rt_new' });
  });

  it('on deadline_exceeded with nothing new it says so, still without a second picker', async () => {
    const seen = fake({ 'roots.request_grant': { error: 'deadline_exceeded' }, 'roots.list': { roots: [root('rt_old')] } });
    await grantPicker.ask(['rt_old']);
    expect(seen.filter((s) => s.command === 'roots.request_grant')).toHaveLength(1);
    expect(grantPicker).toMatchObject({ phase: 'deadline', rootId: null });
    expect(grantPicker.text).toMatch(/No folder was added yet/);
  });

  it('shows the service message for other engine errors', async () => {
    fake({ 'roots.request_grant': { error: 'internal_error', message: 'the service failed' } });
    await grantPicker.ask([]);
    expect(grantPicker).toMatchObject({ phase: 'failed', text: 'the service failed' });
  });

  it('busy is neutral: the dialog is still open, so it is not worded as a failure and a root that lands counts', async () => {
    fake({ 'roots.request_grant': { error: 'busy', message: 'a native dialog is already open' } });
    await grantPicker.ask(['rt_old']);
    expect(grantPicker.phase).toBe('deadline');
    expect(grantPicker.text).toMatch(/may still be open/);
    grantPicker.sync([root('rt_old'), root('rt_new', 'E:\\New')]);
    expect(grantPicker).toMatchObject({ phase: 'granted', rootId: 'rt_new' });
  });

  it.each([
    'a native dialog is already open.',
    '  a native dialog is already open  ',
    'A native dialog is already open.',
  ])('busy dialog-open variant %s stays neutral instead of failing', async (message) => {
    fake({ 'roots.request_grant': { error: 'busy', message } });
    await grantPicker.ask(['rt_old']);
    expect(grantPicker.phase).toBe('deadline');
    expect(grantPicker.text).toMatch(/may still be open/);
  });

  it.each([
    'native picker cooldown; retry shortly',
    'native dialog cannot open during cooldown; retry shortly',
  ])('a busy that is not an open dialog (%s) shows the service message and credits no later root', async (message) => {
    fake({ 'roots.request_grant': { error: 'busy', message } });
    await grantPicker.ask(['rt_old']);
    expect(grantPicker).toMatchObject({ phase: 'failed', text: message });
    grantPicker.sync([root('rt_old'), root('rt_other')]);
    expect(grantPicker.phase).toBe('failed');
    expect(grantPicker.rootId).toBeNull();
  });

  it('asks nothing until the first roots read has finished: no baseline, no "Added"', async () => {
    const seen = fake({ 'roots.request_grant': granted(root('rt_old')) });
    await grantPicker.ask(null);
    expect(seen).toHaveLength(0);
    expect(grantPicker.phase).toBe('failed');
    expect(grantPicker.rootId).toBeNull();
  });

  it('a root that lands after the deadline text is worded as added by the next roots refresh, without a second picker', async () => {
    const seen = fake({ 'roots.request_grant': { error: 'deadline_exceeded' }, 'roots.list': { roots: [root('rt_old')] } });
    await grantPicker.ask(['rt_old']);
    expect(grantPicker.phase).toBe('deadline');
    grantPicker.sync([root('rt_old')]); // an unrelated refresh changes nothing
    expect(grantPicker.phase).toBe('deadline');
    grantPicker.sync([root('rt_old'), root('rt_late', 'F:\\Late')]);
    expect(grantPicker).toMatchObject({ phase: 'granted', rootId: 'rt_late' });
    expect(grantPicker.text).toContain('Added F:\\Late');
    expect(seen.filter((s) => s.command === 'roots.request_grant')).toHaveLength(1);
  });

  it('an "Added" outcome is dropped when the root is revoked, and when the page is opened again', async () => {
    fake({ 'roots.request_grant': granted(root('rt_new')) });
    await grantPicker.ask([]);
    grantPicker.sync([root('rt_new')]);
    expect(grantPicker.phase).toBe('granted');
    grantPicker.sync([{ ...root('rt_new'), grant_state: 'revoked' } as Root]);
    expect(grantPicker).toMatchObject({ phase: 'idle', text: '', rootId: null });
    fake({ 'roots.request_grant': granted(root('rt_two')) });
    await grantPicker.ask([]);
    grantPicker.dismiss();
    expect(grantPicker).toMatchObject({ phase: 'idle', text: '', rootId: null });
  });

  it('a transport fault is handed to the shell and leaves the button usable', async () => {
    fake({ 'roots.request_grant': new TransportError('desktop engine not reachable') });
    await grantPicker.ask([]);
    expect(grantPicker.phase).toBe('failed');
    expect(grantPicker.text).toBe('desktop engine not reachable');
    expect(session.state).toBe('unavailable');
    session.state = 'connecting';
  });
});
