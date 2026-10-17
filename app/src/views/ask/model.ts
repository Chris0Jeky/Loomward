import type { AskCitation, AskStatus, AskTurnRecord, Root } from '../../lib/contracts.gen';

const SCAN_WORDS: Record<Root['scan']['state'], string> = {
  never_scanned: 'never scanned', scanning: 'being scanned', complete: 'complete', partial: 'partly scanned', stale: 'out of date', failed: 'scan failed',
};

export interface Scope {
  /** Active roots the grant names, in roots.list order. */
  visible: Root[];
  /** Active roots Ask may not see. */
  hidden: Root[];
  /** Visible roots whose scan is not complete: their contents are unknown, not empty. */
  unknown: { root: Root; why: string }[];
}

/** What Ask can see, computed from the owner's roots and the grant; nothing outside it is searched or counted. */
export function scopeOf(roots: Root[], grant: AskStatus['grant']): Scope {
  const active = roots.filter((r) => r.grant_state === 'active');
  const granted = new Set(grant?.roots ?? []);
  const visible = active.filter((r) => granted.has(r.root_id));
  return {
    visible,
    hidden: active.filter((r) => !granted.has(r.root_id)),
    unknown: visible.filter((r) => r.scan.state !== 'complete' || r.scan.coverage !== 'complete').map((root) => ({ root, why: SCAN_WORDS[root.scan.state] })),
  };
}

/** One sentence for people (and screen readers): what Ask sees, what it does not, and what is unknown. */
export function scopeSentence(s: Scope): string {
  if (!s.visible.length) return 'Ask can see no folder yet, so it cannot look anything up.';
  const n = (k: number, one: string, many: string) => `${k} ${k === 1 ? one : many}`;
  let out = `Ask can see ${n(s.visible.length, 'folder', 'folders')}. `;
  out += s.hidden.length ? `${n(s.hidden.length, 'other folder is', 'other folders are')} not visible to Ask and never searched. ` : 'No other folder is granted to Loomward. ';
  out += s.unknown.length
    ? `${n(s.unknown.length, 'visible folder is', 'visible folders are')} not fully scanned: what is there is unknown, not empty. `
    : 'Every visible folder is fully scanned. ';
  return out + 'Anything outside these folders, and anything not yet scanned, cannot appear in an answer.';
}

export type Provider = { tone: 'ok' | 'unk' | 'bad'; label: string; detail: string };

/** The provider badge. Unreachable is reported only from a turn that actually tried; the status read never contacts it. */
export function providerOf(status: AskStatus | null, last: AskTurnRecord | null): Provider {
  if (!status) return { tone: 'unk', label: 'Provider unknown', detail: 'The Ask status could not be read.' };
  if (status.provider === 'not_configured' || !status.profile) {
    return { tone: 'unk', label: 'Not configured', detail: 'No model is set up, so nothing is ever sent anywhere.' };
  }
  const where = `${status.profile.model} at ${status.profile.base_url}`;
  if (last?.failure && (last.failure.kind === 'unreachable' || last.failure.kind === 'timeout')) {
    return { tone: 'bad', label: `Local · ${status.profile.model} · unreachable`, detail: `The last question could not reach ${where}.` };
  }
  return { tone: 'ok', label: `Local · ${status.profile.model}`, detail: `A model on this machine (${where}): loopback only.` };
}

export type Piece = { text: string } | { handle: string; citation: AskCitation | null };

/** The answer as plain text with its handles split out: a handle the ledger holds becomes a citation, any other one an unverified reference. */
export function piecesOf(t: AskTurnRecord): Piece[] {
  const answer = t.answer ?? '';
  const out: Piece[] = [];
  let at = 0;
  for (const m of answer.matchAll(/(?<![A-Za-z0-9_])h[1-9][0-9]{0,3}(?![A-Za-z0-9_])/g)) {
    if (m.index > at) out.push({ text: answer.slice(at, m.index) });
    out.push({ handle: m[0], citation: t.citations.find((c) => c.handle === m[0]) ?? null });
    at = m.index + m[0].length;
  }
  if (at < answer.length) out.push({ text: answer.slice(at) });
  return out;
}

/** A citation grounds only its own sentence; even an invented handle supplies no evidence. */
export function sentencesOf(t: AskTurnRecord): { pieces: Piece[]; unsupported: boolean }[] {
  // shortcut: sentences follow punctuation and line breaks, upgrade for language-aware boundaries.
  return (t.answer ?? '').split(/(?<=[.!?]["'”’]?)(?=\s)|(?<=[。！？])|(?=\n)/u).filter(Boolean).map((answer) => {
    const pieces = piecesOf({ ...t, answer });
    return { pieces, unsupported: !!answer.trim() && !pieces.some((p) => 'citation' in p && p.citation !== null) };
  });
}

/** A digest shortened for display; the full value is in the title and the copy. */
export const shortDigest = (d: string) => d.replace(/^sha256:/, '').slice(0, 12);

export const FAILURE_WORDS: Record<string, string> = {
  unreachable: 'Nothing answered at the model endpoint. Is the local model server running?',
  timeout: 'The model took too long; the outcome of that round is unknown.',
  http_status: 'The model server refused the request.',
  invalid_reply: 'The model returned no usable answer.',
  oversize: 'The model reply was too large.',
  broker_failed: 'The broker process that talks to the model could not run.',
  not_local: 'The endpoint is not a loopback address, so nothing was sent.',
  invalid_request: 'The conversation was too large to send.',
  consent_required: 'Cloud models need consent and are not available.',
  cloud_unavailable: 'Cloud models are not available.',
};
