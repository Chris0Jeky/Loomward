import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { Client, LoomwardError } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import type { AlertItem } from '../../src/lib/contracts.gen';
import { formFrom, fromGib, measured, settingsFrom, STATUS, threshold, toGib } from '../../src/views/alerts/model';

const now = () => new Date('2026-10-17T12:00:00Z');
const client = () => new Client(createMockTransport({ now }));
const GiB = 2 ** 30;

describe('alerts model', () => {
  it('shows an unknown measurement as unknown, never zero, and a measured one with its unit', async () => {
    const { result } = await client().call('alerts.list', {});
    const [low, unseen, partial] = result.items as [AlertItem, AlertItem, AlertItem];
    expect(measured(low)).toBe('4.00 GiB free of 100 GiB');
    expect(threshold(low)).toBe('below 10.0 GiB');
    expect(measured(unseen)).toBe('unknown');
    expect(threshold(unseen)).toBe('none applied');
    expect(measured(partial)).toBe('unknown');
    expect(measured({ ...partial, kind: 'fast_growth', measured_bytes: String(7 * GiB) })).toBe('+7.00 GiB');
    expect(Object.keys(STATUS).sort()).toEqual(['alert', 'incomplete_coverage', 'insufficient_history', 'ok', 'unknown']);
  });

  it('converts GiB text to exact byte strings and back, and refuses anything else', () => {
    expect(fromGib('10')).toBe(String(10 * GiB));
    expect(fromGib('2.5')).toBe(String(2.5 * GiB));
    expect(fromGib('0')).toBe('0');
    for (const bad of ['', '-1', '1e3', 'ten', '1.234', '1,5', ' ']) expect(fromGib(bad), bad).toBeNull();
    expect(toGib(String(10 * GiB))).toBe('10');
    expect(toGib(String(2.5 * GiB))).toBe('2.5');
    expect(toGib(null as unknown as string)).toBe('');
  });

  it('builds the whole threshold record from the form, or names the first bad field', async () => {
    const { result } = await client().call('alerts.list', {});
    const form = formFrom(result.settings);
    expect(settingsFrom(form)).toEqual({ settings: result.settings });
    expect(settingsFrom({ ...form, percent: '0' })).toMatchObject({ field: 'percent' });
    expect(settingsFrom({ ...form, percent: '51' })).toMatchObject({ field: 'percent' });
    expect(settingsFrom({ ...form, freeGib: 'lots' })).toMatchObject({ field: 'freeGib' });
    expect(settingsFrom({ ...form, days: '91' })).toMatchObject({ field: 'days' });
    expect(settingsFrom({ ...form, growthGib: '' })).toMatchObject({ field: 'growthGib' });
    expect(settingsFrom({ ...form, notifications: true })).toMatchObject({ settings: { notifications_enabled: true } });
  });
});

describe('mock alerts transport', () => {
  it('starts with notifications off and stores a replacement record', async () => {
    const c = client();
    const first = (await c.call('alerts.list', {})).result;
    expect(first.settings.notifications_enabled).toBe(false);
    expect(first.settings).toEqual(first.defaults);
    expect(first.active_count).toBe(first.items.filter((i) => i.status === 'alert').length);
    const next = { ...first.settings, notifications_enabled: true, growth_window_days: 14 };
    const saved = (await c.call('alerts.configure', next)).result;
    expect(saved.settings).toEqual(next);
    expect(saved.defaults).toEqual(first.defaults);
    expect((await c.call('alerts.list', {})).result.settings).toEqual(next);
  });

  it('refuses out-of-range or missing thresholds', async () => {
    const c = client();
    const ok = (await c.call('alerts.list', {})).result.settings;
    for (const bad of [{ ...ok, low_space_free_percent: 0 }, { ...ok, growth_window_days: 91 }, { ...ok, growth_bytes: 5 }, { ...ok, extra: 1 }]) {
      await expect(c.call('alerts.configure', bad as never)).rejects.toBeInstanceOf(LoomwardError);
    }
    const { notifications_enabled: _drop, ...partial } = ok;
    await expect(c.call('alerts.configure', partial as never)).rejects.toMatchObject({ code: 'invalid_request' });
  });
});

// The View has no DOM harness in this repository, so this pins its save() against the same shape as load():
// one generation counter, a reply dropped when superseded, and an error shown only for the current request.
describe('alerts view save race', () => {
  it('guards save() with the generation load() uses, so a superseded reply cannot overwrite the form', () => {
    const source = readFileSync(new URL('../../src/views/alerts/View.svelte', import.meta.url), 'utf8');
    const save = source.slice(source.indexOf('async function save()'), source.indexOf('function reset()'));
    expect(save).toContain('const mine = ++generation;');
    const reply = save.indexOf('report = result;');
    expect(reply).toBeGreaterThan(save.indexOf('if (mine !== generation) return;'));
    expect(save.indexOf('if (mine !== generation) return;')).toBeGreaterThan(-1);
    expect(save).toContain('if (mine === generation) error = session.handle(e);');
  });
});
