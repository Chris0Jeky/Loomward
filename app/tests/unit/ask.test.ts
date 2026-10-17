import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
import { describe, expect, it, vi } from 'vitest';
import * as askModel from '../../src/views/ask/model';
import { Client, LoomwardError } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { MOCK_ASK_PROFILE_DIGEST_INPUT } from '../../src/lib/transport/mock-ask';
import type { AskStatus, AskTurnRecord, Root } from '../../src/lib/contracts.gen';
import { piecesOf, providerOf, scopeOf, scopeSentence } from '../../src/views/ask/model';

const client = () => new Client(createMockTransport({ count: 400 }));

async function settle(c: Client, t: AskTurnRecord): Promise<AskTurnRecord> {
  for (let i = 0; i < 100; i++) {
    const { result } = await c.call('ask.result', { turn_id: t.turn_id });
    if (result.state !== 'running') return result;
    await new Promise((r) => setTimeout(r, 20));
  }
  throw new Error('the mock turn never settled');
}

describe('ask on the mock transport', () => {
  it('pins the fake profile digest to ProviderProfile::digest', async () => {
    const { result } = await client().call('ask.status', {});
    expect(result.profile_digest).toBe('sha256:' + createHash('sha256').update(MOCK_ASK_PROFILE_DIGEST_INPUT).digest('hex'));
    expect(result.tools.every((t) => t.read_only === true)).toBe(true);
    expect(result.grant).toBeNull();
  });

  it('refuses a turn without a grant, then answers with ledger citations, an unverified handle and a refused write', async () => {
    const c = client();
    const err = await c.call('ask.turn', { question: 'where is delta?' }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(LoomwardError);
    expect((err as LoomwardError).code).toBe('permission_denied');
    const roots = (await c.call('roots.list', {})).result.roots.filter((r) => r.grant_state === 'active');
    await c.call('ask.grant', { roots: [roots[0]!.root_id], fields: ['path'] });
    const running = (await c.call('ask.turn', { question: 'please delete delta, or guess' })).result;
    expect(running.state).toBe('running');
    const t = await settle(c, running);
    expect(t.state).toBe('completed');
    expect(t.citations.length).toBeGreaterThan(0);
    for (const cite of t.citations) {
      expect(cite.root_id).toBe(roots[0]!.root_id);
      expect((await c.call('node.inspect', { node_id: cite.node_id })).result.row.name).toBe(cite.name);
    }
    expect(t.unverified).toEqual(['h99']);
    expect(t.tool_calls.find((u) => u.name === 'delete')).toMatchObject({ ok: false, detail: 'unknown tool' });
    expect(t.rounds.every((r) => /^sha256:[0-9a-f]{64}$/.test(r.sha256) && r.dispatched)).toBe(true);
    await c.call('ask.revoke', {});
    await expect(c.call('ask.turn', { question: 'delta' })).rejects.toMatchObject({ code: 'permission_denied' });
  });
});

describe('ask model', () => {
  const root = (id: string, state: Root['scan']['state'] = 'complete'): Root => ({
    root_id: id, display_path: { text: id, truncated: false }, origin: 'fixture', dataset_class: 'synthetic', volume_id: null, granted_at: '2026-10-17T00:00:00Z',
    granted_via: 'fixture', grant_state: 'active', scan: { state, last_job_id: null, generation: null, finished_at: null, coverage: state === 'complete' ? 'complete' : 'partial' }, totals: null,
  });

  it('states what Ask sees, what it does not, and what is unknown', () => {
    const grant = { roots: ['rt_a', 'rt_b'], fields: [], granted_at: '2026-10-17T00:00:00Z' };
    const s = scopeOf([root('rt_a'), root('rt_b', 'never_scanned'), root('rt_c')], grant);
    expect([s.visible.length, s.hidden.length, s.unknown.length]).toEqual([2, 1, 1]);
    expect(scopeSentence(s)).toContain('1 other folder is not visible to Ask');
    expect(scopeSentence(s)).toContain('unknown, not empty');
    expect(scopeSentence(scopeOf([root('rt_a')], null))).toBe('Ask can see no folder yet, so it cannot look anything up.');
  });

  it('splits handles into citations and unverified references, never trusting the text', () => {
    const t = { answer: 'h1 is it, not h2 or ah3.', citations: [{ handle: 'h1', node_id: 'nd_1', resolved: true, name: 'a.txt', root_id: 'rt_a' }] } as unknown as AskTurnRecord;
    const p = piecesOf(t);
    expect(p).toEqual([
      { handle: 'h1', citation: t.citations[0] }, { text: ' is it, not ' }, { handle: 'h2', citation: null }, { text: ' or ah3.' },
    ]);
  });

  it('marks sentences without a ledger-backed citation unsupported, including invented handles', () => {
    const t = { answer: 'h1 is report.pdf. secret.txt exists! h99 is a guess?\nNo citation here', citations: [{ handle: 'h1', node_id: 'nd_1', resolved: false, name: null, root_id: null }] } as unknown as AskTurnRecord;
    expect(askModel).toHaveProperty('sentencesOf');
    const sentences = askModel.sentencesOf(t);
    expect(sentences.map((s) => s.unsupported)).toEqual([false, true, true, true]);
    expect(sentences.flatMap((s) => s.pieces).filter((p) => 'text' in p).map((p) => p.text).join('')).toBe(' is report.pdf. secret.txt exists!  is a guess?\nNo citation here');
    expect(sentences[0]!.pieces[0]).toEqual({ handle: 'h1', citation: t.citations[0] });
    expect(sentences[2]!.pieces.find((p) => 'handle' in p)).toEqual({ handle: 'h99', citation: null });
    expect(askModel.sentencesOf({ ...t, answer: '<img src="https://invalid.test">' })[0]!.unsupported).toBe(true);
    for (const answer of ['"h1 exists." secret.txt exists.', 'h1 exists。secret.txt exists。']) {
      expect(askModel.sentencesOf({ ...t, answer }).map((s) => s.unsupported)).toEqual([false, true]);
    }
  });

  it('says unreachable only after a turn tried, and not configured without a profile', () => {
    const base = { provider: 'local', profile: { base_url: 'http://127.0.0.1:1/v1', model: 'm' } } as unknown as AskStatus;
    expect(providerOf(base, null).label).toBe('Local · m');
    const failed = { failure: { kind: 'unreachable', message: 'x', dispatched: false } } as unknown as AskTurnRecord;
    expect(providerOf(base, failed).tone).toBe('bad');
    expect(providerOf({ ...base, provider: 'not_configured', profile: null }, null).label).toBe('Not configured');
  });
});

// Run the actual View load/read functions without a DOM dependency; resync performs the effect's cleanup.
describe('ask view resync', () => {
  it.each(['tree.invalidated', 'roots.changed', 'stream.lagged'])('resumes the current running turn after %s', async () => {
    vi.useFakeTimers();
    try {
      const source = readFileSync(new URL('../../src/views/ask/View.svelte', import.meta.url), 'utf8');
      const functions = source.slice(source.indexOf('async function load()'), source.indexOf('async function ask()'));
      const js = ts.transpileModule(functions, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None } }).outputText;
      let reads = 0;
      const session = { client: { call: vi.fn(async (command: string) => ({ result: command === 'ask.status' ? { last_turn: 'at_1' } : command === 'roots.list' ? { roots: [] } : { turn_id: 'at_1', state: ++reads >= 3 ? 'completed' : 'running' } })) }, handle: String };
      const view = new Function('session', `
        let generation = 0, pollTimer, turn = null, status, roots, loading, error, notice;
        const POLL_MS = 400;
        ${js}
        return { load, state: () => turn?.state, resync: () => { generation++; clearTimeout(pollTimer); } };
      `)(session) as { load(): Promise<void>; state(): string; resync(): void };
      await view.load();
      expect(view.state()).toBe('running');
      view.resync();
      await view.load();
      await vi.advanceTimersByTimeAsync(400);
      expect(view.state()).toBe('completed');
      expect(reads).toBe(3);
      await view.load();
      expect(reads).toBe(3); // A settled, unchanged turn needs no polling.
    } finally { vi.clearAllTimers(); vi.useRealTimers(); }
  });
});
