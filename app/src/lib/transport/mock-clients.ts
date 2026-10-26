import { CLIENT_FIELDS, type ClientField, type ClientGrantList, type ClientGrantRecord, type RootList } from '../contracts.gen';
import fixture from '../../../../contracts/v3/examples/commands/clients.list.result.json';
import type { Fail } from './mock-views';

export function createClientMock(now: () => Date, fail: Fail, roots: () => RootList) {
  const grants = structuredClone(fixture.grants) as ClientGrantRecord[];
  let seq = 2;
  const clientName = (p: Record<string, unknown>): string => {
    if (typeof p.client !== 'string' || !/^mcp:[A-Za-z0-9._-]{1,64}$/.test(p.client)) fail('invalid_request', 'invalid client grant');
    return p.client as string;
  };
  const revoke = (client: string): boolean => {
    let changed = false;
    for (const grant of grants) if (grant.client === client && !grant.revoked_at) {
      grant.revoked_at = now().toISOString(); changed = true;
    }
    return changed;
  };
  return {
    'clients.list': (): ClientGrantList => structuredClone({ grants, available_fields: [...CLIENT_FIELDS] }),
    'clients.grant': (p: Record<string, unknown>) => {
      const client = clientName(p);
      const active = new Set(roots().roots.filter(r => r.grant_state === 'active').map(r => r.root_id));
      if (!Array.isArray(p.roots) || !p.roots.length || p.roots.length > 64 || new Set(p.roots).size !== p.roots.length) fail('invalid_request', 'invalid client grant');
      if ((p.roots as unknown[]).some(r => typeof r !== 'string' || !active.has(r))) fail('not_found', 'unknown root');
      if (!Array.isArray(p.fields) || p.fields.some(f => !CLIENT_FIELDS.includes(f as ClientField)) || new Set(p.fields).size !== p.fields.length) fail('invalid_request', 'invalid client grant');
      if (p.expires_at !== undefined && (typeof p.expires_at !== 'string' || !Number.isFinite(Date.parse(p.expires_at)))) fail('invalid_request', 'expiry timestamp out of range');
      revoke(client);
      const grant_id = `cg_${++seq}`;
      grants.push({ grant_id, client, roots: [...p.roots as string[]], fields: [...p.fields as ClientField[]], created_at: now().toISOString(), expires_at: p.expires_at as string ?? null, revoked_at: null });
      return { grant_id };
    },
    'clients.revoke': (p: Record<string, unknown>) => {
      const client = clientName(p);
      return { client, revoked: revoke(client) };
    },
  };
}
