import { afterEach, expect, it, vi } from 'vitest';
import { Client, LoomwardError } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { warpOf, weaveKey } from '../../viz/colour.js';
import { warpBreak } from '../../viz/weave.js';
import type { Palette } from '../../viz/types.js';
import type { Warp } from '../../viz/colour.js';
import { isHumanMeaning, Meaning, overlayMeaning } from '../../src/lib/stores/meaning.svelte';
import type { MeaningResult } from '../../src/lib/contracts.gen';
const wovenAtlas = { dyes: ['#abc', '#def'], undyed: '#888', neutral: '#777' } as Palette;

it('mock meaning returns ranked advice and neutral abstention, bounded to a prefix', async () => {
  const c = new Client(createMockTransport());
  const slice = (await c.call('tree.slice', { anchor: { kind: 'atlas' }, depth: 8, max_nodes: 6000, min_share: 0, basis: 'logical', include_files: true })).result;
  const result = (await c.call('threads.meaning', { node_ids: slice.nodes.map(n => n.node_id) })).result;
  expect(result.model_id).not.toBeNull();
  expect(result.budget_hit).toBe(true);
  expect(result.items).toHaveLength(1000);
  const suggested = result.items.find(i => i.meaning.state === 'suggested')!.meaning;
  const abstained = result.items.find(i => i.meaning.state === 'abstained')!.meaning;
  expect(suggested.suggestion!.ranked[0]!.score).toBeGreaterThan(0);
  expect(suggested.suggestion!.autonomy_allowed).toBe(false);
  expect(abstained.label).toBeNull();
  expect(abstained.suggestion!.reasons).toContain('unfamiliar_features');
  expect(warpOf(abstained, wovenAtlas).colour).toBe(wovenAtlas.undyed);
  await expect(c.call('threads.meaning', { node_ids: [slice.nodes[0]!.node_id, slice.nodes[0]!.node_id] })).rejects.toThrow();
});

it('relative score changes the suggestion intensity; abstention never receives dye', () => {
  const a = warpOf({ state: 'suggested', label: 'Documents', share: 0.2 }, wovenAtlas);
  const b = warpOf({ state: 'suggested', label: 'Documents', share: 0.9 }, wovenAtlas);
  expect(a.intensity).toBe(0.25);
  expect(b.intensity).toBe(1);
  expect(warpOf({ state: 'abstained', label: 'Documents', share: 0.9 }, wovenAtlas).colour).toBe(wovenAtlas.undyed);
});

it('lazy projection requests only supplied visible IDs and drops replies after navigation', async () => {
  const pending: { ids: string[]; signal: AbortSignal; reply: (v: { result: MeaningResult }) => void }[] = [];
  const c = { call: (_: string, p: { node_ids: string[] }, o: { signal: AbortSignal }) => new Promise(resolve => pending.push({ ids: p.node_ids, signal: o.signal, reply: resolve })) } as unknown as Client;
  const meaning = new Meaning();
  const old = meaning.load(c, ['nd_visible', 'nd_visible']);
  expect(pending[0]!.ids).toEqual(['nd_visible']);
  const fresh = meaning.load(c, ['nd_next']);
  expect(pending[0]!.signal.aborted).toBe(true);
  pending[1]!.reply({ result: { model_id: null, items: [], budget_hit: false } });
  await fresh;
  expect(meaning.message).toContain('No fitted model');
  pending[0]!.reply({ result: { model_id: 'md_old', items: [], budget_hit: true } });
  await old;
  expect(meaning.message).toContain('No fitted model');
  expect(meaning.busy).toBe(false);
  meaning.clear();
  expect(pending[1]!.signal.aborted).toBe(true);
  expect(meaning.items).toEqual({});
  expect(meaning.message).toBe('');
  const bounded = meaning.load(c, Array.from({ length: 1001 }, (_, i) => `nd_visible_${i}`));
  expect(pending[2]!.ids).toHaveLength(1000);
  pending[2]!.reply({ result: { model_id: 'md_current', items: [], budget_hit: false } });
  await bounded;
  expect(meaning.message).toContain('unreturned nodes remain unknown');
  meaning.clear();
});

it('intensity is quantised to the eight-thread tile, so continuous scores share few tiles', () => {
  const weft = { pattern: 'missing', colour: null } as never;
  const keys = new Set<string>();
  for (let i = 0; i < 1000; i++) keys.add(weaveKey(warpOf({ state: 'suggested', label: 'Documents', share: 0.8 + Math.random() * 0.2 }, wovenAtlas), weft, false));
  expect(keys.size).toBeLessThanOrEqual(9);
  const all = new Set<number | undefined>();
  for (let i = 0; i < 1000; i++) all.add(warpOf({ state: 'suggested', label: 'Documents', share: Math.random() }, wovenAtlas).intensity);
  expect(all.size).toBeLessThanOrEqual(9);
  expect(warpOf({ state: 'suggested', label: 'Documents', share: 0 }, wovenAtlas).intensity).toBe(0);
});

afterEach(() => vi.useRealTimers());

function recording() {
  const calls: { ids: string[]; reply: (v: { result: MeaningResult }) => void }[] = [];
  const c = { call: (_: string, p: { node_ids: string[] }) => new Promise(resolve => calls.push({ ids: p.node_ids, reply: resolve })) } as unknown as Client;
  return { calls, c };
}

it('a burst of resize-driven loads sends one request for the last list after the debounce', async () => {
  vi.useFakeTimers();
  const { calls, c } = recording();
  const meaning = new Meaning();
  for (let i = 0; i < 20; i++) { meaning.loadSoon(c, [`nd_${i}`]); await vi.advanceTimersByTimeAsync(50); }
  expect(calls).toHaveLength(0);
  await vi.advanceTimersByTimeAsync(200);
  expect(calls).toHaveLength(1);
  expect(calls[0]!.ids).toEqual(['nd_19']);
  meaning.loadSoon(c, ['nd_late']);
  meaning.clear();
  await vi.advanceTimersByTimeAsync(500);
  expect(calls).toHaveLength(1);
});

it('a reload keeps the previous items until the replacement arrives', async () => {
  const { calls, c } = recording();
  const meaning = new Meaning();
  const thread = (label: string) => ({ state: 'labelled', label }) as never;
  const first = meaning.load(c, ['nd_a']);
  calls[0]!.reply({ result: { model_id: 'md', items: [{ node_id: 'nd_a', meaning: thread('A') }], budget_hit: false } });
  await first;
  const second = meaning.load(c, ['nd_a', 'nd_b']);
  expect(meaning.items['nd_a']).toBeDefined();
  expect(meaning.busy).toBe(true);
  calls[1]!.reply({ result: { model_id: 'md', items: [{ node_id: 'nd_a', meaning: thread('A2') }], budget_hit: false } });
  await second;
  expect(meaning.items['nd_a']!.label).toBe('A2');
});

it('a store covers a node while a request is in flight or once it answered for it, and not when it left the node out', async () => {
  const { calls, c } = recording();
  const meaning = new Meaning();
  expect(meaning.covers('nd_a')).toBe(false);
  const p = meaning.load(c, ['nd_a', 'nd_b']);
  expect(meaning.covers('nd_a')).toBe(true);
  calls[0]!.reply({ result: { model_id: 'md', items: [{ node_id: 'nd_a', meaning: { state: 'abstained' } as never }], budget_hit: true } });
  await p;
  expect(meaning.covers('nd_a')).toBe(true);
  // nd_b was omitted by the budget: the inspector must fetch it itself
  expect(meaning.covers('nd_b')).toBe(false);
  meaning.clear();
  expect(meaning.covers('nd_a')).toBe(false);
});

it('after any failure the inspector can fetch for itself', async () => {
  const failing = new Meaning();
  await failing.load({ call: () => Promise.reject(new Error('boom')) } as unknown as Client, ['nd_a']);
  expect(failing.message).toBe('boom');
  expect(failing.covers('nd_a')).toBe(false);
  const other = new Meaning();
  await other.load({ call: () => Promise.reject(new LoomwardError({ code: 'capability_unavailable', message: 'x', retryable: false, detail: null })) } as unknown as Client, ['nd_a']);
  expect(other.covers('nd_a')).toBe(false);
});

it('the undyed meaning states keep the neutral colour but weave differently, so the canvas tells them apart', () => {
  const states = ['none', 'no_model', 'abstained', 'not_eligible', 'pending'] as const;
  const warps = states.map(state => warpOf({ state, label: null, share: null }, wovenAtlas));
  expect(new Set(warps.map(w => w.pattern)).size).toBe(states.length);
  expect(new Set(warps.map(w => weaveKey(w, { pattern: 'missing', colour: null } as never, false))).size).toBe(states.length);
  for (const w of warps) expect(w.colour).toBe(wovenAtlas.undyed);
  const crossings = (pattern: Warp['pattern']) => Array.from({ length: 64 }, (_, k) => warpBreak(pattern, k % 8, Math.floor(k / 8)));
  const rhythms = new Set(warps.map(w => crossings(w.pattern).join('')));
  expect(rhythms.size).toBe(states.length);
  expect(crossings('undyed').some(Boolean)).toBe(false);
  expect(crossings('not_eligible').every(Boolean)).toBe(true);
});

const thread = (state: string, label: string | null = null, source: string | null = null) => ({ state, label, share: null, source, collection_ids: [] }) as never;

it('a student suggestion never replaces a human meaning already in the slice', () => {
  const node = (id: string, state: string, label: string | null = null, source: string | null = null) => ({ node_id: id, threads: { meaning: thread(state, label, source), residency: 'r' } });
  // nd_s is a system aggregate and nd_t a teacher label: neither is human, so the projection overlays both.
  const slice = { anchor: 'a', nodes: [node('nd_h', 'labelled', 'Projects', 'human'), node('nd_m', 'mixed', 'Media', 'human'), node('nd_s', 'mixed'), node('nd_t', 'labelled', 'Media', 'teacher'), node('nd_f', 'none'), node('nd_x', 'none')] } as never;
  const items = Object.fromEntries(['nd_h', 'nd_m', 'nd_s', 'nd_t', 'nd_f'].map(id => [id, thread('suggested', 'Documents')]));
  const out = overlayMeaning(slice, items) as unknown as { nodes: { node_id: string; threads: { meaning: { state: string; label: string | null }; residency: string } }[] };
  const by = Object.fromEntries(out.nodes.map(n => [n.node_id, n.threads]));
  expect(by.nd_h!.meaning).toMatchObject({ state: 'labelled', label: 'Projects' });
  expect(by.nd_m!.meaning).toMatchObject({ state: 'mixed', label: 'Media' });
  expect(by.nd_s!.meaning).toMatchObject({ state: 'suggested', label: 'Documents' });
  expect(by.nd_t!.meaning).toMatchObject({ state: 'suggested', label: 'Documents' });
  expect(by.nd_f!.meaning).toMatchObject({ state: 'suggested', label: 'Documents' });
  expect(by.nd_x!.meaning.state).toBe('unknown');
  expect(by.nd_h!.residency).toBe('r');
  expect(isHumanMeaning(thread('labelled', 'Projects', 'human'))).toBe(true);
  expect(isHumanMeaning(thread('labelled'))).toBe(false);
  expect(isHumanMeaning(thread('mixed'))).toBe(false);
  expect(isHumanMeaning(thread('suggested'))).toBe(false);
});

it('the mock reports a hit work budget in the response meta, as the service does', async () => {
  const c = new Client(createMockTransport());
  const slice = (await c.call('tree.slice', { anchor: { kind: 'atlas' }, depth: 8, max_nodes: 6000, min_share: 0, basis: 'logical', include_files: true })).result;
  const ids = slice.nodes.map(n => n.node_id);
  const big = await c.call('threads.meaning', { node_ids: ids });
  expect(big.result.budget_hit).toBe(true);
  expect(big.meta.budget_hit).toBe(true);
  const small = await c.call('threads.meaning', { node_ids: ids.slice(0, 10) });
  expect(small.result.budget_hit).toBe(false);
  expect(small.meta.budget_hit).toBe(false);
});
