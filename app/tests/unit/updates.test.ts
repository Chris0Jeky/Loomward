import { describe, expect, it } from 'vitest';
import { Updates, type UpdateStatus } from '../../src/lib/stores/updates.svelte';

const base: UpdateStatus = {
  current_version: '0.3.0-nightly.20261012.4',
  unavailable: null,
  prefs: { check_on_start: false, channel: 'preview' },
  offer: null,
  last_error: null,
};

/** A fake `lw_update` that applies prefs and answers checks from `offer`. */
function desktop(start: Partial<UpdateStatus> = {}, offer: UpdateStatus['offer'] = null) {
  let state: UpdateStatus = { ...base, ...start };
  const calls: Record<string, unknown>[] = [];
  const invoke = async (cmd: string, args: Record<string, unknown>) => {
    expect(cmd).toBe('lw_update');
    const r = args.request as Record<string, unknown>;
    calls.push(r);
    if (r.action === 'set_prefs') {
      const channel = r.channel as 'preview' | 'nightly';
      state = { ...state, prefs: { check_on_start: r.check_on_start as boolean, channel }, offer: channel === state.prefs.channel ? state.offer : null };
    }
    if (r.action === 'check') state = { ...state, offer };
    if (r.action === 'install') return { installed: false };
    return state;
  };
  return { updates: new Updates(invoke), calls };
}

describe('Updates view state', () => {
  it('starts off and on the preview channel, and loading never checks', async () => {
    const { updates, calls } = desktop();
    await updates.load();
    expect(updates.status?.prefs).toEqual({ check_on_start: false, channel: 'preview' });
    expect(calls.map((c) => c.action)).toEqual(['status']);
  });

  it('the toggle and the channel select save prefs without making a request', async () => {
    const { updates, calls } = desktop();
    await updates.load();
    await updates.setCheckOnStart(true);
    await updates.setChannel('nightly');
    await updates.setChannel('nightly'); // unchanged: nothing sent
    expect(calls).toEqual([
      { action: 'status' },
      { action: 'set_prefs', check_on_start: true, channel: 'preview' },
      { action: 'set_prefs', check_on_start: true, channel: 'nightly' },
    ]);
    expect(updates.status?.prefs).toEqual({ check_on_start: true, channel: 'nightly' });
    expect(calls.some((c) => c.action === 'check')).toBe(false);
  });

  it('checks only on the button, words the offer, and installs exactly the version shown', async () => {
    const offer = { version: '0.3.0-nightly.20261013.5', channel: 'nightly' as const, notes: 'NIGHTLY', date: null, previous: false };
    const { updates, calls } = desktop({ prefs: { check_on_start: false, channel: 'nightly' } }, offer);
    await updates.load();
    await updates.check();
    expect(calls.at(-1)).toEqual({ action: 'check', previous: false });
    expect(updates.message).toBe('Update 0.3.0-nightly.20261013.5 is available on the nightly channel.');
    await updates.install();
    expect(calls.at(-1)).toEqual({ action: 'install', version: '0.3.0-nightly.20261013.5' });
    expect(updates.message).toBe('Not installed: you cancelled.');
  });

  it('asks for the previous nightly and reports none', async () => {
    const { updates, calls } = desktop({ prefs: { check_on_start: false, channel: 'nightly' } });
    await updates.load();
    await updates.check(true);
    expect(calls.at(-1)).toEqual({ action: 'check', previous: true });
    expect(updates.message).toBe('No other previous nightly is published.');
  });

  it('shows the previous nightly but never asks the desktop to install it', async () => {
    const prev = { version: '0.3.0-nightly.20261012.3', channel: 'nightly' as const, notes: '', date: null, previous: true };
    const { updates, calls } = desktop({ prefs: { check_on_start: false, channel: 'nightly' } }, prev);
    await updates.load();
    await updates.check(true);
    expect(updates.message).toBe('Previous nightly 0.3.0-nightly.20261012.3 is available on the nightly channel.');
    await updates.install();
    expect(calls.some((c) => c.action === 'install')).toBe(false);
  });

  it('an unconfigured build never sends a check', async () => {
    const { updates, calls } = desktop({ unavailable: 'updates are not configured in this build' });
    await updates.load();
    await updates.check();
    expect(calls.map((c) => c.action)).toEqual(['status']);
  });

  it('a failed check on start is shown when the view loads', async () => {
    const { updates } = desktop({ prefs: { check_on_start: true, channel: 'nightly' }, last_error: 'error sending request' });
    await updates.load();
    expect(updates.message).toBe('The last check failed: error sending request');
  });

  it('a failed check is reported, not swallowed', async () => {
    const updates = new Updates(async (_c, a) => {
      if ((a.request as { action: string }).action === 'check') throw 'error sending request';
      return base;
    });
    await updates.load();
    await updates.check();
    expect(updates.message).toBe('The check failed: error sending request');
  });

  it('outside the desktop there is no updater and nothing is sent', async () => {
    const updates = new Updates(null);
    await updates.load();
    await updates.check();
    expect(updates.desktop).toBe(false);
    expect(updates.status).toBeNull();
  });

  it('an update check while another update action is busy leaves the message unchanged', async () => {
    const calls: Record<string, unknown>[] = [];
    let holdNext = false;
    let release: (s: UpdateStatus) => void = () => {};
    const updates = new Updates(async (_c, args) => {
      const r = args.request as Record<string, unknown>;
      calls.push(r);
      if (holdNext) {
        holdNext = false;
        return new Promise<UpdateStatus>((resolve) => {
          release = resolve;
        });
      }
      return base;
    });
    await updates.load(); // status known, nothing busy
    holdNext = true;
    const pending = updates.load(); // busy = 'loading' while the reply is held
    expect(updates.busy).toBe('loading');
    const before = updates.message;
    const sent = calls.length;
    await updates.check(); // refused while busy: no request, no message change
    expect(updates.message).toBe(before);
    expect(calls.slice(sent).some((c) => c.action === 'check')).toBe(false);
    release(base);
    await pending;
    expect(updates.busy).toBe('');
    await updates.check();
    expect(calls.at(-1)).toEqual({ action: 'check', previous: false });
    expect(updates.message).toBe('You have the newest preview build.');
  });

  it('an install while another update action is busy leaves the message unchanged', async () => {
    const offer = { version: '0.3.1-preview.1', channel: 'preview' as const, notes: '', date: null, previous: false };
    const offered: UpdateStatus = { ...base, offer };
    const calls: Record<string, unknown>[] = [];
    let release: (s: UpdateStatus) => void = () => {};
    let hold = false;
    const updates = new Updates(async (_c, args) => {
      const r = args.request as Record<string, unknown>;
      calls.push(r);
      if (r.action === 'install') return { installed: false };
      if (hold) { hold = false; return new Promise<UpdateStatus>((resolve) => { release = resolve; }); }
      return offered;
    });
    await updates.load();
    hold = true;
    const pending = updates.load();
    const before = updates.message;
    await updates.install(); // refused while busy: no confirmation box, no message change
    expect(updates.message).toBe(before);
    expect(calls.some((c) => c.action === 'install')).toBe(false);
    release(offered);
    await pending;
    await updates.install();
    expect(calls.at(-1)).toEqual({ action: 'install', version: '0.3.1-preview.1' });
    expect(updates.message).toBe('Not installed: you cancelled.');
  });
});
