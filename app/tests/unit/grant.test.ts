import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import type { ErrorCode, Root } from '../../src/lib/contracts.gen';
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

describe('"Add a folder…" state machine', () => {
  beforeEach(() => { grantPicker.phase = 'idle'; grantPicker.text = ''; grantPicker.rootId = null; });
  afterEach(() => { session.client = null; });

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
    expect(grantPicker.text).toMatch(/still open/);
    grantPicker.sync([root('rt_old'), root('rt_new', 'E:\New')]);
    expect(grantPicker).toMatchObject({ phase: 'granted', rootId: 'rt_new' });
  });

  it('a busy that is not an open dialog (cooldown, mutation capacity) shows the service message and credits no later root', async () => {
    fake({ 'roots.request_grant': { error: 'busy', message: 'native picker cooldown; retry shortly' } });
    await grantPicker.ask(['rt_old']);
    expect(grantPicker).toMatchObject({ phase: 'failed', text: 'native picker cooldown; retry shortly' });
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
    grantPicker.sync([root('rt_old'), root('rt_late', 'F:\Late')]);
    expect(grantPicker).toMatchObject({ phase: 'granted', rootId: 'rt_late' });
    expect(grantPicker.text).toContain('Added F:\Late');
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

  it('opening the page again keeps a dialog that may still be open worded', async () => {
    fake({ 'roots.request_grant': { error: 'deadline_exceeded' }, 'roots.list': { roots: [] } });
    await grantPicker.ask([]);
    grantPicker.dismiss();
    expect(grantPicker.phase).toBe('deadline');
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
