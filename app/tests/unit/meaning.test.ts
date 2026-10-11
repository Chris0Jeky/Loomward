import { expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { warpOf } from '../../viz/colour.js';
import type { Palette } from '../../viz/types.js';
import { Meaning } from '../../src/lib/stores/meaning.svelte';
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
  expect(a.intensity).toBe(0.2);
  expect(b.intensity).toBe(0.9);
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
