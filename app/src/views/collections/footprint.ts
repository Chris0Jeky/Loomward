import { LoomwardError, type Client } from '../../lib/transport/client';

export async function loadFootprint(client: Client, node: string, root: string, generation: string, signal: AbortSignal) {
  const options = { expectedGeneration: generation, signal };
  signal.throwIfAborted();
  const [detail, families, children] = await Promise.all([
    client.call('node.inspect', { node_id: node }, options),
    client.call('stats.breakdown', { node_id: node, by: 'ext_family', basis: 'logical', limit: 8 }, options),
    client.call('tree.children', { node_id: node, sort: 'size_desc', basis: 'logical', limit: 12, cursor: null }, options),
  ]);
  signal.throwIfAborted();
  const revisions = [detail.meta.catalog_rev, families.meta.catalog_rev, children.meta.catalog_rev];
  if (detail.result.row.node_id !== node || detail.result.row.kind !== 'dir' || detail.result.root_id !== root
    || detail.result.identity.observed_generation !== generation
    || families.result.node_id !== node || families.result.generation !== generation
    || families.result.by !== 'ext_family' || families.result.basis !== 'logical'
    || children.result.anchor !== node || children.result.generation !== generation
    || revisions.some(rev => rev === null || rev !== revisions[0])) {
    throw new LoomwardError({ code: 'stale_generation', message: 'Project observations changed. Find projects again.', retryable: true, detail: null });
  }
  return { detail: detail.result, families: families.result, children: children.result };
}

export type Footprint = Awaited<ReturnType<typeof loadFootprint>>;
