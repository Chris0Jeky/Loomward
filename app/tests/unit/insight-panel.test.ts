import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import FolderInsightPanel from '../../src/lib/ui/FolderInsight.svelte';
import { INSIGHT_FOOTER, axisRows, insightFailure } from '../../src/lib/format/insight';
import { FolderInsight } from '../../src/lib/stores/insight.svelte';
import { LoomwardError, type Client } from '../../src/lib/transport/client';
import type { ErrorCode, InsightAxis, InsightFolder } from '../../src/lib/contracts.gen';
import example from '../../../contracts/v3/examples/commands/insight.folder.result.json';

const folder = example as unknown as InsightFolder;
const withAxes = (axes: InsightAxis[]): InsightFolder => ({ ...folder, axes });
const abstain = (axis: InsightAxis['axis'], abstain_reason: InsightAxis['abstain_reason'], votes: InsightAxis['votes'] = []): InsightAxis =>
  ({ axis, outcome: 'abstain', label: null, abstain_reason, votes });
/** Words that would turn a description into a recommendation or an approval. */
const FORBIDDEN = /approv|recommend|safe|should|delete|remove|clean|unused|probab|confiden|certain|\bok\b/i;

describe('axis rows', () => {
  it('words a label as an inference, never as a fact, and names the agreeing rule', () => {
    const rows = axisRows(folder);
    expect(rows.map((r) => r.name)).toEqual(['Meaning', 'Lifecycle', 'Recreatability']);
    expect(rows[0]).toMatchObject({ text: 'source project (rule inference, not a recorded fact)', rules: ['lf_source_manifest'], decided: true });
    expect(rows[1]!.text).toBe('historical (rule inference, not a recorded fact)');
    expect(rows[2]!.text).toBe('mixed (rule inference, not a recorded fact)');
  });

  it('keeps every abstention visible with its own reason', () => {
    const rows = axisRows(withAxes([
      abstain('meaning', 'no_vote'),
      abstain('lifecycle', 'conflict', [{ function: 'lf_a', label: 'active' }, { function: 'lf_b', label: 'historical' }, { function: 'lf_c', label: 'active' }]),
      abstain('recreatability', 'coverage'),
    ]));
    expect(rows.map((r) => r.text)).toEqual([
      'not decided — no rule had enough evidence',
      'not decided — rules disagree (active, historical)',
      'not decided — coverage is not complete, so no rule ran',
    ]);
    expect(rows.every((r) => !r.decided && r.rules.length === 0)).toBe(true);
  });

  it('never uses recommendation or approval words in any row, label or failure message', () => {
    const texts = [
      ...axisRows(folder).map((r) => r.text),
      ...axisRows(withAxes([abstain('meaning', 'no_vote'), abstain('lifecycle', 'conflict'), abstain('recreatability', 'coverage')])).map((r) => r.text),
      ...(['permission_denied', 'not_found', 'invalid_request', 'resource_budget', 'deadline_exceeded'] as ErrorCode[]).map((c) => insightFailure(c, '')),
    ];
    for (const t of texts) expect(t).not.toMatch(FORBIDDEN);
    expect(INSIGHT_FOOTER).toBe('Inferred from folder facts; not a recommendation and not an approval to change anything.');
  });

  it('keeps an unfamiliar axis visible under a readable name instead of dropping it', () => {
    const rows = axisRows(withAxes([abstain('access_pattern' as InsightAxis['axis'], 'no_vote')]));
    expect(rows).toHaveLength(1);
    expect(rows[0]!.name).toBe('Access pattern');
  });
});

describe('failure wording', () => {
  it('says the folder is too large for the budget and that nothing was guessed', () => {
    expect(insightFailure('resource_budget', 'x')).toBe('This folder is too large to describe within the budget. Nothing was guessed.');
  });
  it('refusals say nothing about which check failed', () => {
    expect(insightFailure('permission_denied', 'x')).toContain('not available');
    expect(insightFailure('not_found', 'x')).toContain('Nothing was described');
  });
  it('keeps the engine message for other codes and never goes blank', () => {
    expect(insightFailure('internal_error', 'engine said so')).toBe('engine said so');
    expect(insightFailure(null, '')).toBe('Describing failed.');
  });
});

type Reply = { result: InsightFolder };
function pendingClient() {
  const pending: { params: { root_id: string; node_id: string }; signal: AbortSignal; reply: (v: Reply) => void; fail: (e: Error) => void }[] = [];
  const client = {
    call: (_: string, params: { root_id: string; node_id: string }, o: { signal: AbortSignal }) =>
      new Promise<Reply>((reply, fail) => pending.push({ params, signal: o.signal, reply, fail })),
  } as unknown as Client;
  return { client, pending };
}
/** The engine's answer about one folder of root rt_a. */
const about = (node: string): Reply => ({ result: { ...folder, root_id: 'rt_a', node_id: node } });
const refusal = (code: ErrorCode) => new LoomwardError({ code, message: 'engine wording', retryable: false, detail: null });

describe('FolderInsight request', () => {
  it('asks only when told to, and shows the reply exactly as returned', async () => {
    const { client, pending } = pendingClient();
    const ins = new FolderInsight();
    expect(pending).toHaveLength(0);
    const run = ins.describe(client, 'rt_a', 'nd_a');
    expect(ins.busy).toBe(true);
    expect(pending[0]!.params).toEqual({ root_id: 'rt_a', node_id: 'nd_a' });
    pending[0]!.reply(about('nd_a'));
    await run;
    expect(ins.busy).toBe(false);
    expect(ins.result!.lines.map((l) => l.text)).toEqual(folder.lines.map((l) => l.text));
  });

  it('drops an older reply once a newer request or a new selection has superseded it', async () => {
    const { client, pending } = pendingClient();
    const ins = new FolderInsight();
    const old = ins.describe(client, 'rt_a', 'nd_old');
    const fresh = ins.describe(client, 'rt_a', 'nd_new');
    expect(pending[0]!.signal.aborted).toBe(true);
    pending[1]!.reply(about('nd_new'));
    await fresh;
    pending[0]!.reply(about('nd_old'));
    await old;
    expect(ins.result!.node_id).toBe('nd_new');
    expect(ins.busy).toBe(false);
    ins.clear();
    expect(ins.result).toBeNull();
  });

  it('does not let a failure of an older request overwrite the newer answer, nor a reply after clear()', async () => {
    const { client, pending } = pendingClient();
    const ins = new FolderInsight();
    const old = ins.describe(client, 'rt_a', 'nd_old');
    const fresh = ins.describe(client, 'rt_a', 'nd_new');
    pending[1]!.reply(about('nd_new'));
    await fresh;
    pending[0]!.fail(refusal('resource_budget'));
    await old;
    expect(ins.message).toBe('');
    expect(ins.result!.node_id).toBe('nd_new');
    const third = ins.describe(client, 'rt_a', 'nd_third');
    ins.clear();
    pending[2]!.reply(about('nd_third'));
    await third;
    expect(ins.result).toBeNull();
    expect(ins.busy).toBe(false);
  });

  it.each([
    ['resource_budget', 'This folder is too large to describe within the budget. Nothing was guessed.'],
    ['permission_denied', 'Describing a folder is not available to this session.'],
  ] as const)('shows %s as words and no result', async (code, text) => {
    const { client, pending } = pendingClient();
    const ins = new FolderInsight();
    const run = ins.describe(client, 'rt_a', 'nd_a');
    pending[0]!.fail(refusal(code));
    await run;
    expect(ins.message).toBe(text);
    expect(ins.result).toBeNull();
    expect(ins.busy).toBe(false);
  });

  it('hands a transport failure to the session handler', async () => {
    const { client, pending } = pendingClient();
    const ins = new FolderInsight();
    const run = ins.describe(client, 'rt_a', 'nd_a', () => 'connection lost');
    pending[0]!.fail(new Error('boom'));
    await run;
    expect(ins.message).toBe('connection lost');
  });

  it('drops a reply about another folder or root instead of showing it', async () => {
    const { client, pending } = pendingClient();
    const ins = new FolderInsight();
    const run = ins.describe(client, 'rt_a', 'nd_a');
    pending[0]!.reply(about('nd_other'));
    await run;
    expect(ins.result).toBeNull();
    expect(ins.message).toContain('different folder');
    const again = ins.describe(client, 'rt_a', 'nd_a');
    pending[1]!.reply({ result: { ...folder, root_id: 'rt_b', node_id: 'nd_a' } });
    await again;
    expect(ins.result).toBeNull();
    expect(ins.busy).toBe(false);
  });
});

describe('FolderInsight panel', () => {
  // server rendering adds hydration comments between nodes; the markup under test is what is left
  const show = (props: Record<string, unknown>) => render(FolderInsightPanel, { props: { rootId: 'rt_a', nodeId: 'nd_a', kind: 'dir', ...props } }).body.replace(/<!--.*?-->/g, '');
  const seeded = (set: (m: FolderInsight) => void) => { const m = new FolderInsight(); set(m); return m; };
  const decode = (html: string) => html.replaceAll('&quot;', '"').replaceAll('&#39;', "'").replaceAll('&amp;', '&');

  it('renders the button and the footer, and asks for nothing before the button is pressed', () => {
    const body = show({});
    expect(body).toContain('About this folder');
    expect(body).toMatch(/<button[^>]*>Describe<\/button>/);
    expect(body).toContain(INSIGHT_FOOTER);
    expect(body).not.toContain('Meaning');
  });

  it('shows the section for a folder only', () => {
    for (const kind of ['file', 'root', 'other', '']) expect(show({ kind })).not.toContain('About this folder');
    expect(show({ kind: 'dir' })).toContain('About this folder');
  });

  it('keeps the button focusable while busy: aria-disabled, never disabled', () => {
    const body = show({ model: seeded((m) => { m.busy = true; }) });
    const button = body.match(/<button[^>]*>Describe<\/button>/)![0];
    expect(button).toContain('aria-disabled="true"');
    expect(button).not.toMatch(/\sdisabled/);
    expect(body).toContain('Describing…');
  });

  it('renders the result: three rows, the lines verbatim, the read time and the ready announcement', () => {
    const body = show({ model: seeded((m) => { m.result = folder; }) });
    expect(body).toContain('Description ready');
    const rows = [...body.matchAll(/<dt[^>]*>([^<]*)<\/dt>/g)].map((x) => x[1]);
    expect(rows).toEqual(['Meaning', 'Lifecycle', 'Recreatability']);
    expect(body).toContain('source project (rule inference, not a recorded fact)');
    expect(body).toContain('Rule: lf_source_manifest');
    const items = [...body.matchAll(/<li><bdi[^>]*>([\s\S]*?)<\/bdi><\/li>/g)].map((x) => decode(x[1]!));
    expect(items).toEqual(folder.lines.map((l) => l.text));
    expect(body).toContain('Read from the catalogue at 2026-10-30 09:00 UTC.');
    expect(body).not.toMatch(/<dd[^>]*class="[^"]*unknown/);
  });

  it('marks an abstaining row as unknown and shows a line with a hidden character as a badge', () => {
    const result = { ...withAxes([abstain('meaning', 'no_vote'), abstain('lifecycle', 'conflict'), abstain('recreatability', 'coverage')]),
      lines: [{ text: 'Folder name words: a‮b.', trace: ['/subject/name_tokens'] }] };
    const body = show({ model: seeded((m) => { m.result = result; }) });
    expect(body.match(/<dd[^>]*class="[^"]*unknown/g)).toHaveLength(3);
    expect(body).toContain('U+202E');
    expect(body).not.toContain('‮');
  });

  it('renders a failure as an alert and no result', () => {
    const body = show({ model: seeded((m) => { m.message = insightFailure('resource_budget', ''); }) });
    expect(body).toMatch(/<p[^>]*role="alert"[^>]*>This folder is too large to describe within the budget\. Nothing was guessed\.<\/p>/);
    expect(body).not.toContain('<dl');
    expect(body).not.toContain('Description ready');
  });
});
