import { describe, expect, it, vi } from 'vitest';
import { Client, LoomwardError } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { CLIENT_FIELDS, type ClientGrantRequest } from '../../src/lib/contracts.gen';
import { accessState, fieldOptions, createPayload, refreshAccess, grantAccess, revokeAccess } from '../../src/views/agent-access/model';

describe('Agent access model', () => {
  it('derives field options from the generated list and running service availability', () => {
    expect(fieldOptions([...CLIENT_FIELDS]).map(f => f.field)).toEqual([...CLIENT_FIELDS]);
    expect(fieldOptions(['size']).map(f => f.field)).toEqual(['size']);
    expect(fieldOptions([...CLIENT_FIELDS]).every(f => f.explanation.length > 0)).toBe(true);
  });

  it('builds an MCP payload, optional expiry and no teacher grant', () => {
    expect(createPayload({ name: 'synthetic.agent_1', roots: ['rt_1'], fields: ['size'], expiry: '' }))
      .toEqual({ client: 'mcp:synthetic.agent_1', roots: ['rt_1'], fields: ['size'] });
    expect(createPayload({ name: 'synthetic', roots: ['rt_1'], fields: [], expiry: '2099-01-01T00:00:00Z' }).expires_at)
      .toBe('2099-01-01T00:00:00.000Z');
    for (const name of ['', 'bad name', 'ask:local', 'a'.repeat(65)]) {
      expect(() => createPayload({ name, roots: ['rt_1'], fields: [], expiry: '' })).toThrow();
    }
  });

  it('requires a pending confirmation, cancels without calling, then revokes and reloads', async () => {
    const c = new Client(createMockTransport());
    const call = vi.spyOn(c, 'call');
    const s = accessState();
    await refreshAccess(s, c, String);
    await revokeAccess(s, c, String);
    expect(call.mock.calls.some(([command]) => command === 'clients.revoke')).toBe(false);
    s.pendingRevoke = 'mcp:synthetic-reader';
    s.pendingRevoke = null;
    await revokeAccess(s, c, String);
    expect(call.mock.calls.some(([command]) => command === 'clients.revoke')).toBe(false);
    s.pendingRevoke = 'mcp:synthetic-reader';
    await revokeAccess(s, c, String);
    expect(s.pendingRevoke).toBeNull();
    expect(s.list!.grants.find(g => g.client === 'mcp:synthetic-reader')!.revoked_at).not.toBeNull();
  });

  it('shows the exact service error and keeps unknown distinct from an empty list', async () => {
    const c = new Client(createMockTransport());
    const s = accessState();
    await refreshAccess(s, c, e => (e as Error).message);
    vi.spyOn(c, 'call').mockRejectedValue(new Error('service says synthetic refusal'));
    await refreshAccess(s, c, e => (e as Error).message);
    expect(s.error).toBe('service says synthetic refusal');
    expect(s.list).toBeNull();
    expect(s.roots).toBeNull();
    await grantAccess(s, c, { name: 'synthetic', roots: ['rt_1'], fields: ['path'], expiry: '' }, e => (e as Error).message);
    expect(s.error).toBe('service says synthetic refusal');
  });

  it('invalidates a known empty list when a mutation outcome is unknown', async () => {
    const c = new Client(createMockTransport());
    const s = accessState();
    const listed = (await c.call('clients.list', {})).result;
    s.list = { grants: [], available_fields: [...CLIENT_FIELDS] };
    s.roots = [];
    vi.spyOn(c, 'call').mockRejectedValue(new LoomwardError({
      code: 'deadline_exceeded', message: 'synthetic outcome unknown', retryable: false, detail: null,
    }));
    await grantAccess(s, c, { name: 'synthetic', roots: ['rt_1'], fields: [], expiry: '' }, e => (e as Error).message);
    expect(s.error).toBe('synthetic outcome unknown');
    expect(s.list).toBeNull();
    expect(s.roots).toBeNull();
    s.list = listed;
    s.pendingRevoke = listed.grants[0]!.client;
    await revokeAccess(s, c, e => (e as Error).message);
    expect(s.error).toBe('synthetic outcome unknown');
    expect(s.list).toBeNull();
  });
});

describe('Agent access contract fixture', () => {
  it('lists, creates, replaces and revokes an MCP grant without changing Ask', async () => {
    const c = new Client(createMockTransport());
    const before = (await c.call('clients.list', {})).result;
    expect(before.grants[0]?.client).toBe('mcp:synthetic-reader');
    const roots = (await c.call('roots.list', {})).result.roots;
    const payload: ClientGrantRequest = { client: 'mcp:synthetic-new', roots: [roots[0]!.root_id], fields: ['size'] };
    const first = (await c.call('clients.grant', payload)).result;
    const next = (await c.call('clients.grant', payload)).result;
    expect(next.grant_id).not.toBe(first.grant_id);
    expect((await c.call('clients.revoke', { client: payload.client })).result.revoked).toBe(true);
    const history = (await c.call('clients.list', {})).result.grants.filter(g => g.client === payload.client);
    expect(history).toHaveLength(2);
    expect(history.every(g => g.revoked_at !== null)).toBe(true);
  });
});
