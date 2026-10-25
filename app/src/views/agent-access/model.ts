import { CLIENT_FIELDS, type ClientField, type ClientGrantList, type ClientGrantRequest, type Root } from '../../lib/contracts.gen';
import type { Client } from '../../lib/transport/client';

const EXPLANATIONS: Partial<Record<ClientField, string>> = {
  path: 'Names and paths', size: 'Sizes and the space tools', modified: 'Modified times and ages',
  attributes: 'File attributes', volumes: 'Drive labels, capacity and free space (all drives)',
};

export function fieldOptions(available: readonly ClientField[]) {
  return CLIENT_FIELDS.filter(field => available.includes(field))
    .map(field => ({ field, explanation: EXPLANATIONS[field] ?? field }));
}

export interface GrantDraft { name: string; roots: string[]; fields: ClientField[]; expiry: string }

export function createPayload(draft: GrantDraft): ClientGrantRequest {
  if (!/^[A-Za-z0-9._-]{1,64}$/.test(draft.name)) throw new Error('Use 1–64 letters, digits, dots, underscores or hyphens for the client name.');
  if (!draft.roots.length) throw new Error('Choose at least one folder.');
  return {
    client: `mcp:${draft.name}`, roots: [...draft.roots], fields: [...draft.fields],
    ...(draft.expiry ? { expires_at: new Date(draft.expiry).toISOString() } : {}),
  };
}

export interface AccessState {
  list: ClientGrantList | null;
  roots: Root[] | null;
  busy: boolean;
  error: string;
  notice: string;
  pendingRevoke: string | null;
}
export const accessState = (): AccessState => ({ list: null, roots: null, busy: false, error: '', notice: '', pendingRevoke: null });
type ShowError = (e: unknown) => string;

async function load(s: AccessState, c: Client): Promise<void> {
  const [list, roots] = await Promise.all([c.call('clients.list', {}), c.call('roots.list', {})]);
  s.list = list.result;
  s.roots = roots.result.roots;
}

export async function refreshAccess(s: AccessState, c: Client, showError: ShowError): Promise<void> {
  s.busy = true; s.error = ''; s.list = null; s.roots = null; s.pendingRevoke = null;
  try { await load(s, c); }
  catch (e) { s.error = showError(e); }
  finally { s.busy = false; }
}

export async function grantAccess(s: AccessState, c: Client, draft: GrantDraft, showError: ShowError): Promise<void> {
  s.busy = true; s.error = ''; s.notice = '';
  try {
    await c.call('clients.grant', createPayload(draft));
    s.list = null; s.roots = null;
    s.notice = 'Grant saved. Any earlier grant for this client was replaced.';
    await load(s, c);
  } catch (e) { s.list = null; s.roots = null; s.error = showError(e); }
  finally { s.busy = false; }
}

export async function revokeAccess(s: AccessState, c: Client, showError: ShowError): Promise<void> {
  if (!s.pendingRevoke) return;
  s.busy = true; s.error = ''; s.notice = '';
  try {
    await c.call('clients.revoke', { client: s.pendingRevoke });
    s.pendingRevoke = null; s.list = null; s.roots = null;
    s.notice = 'Access revoked. The agent is refused from its next call.';
    await load(s, c);
  } catch (e) { s.list = null; s.roots = null; s.error = showError(e); }
  finally { s.busy = false; }
}
