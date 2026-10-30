import { describe, expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';

async function setup() {
  const client = new Client(createMockTransport());
  const roots = (await client.call('roots.list', {})).result.roots;
  const root = roots[0]!;
  const slice = (await client.call('tree.slice', { anchor: { kind: 'root', root_id: root.root_id }, depth: 1, max_nodes: 16, min_share: 0, basis: 'logical', include_files: false })).result;
  return { client, roots, root, top: slice.anchor_node_id };
}

/** Breadth-first through tree.children for the first row matching `wanted`. */
async function find(client: Client, start: string, wanted: (row: { kind: string; coverage: string }) => boolean) {
  const queue = [start];
  for (let i = 0; i < queue.length && i < 300; i++) {
    const page = (await client.call('tree.children', { node_id: queue[i]!, sort: 'name_asc', basis: 'logical', limit: 200, cursor: null })).result;
    for (const row of page.items) {
      if (wanted(row)) return row.node_id;
      if (row.kind === 'dir') queue.push(row.node_id);
    }
  }
  throw new Error('no such node in the mock tree');
}

describe('insight.folder on the mock transport', () => {
  it('returns the three axes and fact-only lines, each with the packet fields it rests on', async () => {
    const { client, root, top } = await setup();
    const dir = await find(client, top, (r) => r.kind === 'dir' && r.coverage === 'complete');
    const r = (await client.call('insight.folder', { root_id: root.root_id, node_id: dir })).result;
    expect(r.root_id).toBe(root.root_id);
    expect(r.node_id).toBe(dir);
    expect(r.axes.map((a) => a.axis)).toEqual(['meaning', 'lifecycle', 'recreatability']);
    for (const a of r.axes) {
      expect(a.outcome === 'label' ? a.label !== null && a.abstain_reason === null : a.label === null && a.abstain_reason !== null).toBe(true);
    }
    expect(r.lines.length).toBeGreaterThan(0);
    for (const line of r.lines) expect(line.trace.every((p) => p.startsWith('/'))).toBe(true);
    const text = r.lines.map((l) => l.text).join('\n');
    expect(text).toContain('No activity evidence seen since');
    expect(text.toLowerCase()).not.toMatch(/unused|probab|safe to|approve/);
  });

  it('abstains on coverage for a folder that was not observed, and says nothing else', async () => {
    const { client, root, top } = await setup();
    const denied = await find(client, top, (r) => r.kind === 'dir' && r.coverage === 'denied');
    const r = (await client.call('insight.folder', { root_id: root.root_id, node_id: denied })).result;
    expect(r.coverage).toBe('denied');
    expect(r.partial).toBe(true);
    expect(r.axes.every((a) => a.outcome === 'abstain' && a.abstain_reason === 'coverage' && a.label === null && a.votes.length === 0)).toBe(true);
    expect(r.lines).toHaveLength(1);
  });

  it('refuses unknown roots and nodes, a node outside the root, a file and malformed requests', async () => {
    const { client, roots, root, top } = await setup();
    const dir = await find(client, top, (r) => r.kind === 'dir' && r.coverage === 'complete');
    const file = await find(client, top, (r) => r.kind === 'file');
    const ask = (payload: Record<string, unknown>) => client.call('insight.folder', payload as never);
    await expect(ask({ root_id: 'rt_nope', node_id: dir })).rejects.toMatchObject({ code: 'not_found' });
    await expect(ask({ root_id: root.root_id, node_id: 'nd_nope' })).rejects.toMatchObject({ code: 'not_found' });
    await expect(ask({ root_id: roots[1]!.root_id, node_id: dir })).rejects.toMatchObject({ code: 'not_found' });
    await expect(ask({ root_id: root.root_id, node_id: file })).rejects.toMatchObject({ code: 'invalid_request' });
    await expect(ask({ root_id: root.root_id })).rejects.toMatchObject({ code: 'invalid_request' });
    await expect(ask({ root_id: root.root_id, node_id: dir, limit: 5 })).rejects.toMatchObject({ code: 'invalid_request' });
    await client.call('roots.revoke', { root_id: root.root_id, purge_catalog: false });
    await expect(ask({ root_id: root.root_id, node_id: dir })).rejects.toMatchObject({ code: 'not_found' });
  });
});
