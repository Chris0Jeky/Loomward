import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { Client, LoomwardError } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import type { DeveloperClutterReport } from '../../src/lib/contracts.gen';
import { allocatedText, countText, KIND_WORDS, outcome, PROVIDER_WORDS, sizeText, TOPIC_WORDS } from '../../src/views/clutter/model';

const now = () => new Date('2026-10-17T12:00:00Z');
const client = () => new Client(createMockTransport({ now }));

async function rootIds(c: Client): Promise<string[]> {
  return (await c.call('roots.list', {})).result.roots.filter((r) => r.grant_state === 'active').map((r) => r.root_id);
}

describe('developer clutter model', () => {
  it('shows an unknown size as unknown, never zero, and says why there is no on-disk size', async () => {
    const c = client();
    const [first] = await rootIds(c);
    const { result } = await c.call('clutter.developer', { root_id: first!, limit: 50 });
    const [known, unknown] = result.candidates;
    expect(sizeText(known!)).toBe('2.00 GiB');
    expect(allocatedText(known!)).toBe('2.00 GiB on disk');
    expect(sizeText(unknown!)).toBe('unknown');
    expect(allocatedText(unknown!)).toBe('on-disk size unknown');
  });

  it('has a word for every kind, provider and not-assessed topic in the contract', () => {
    expect(Object.keys(KIND_WORDS).sort()).toEqual(['build_cache', 'build_output', 'bytecode_cache', 'dependency_install', 'environment']);
    expect(Object.keys(PROVIDER_WORDS).sort()).toEqual(['cargo', 'dotnet', 'gradle', 'npm', 'python']);
    expect(Object.keys(TOPIC_WORDS).sort()).toEqual(['model_host_lifecycle', 'regeneration_cost', 'stale_worktrees']);
  });

  it('never reads an empty answer as clean when the search ran out of budget or the scan was partial', () => {
    const base = { candidates: [], partial: false, truncated: false, budget_hit: false } as unknown as DeveloperClutterReport;
    expect(outcome(base)).toBe('none');
    expect(outcome({ ...base, partial: true })).toBe('none-partial');
    expect(outcome({ ...base, partial: true, budget_hit: true })).toBe('budget');
    expect(countText({ ...base, candidates: [1] as never })).toBe('1 folder');
    expect(countText({ ...base, candidates: [1, 2] as never, truncated: true })).toBe('2+ folders');
  });
});

describe('mock clutter transport', () => {
  it('lists candidates largest known size first, anchors each and names what is not assessed', async () => {
    const c = client();
    const [first] = await rootIds(c);
    const { result } = await c.call('clutter.developer', { root_id: first!, limit: 50 });
    expect(result.root_id).toBe(first);
    expect(result.candidates.map((x) => x.name)).toEqual(['target', 'node_modules']);
    expect(result.candidates.every((x) => x.anchor_name && x.regeneration_cost === 'unknown')).toBe(true);
    expect(result.not_assessed.map((n) => n.topic)).toEqual(['regeneration_cost', 'model_host_lifecycle', 'stale_worktrees']);
    const one = (await c.call('clutter.developer', { root_id: first!, limit: 1 })).result;
    expect(one.candidates).toHaveLength(1);
    expect(one.truncated).toBe(true);
  });

  it('answers a partial empty list for a root with no evidence and budget_hit for the hostile root, never a clean empty', async () => {
    const c = client();
    const ids = await rootIds(c);
    const other = (await c.call('clutter.developer', { root_id: ids[1]!, limit: 50 })).result;
    expect(other.candidates).toEqual([]);
    expect(other.partial).toBe(true);
    expect(other.budget_hit).toBe(false);
    const budget = (await c.call('clutter.developer', { root_id: 'rt_mock_9', limit: 50 })).result;
    expect(budget.budget_hit).toBe(true);
    expect(budget.candidates).toEqual([]);
    expect(budget.budget_note).toContain('says nothing');
  });

  it('refuses an unknown root and out-of-range or extra fields', async () => {
    const c = client();
    const [first] = await rootIds(c);
    await expect(c.call('clutter.developer', { root_id: 'rt_nope', limit: 5 })).rejects.toMatchObject({ code: 'not_found' });
    for (const bad of [{ root_id: first!, limit: 0 }, { root_id: first!, limit: 201 }, { root_id: first!, limit: 5, extra: 1 }]) {
      await expect(c.call('clutter.developer', bad as never)).rejects.toBeInstanceOf(LoomwardError);
    }
  });
});

// The View has no DOM harness in this repository, so this pins the properties that must never regress by source.
describe('developer clutter view', () => {
  const source = readFileSync(new URL('../../src/views/clutter/View.svelte', import.meta.url), 'utf8');

  it('offers no action on a folder: only a root choice and a read-again button', () => {
    expect(source.match(/<button/g)).toHaveLength(1);
    expect(source).toContain('Look again');
    expect(source).not.toMatch(/delete|remove now|clean up|free up|reclaim/i);
    expect(source).toContain('You could remove these; Loomward will not.');
  });

  it('drops a reply superseded by a newer read, and treats a budget hit as its own state', () => {
    expect(source).toContain('const mine = ++generation;');
    expect(source).toContain('if (mine === generation) report = result;');
    expect(source).toContain("shape === 'budget'");
  });

  it('shows rebuild cost as not assessed rather than a value', () => {
    expect(source).toContain('Rebuild cost: not assessed yet.');
    expect(source).toContain('Not assessed yet');
  });
});
