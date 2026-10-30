import { describe, expect, it } from 'vitest';
import resultFixture from '../../../contracts/v3/examples/commands/reclaim.plan.result.json';
import { Client, LoomwardError } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { readFileSync } from 'node:fs';
import { ScriptTarget, transpileModule } from 'typescript';
import type { ReclaimPlan, ReclaimSelection, Volume } from '../../src/lib/contracts.gen';
import { observationAge } from '../../src/lib/format/time';
import { defaultVolume, GROUP_HEADINGS, selectionComparison, targetBytes, unaccountedText } from '../../src/views/reclaim/model';

describe('mock reclaim transport', () => {
  it('serves the contract example for the selected listed volume, with unknown relief intact', async () => {
    const client = new Client(createMockTransport());
    const { result: { volumes } } = await client.call('volumes.list', {});
    const { result, meta } = await client.call('reclaim.plan', { volume_id: volumes[0]!.volume_id, limit: 50 });
    expect(result).toEqual({ ...resultFixture, volume: { ...resultFixture.volume, volume_id: volumes[0]!.volume_id } });
    expect(result.groups[0]!.candidates[0]!.reclaimable_bytes).toBeNull();
    expect(meta.catalog_rev).not.toBeNull();
    expect(meta.state_rev).not.toBeNull();
  });

  it('refuses unknown volumes, invalid limits and extra fields', async () => {
    const client = new Client(createMockTransport());
    const volume_id = (await client.call('volumes.list', {})).result.volumes[0]!.volume_id;
    await expect(client.call('reclaim.plan', { volume_id: 'missing', limit: 50 })).rejects.toMatchObject({ code: 'not_found' });
    for (const payload of [{ volume_id, limit: 0 }, { volume_id, limit: 201 }, { volume_id, limit: 1.5 }, { volume_id, limit: 50, execute: true }]) {
      await expect(client.call('reclaim.plan', payload as never)).rejects.toBeInstanceOf(LoomwardError);
    }
  });
});

describe('reclaim display model', () => {
  it('compares exact byte targets and keeps unknown, expired and zero separate', () => {
    const result: ReclaimSelection = { members: ['nd_a'], status: 'recorded', observed_at: '2026-10-29T00:00:00Z', reclaimable_bytes: '9007199254740993', reason: null };
    expect(selectionComparison(result, '9007199254740992')).toContain('1 bytes above');
    expect(selectionComparison(result, '9007199254740994')).toContain('1 bytes short');
    expect(selectionComparison({ ...result, reclaimable_bytes: '0' }, '0')).toContain('covers');
    expect(selectionComparison({ ...result, reclaimable_bytes: null }, '0')).toBe('Target coverage is unknown.');
    expect(selectionComparison({ ...result, status: 'expired' }, '0')).toBe('Target coverage is unknown.');
    for (const invalid of ['', '-1', '1.5', '1e3', '01', '18446744073709551616']) expect(targetBytes(invalid)).toBeNull();
    expect(targetBytes('18446744073709551615')).toBe(18446744073709551615n);
    expect(observationAge(result.observed_at!, Date.parse('2026-10-29T00:02:00Z'))).toBe('2 minutes ago');
  });
  it('words a stored verdict age in the largest whole unit, so an old figure reads as old', () => {
    const at = '2026-10-29T00:00:00Z';
    const after = (ms: number) => observationAge(at, Date.parse(at) + ms);
    expect(observationAge('not a time', Date.parse(at))).toBe('at an unknown time');
    expect(after(30_000)).toBe('less than a minute ago');
    expect(after(59 * 60_000)).toBe('59 minutes ago');
    expect(after(60 * 60_000)).toBe('1 hour ago');
    expect(after(3 * 3_600_000 + 59 * 60_000)).toBe('3 hours ago');
    expect(after(47 * 3_600_000)).toBe('47 hours ago');
    expect(after(48 * 3_600_000)).toBe('2 days ago');
    expect(after(-5_000)).toBe('at an unknown time (clock ahead)'); // a future time never reads as fresh
  });
  it('names the four groups in contract order', () => {
    expect(Object.keys(GROUP_HEADINGS)).toEqual(resultFixture.groups.map((g) => g.kind));
    expect(Object.values(GROUP_HEADINGS)).toEqual(['Tool-owned cleanup', 'Relocate', 'Archive candidates', 'Informational']);
  });

  it('defaults to least known free bytes, retaining exact integers and unknowns', () => {
    const volume = (volume_id: string, free_bytes: string | null) => ({ volume_id, free_bytes } as Volume);
    expect(defaultVolume([volume('unknown', null), volume('more', '9007199254740993'), volume('least', '9007199254740992')])).toBe('least');
    expect(defaultVolume([volume('unknown', null), volume('empty', '0')])).toBe('empty');
    expect(defaultVolume([volume('first', null), volume('second', null)])).toBe('first');
    expect(defaultVolume([])).toBe('');
  });

  it('shows the service unaccounted value, or a short explanation for an unknown', () => {
    const report = resultFixture as ReclaimPlan;
    expect(unaccountedText(report)).toBe('Unaccounted: 68.3 KiB of volume used is outside the covered roots.');
    expect(unaccountedText({ ...report, unaccounted_bytes: null })).toBe('Unaccounted: unknown — coverage or volume measurements are partial or unknown.');
    expect(unaccountedText({ ...report, unaccounted_bytes: '0' })).toContain('0 B');
  });
});

describe('reclaim view', () => {
  const source = readFileSync(new URL('../../src/views/reclaim/View.svelte', import.meta.url), 'utf8');

  function reader(call: (command: string, payload: unknown) => Promise<unknown>) {
    const read = source.slice(source.indexOf('  async function read('), source.indexOf('\n  $effect('));
    return new Function('session', `
      let report = null, loading = true, error = '', generation = 0, members = [], selectionResult = null;
      ${transpileModule(read, { compilerOptions: { target: ScriptTarget.ES2022 } }).outputText}
      return { read, state: () => ({ report, loading, error }) };
    `)({ client: { call }, handle: String }) as {
      read: (id: string) => Promise<void>;
      state: () => { report: ReclaimPlan | null; loading: boolean; error: string };
    };
  }

  it('offers read-only selection, target and observe/cancel controls, with owner steps as text', () => {
    expect(source.match(/<button\b/g)).toHaveLength(7);
    expect(source.match(/<select\b/g)).toHaveLength(1);
    expect(source).toContain('>Look again</button>');
    expect(source).toContain('>Observe hard links</button>');
    expect(source).toContain('>Cancel observation</button>');
    expect(source).toContain('>Record before</button>');
    expect(source).toContain('>Check now</button>');
    expect(source).toContain('Owner step: {candidate.owner_step}</p>');
    expect(source.match(/<input\b/g)).toHaveLength(2);
    expect(source).toContain('type="checkbox"');
    expect(source).toContain('inputmode="numeric"');
    expect(source).not.toMatch(/<a\b|<form\b|{@html|clipboard|exec\(|invoke\(/);
    expect([...source.matchAll(/c.call\('([^']+)'/g)].map((m) => m[1])).toEqual(['reclaim.plan', 'volumes.list', 'reclaim.sample']);
  });

  it('the observe store calls only the read-only job commands and the plan', () => {
    const store = readFileSync(new URL('../../src/lib/stores/reclaim-observe.svelte.ts', import.meta.url), 'utf8');
    const allowed = ['jobs.list', 'jobs.get', 'reclaim.observe', 'jobs.cancel', 'reclaim.plan', 'volumes.list'];
    const used = [...store.matchAll(/\.call\('([^']+)'/g)].map((m) => m[1]);
    expect(used.length).toBeGreaterThan(0);
    expect(used.filter((c) => !allowed.includes(c!))).toEqual([]);
  });

  it('clears old evidence during a new read, and drops superseded replies', async () => {
    let finish!: (value: unknown) => void;
    const pending = new Promise((resolve) => { finish = resolve; });
    const first = { ...resultFixture, volume: { ...resultFixture.volume, volume_id: 'first' } };
    const next = { ...first, volume: { ...first.volume, volume_id: 'next' } };
    const calls: unknown[] = [];
    const view = reader(async (command, payload) => {
      calls.push([command, payload]);
      return (payload as { volume_id: string }).volume_id === 'slow' ? pending : { result: calls.length === 1 ? first : next };
    });
    await view.read('first');
    expect(view.state().report).toEqual(first);
    const slow = view.read('slow');
    expect(view.state()).toMatchObject({ loading: true, report: null });
    await view.read('next');
    finish({ result: first });
    await slow;
    expect(view.state()).toMatchObject({ loading: false, report: next, error: '' });
    expect(calls[0]).toEqual(['reclaim.plan', { volume_id: 'first', limit: 50 }]);
  });

  it('ends loading without a volume, and leaves no report on a failed read', async () => {
    let calls = 0;
    const view = reader(async () => { calls++; throw new Error('plan refused'); });
    await view.read('');
    expect(view.state()).toEqual({ report: null, loading: false, error: '' });
    expect(calls).toBe(0);
    await view.read('missing');
    expect(view.state()).toEqual({ report: null, loading: false, error: 'Error: plan refused' });
  });
});
