import type { AskField, AskStatus, AskToolUse, AskTurnRecord, ErrorCode, RootList, SearchPage, SearchRequest } from '../contracts.gen';
import askStatusFixture from '../../../../contracts/v3/examples/commands/ask.status.result.json';

const FIELDS: AskField[] = ['path', 'size', 'modified', 'attributes', 'volumes'];
const STOP = new Set(['where', 'which', 'what', 'files', 'file', 'folder', 'folders', 'show', 'find', 'delete', 'move', 'the', 'and', 'please', 'guess', 'largest', 'biggest', 'are', 'my', 'any', 'can', 'you']);

/**
 * Ask on the mock transport: a scripted fake model (no network, no endpoint), synthetic only. Like HTTP it has no
 * `ask.configure` (profiles are made on the desktop), so it starts with a fixed synthetic loopback profile and no
 * grant. A turn runs the same shape as the engine: round 1 calls `find` (plus a refused `delete` when the question
 * asks for one), round 2 answers citing handles. Digests are SHA-256 of the round's request body, as recorded.
 */
/** The fake profile's digest input: base_url, NUL, model, as `ProviderProfile::digest` hashes it. */
export const MOCK_ASK_PROFILE_DIGEST_INPUT = 'http://127.0.0.1:11434/v1\u0000fake-synthetic-model';

export function createAskMock(
  now: () => Date,
  fail: (code: ErrorCode, message: string, detail?: Record<string, unknown> | null) => never,
  deps: { roots: () => RootList; search: (p: SearchRequest) => SearchPage },
) {
  const profile = { base_url: 'http://127.0.0.1:11434/v1', model: 'fake-synthetic-model' };
  let grant: AskStatus['grant'] = null;
  let turn: AskTurnRecord | null = null;
  let seq = 0;
  const digestHex = async (text: string) =>
    'sha256:' + [...new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text)))].map((b) => b.toString(16).padStart(2, '0')).join('');
  /** `ProviderProfile::digest` of the profile above: SHA-256 of base_url, NUL, model (pinned by a unit test). */
  const profileDigest = 'sha256:e3aa57344acd957faa922fd21ef9546e9bc6a982c51473574fd6fb4a535f525e';

  const active = () => new Set(deps.roots().roots.filter((r) => r.grant_state === 'active').map((r) => r.root_id));
  const status = (): AskStatus => {
    const live = active();
    return {
      ...structuredClone(askStatusFixture as unknown as AskStatus),
      profile: { ...profile }, profile_digest: profileDigest,
      grant: grant && { ...grant, roots: grant.roots.filter((r) => live.has(r)) },
      last_turn: turn?.turn_id ?? null,
    };
  };

  async function run(t: AskTurnRecord): Promise<void> {
    const q = t.question.toLowerCase();
    const word = (q.match(/[a-z0-9]{3,}/g) ?? []).filter((w) => !STOP.has(w)).sort((a, b) => b.length - a.length)[0] ?? '';
    const live = active();
    const roots = new Set((grant?.roots ?? []).filter((r) => live.has(r)));
    const hits = word ? deps.search({ root_id: null, text: word, extension: null, min_bytes: null, max_bytes: null, modified_after: null, modified_before: null, kind: 'any', limit: 20, cursor: null }).hits.filter((h) => roots.has(h.root_id)) : [];
    const handles = hits.map((h, i) => ({ handle: `h${i + 1}`, hit: h }));
    const tools: AskToolUse[] = [{ name: 'find', ok: true, rows: handles.length, detail: 'ok' }];
    if (/\b(delete|move|remove)\b/.test(q)) tools.push({ name: 'delete', ok: false, rows: null, detail: 'unknown tool' });
    const shown = handles.slice(0, 3);
    let answer = shown.length
      ? `${shown.map((h) => h.handle).join(', ')} match “${word}” in the folders I may see.`
      : `Nothing matching “${word || t.question}” in the folders I may see.`;
    if (tools.length > 1) answer += ` I cannot delete or move anything. You could review ${shown[0]?.handle ?? 'these files'} yourself in Explorer.`;
    if (/\bguess\b/.test(q)) answer += ' h99 might also be relevant.';
    const bodies = [JSON.stringify({ round: 1, question: t.question }), JSON.stringify({ round: 2, question: t.question, rows: handles.length })];
    const pd = profileDigest;
    const rounds = await Promise.all(bodies.map(async (b, i) => ({ round: i + 1, profile_digest: pd, principal: 'ask:local' as const, sha256: await digestHex(b), bytes: new TextEncoder().encode(b).length, dispatched: true, outcome: 'ok' })));
    const cited = [...new Set(answer.match(/\bh[1-9][0-9]{0,3}\b/g) ?? [])];
    if (turn?.turn_id !== t.turn_id) return;
    turn = {
      ...t, state: 'completed', finished_at: now().toISOString(), answer, rounds, tool_calls: tools, tool_calls_total: tools.length,
      citations: cited.flatMap((h) => { const c = handles.find((x) => x.handle === h); return c ? [{ handle: h, node_id: c.hit.row.node_id, resolved: true, name: c.hit.row.name, root_id: c.hit.root_id }] : []; }),
      unverified: cited.filter((h) => !handles.some((x) => x.handle === h)),
    };
  }

  return {
    'ask.status': (p: Record<string, unknown>) => {
      if (Object.keys(p).length) fail('invalid_request', 'ask.status takes no fields');
      return status();
    },
    'ask.grant': (p: Record<string, unknown>) => {
      const roots = p.roots, fields = p.fields;
      const live = active();
      if (!Array.isArray(roots) || roots.length < 1 || roots.length > 64 || new Set(roots).size !== roots.length) fail('invalid_request', 'roots: 1 to 64 unique root IDs');
      if (!Array.isArray(fields) || fields.some((f) => !FIELDS.includes(f as AskField)) || new Set(fields).size !== fields.length) fail('invalid_request', 'fields: unique AskField values');
      if ((roots as unknown[]).some((r) => !live.has(r as string))) fail('not_found', 'unknown root');
      grant = { roots: [...(roots as string[])], fields: [...(fields as AskField[])].sort(), granted_at: now().toISOString() };
      return status();
    },
    'ask.revoke': (p: Record<string, unknown>) => {
      if (Object.keys(p).length) fail('invalid_request', 'ask.revoke takes no fields');
      grant = null;
      return status();
    },
    'ask.turn': (p: Record<string, unknown>): AskTurnRecord => {
      const question = p.question;
      if (typeof question !== 'string' || question.length < 1 || [...question].length > 2000) fail('invalid_request', 'question: 1 to 2,000 characters');
      if (!grant || !grant.roots.some((r) => active().has(r))) fail('permission_denied', 'Ask may see no folder: grant it at least one folder first', { reason: 'ask_no_grant' });
      if (turn?.state === 'running') fail('busy', 'a question is still being answered');
      turn = {
        turn_id: `at_mock_${++seq}`, state: 'running', question: question as string, started_at: now().toISOString(), finished_at: null,
        answer: null, answer_truncated: false, failure: null, citations: [], unverified: [], tool_calls: [], tool_calls_total: 0, rounds: [],
      };
      const running = structuredClone(turn);
      setTimeout(() => void run(running), 150);
      return running;
    },
    'ask.result': (p: Record<string, unknown>) => {
      if (!turn || p.turn_id !== turn.turn_id) fail('not_found', 'unknown turn');
      return structuredClone(turn!);
    },
  };
}
