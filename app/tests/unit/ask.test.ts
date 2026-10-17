import { createHash } from 'node:crypto';
import { describe, expect, it } from 'vitest';
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

  it('says unreachable only after a turn tried, and not configured without a profile', () => {
    const base = { provider: 'local', profile: { base_url: 'http://127.0.0.1:1/v1', model: 'm' } } as unknown as AskStatus;
    expect(providerOf(base, null).label).toBe('Local · m');
    const failed = { failure: { kind: 'unreachable', message: 'x', dispatched: false } } as unknown as AskTurnRecord;
    expect(providerOf(base, failed).tone).toBe('bad');
    expect(providerOf({ ...base, provider: 'not_configured', profile: null }, null).label).toBe('Not configured');
  });
});
