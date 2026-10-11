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

  it('shows the service message for busy and other engine errors', async () => {
    fake({ 'roots.request_grant': { error: 'busy', message: 'a native dialog is already open' } });
    await grantPicker.ask([]);
    expect(grantPicker).toMatchObject({ phase: 'failed', text: 'a native dialog is already open' });
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
