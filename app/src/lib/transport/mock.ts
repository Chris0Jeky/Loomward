import type {
  Basis, DatasetClass, EntryPage, EntryRow, ErrorCode, EventEnvelope, GrantList, Health, RequestEnvelope,
  ResponseEnvelope, Root, RootList, SearchRequest, SessionInfo, SliceNode, TreeChildrenRequest, TreeSlice,
  TreeSliceRequest, Threads,
} from '../types';
import { createViewMock } from './mock-views';
import { generateTree, type SynthNode } from './synth';
import type { Transport } from './transport';

// TODO(L1): serve contracts/v3/examples/ for the commands this does not synthesise, once lane L1 lands them.

export interface MockOptions {
  seed?: number;
  /** Node count of the synthetic catalogue, at most 6,000. */
  count?: number;
  now?: () => Date;
}

const DATASET: DatasetClass = 'synthetic';
const GRANTED_AT = '2026-10-01T09:00:00Z';

class CommandFailure extends Error {
  constructor(readonly code: ErrorCode, message: string) {
    super(message);
  }
}
const fail = (code: ErrorCode, message: string): never => {
  throw new CommandFailure(code, message);
};

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const str = (v: unknown, what: string): string => (typeof v === 'string' ? v : fail('invalid_request', `${what} must be a string`));
const int = (v: unknown, what: string, min: number, max: number): number =>
  Number.isInteger(v) && (v as number) >= min && (v as number) <= max ? (v as number) : fail('invalid_request', `${what} must be an integer in ${min}..${max}`);

export function createMockTransport(opts: MockOptions = {}): Transport {
  const tree = generateTree(opts.count ?? 6000, opts.seed ?? 1);
  const now = opts.now ?? (() => new Date());
  const t0 = now().toISOString();
  const byId = new Map(tree.nodes.map((n) => [n.id, n]));
  const rootId = (n: SynthNode) => `rt_mock_${n.index}`;

  const sizeOf = (n: SynthNode, basis: Basis) => (basis === 'logical' ? n.logical : n.allocatedKnown);
  const nodeFor = (id: unknown) => byId.get(str(id, 'node_id')) ?? fail('not_found', 'no such node in this session');
  const pathOf = (n: SynthNode): string => (n.parent <= 0 ? n.name : `${pathOf(tree.nodes[n.parent]!)}/${n.name}`);

  const threadsOf = (n: SynthNode): Threads => ({
    meaning: { state: n.kind === 'file' && n.index % 7 === 0 ? 'unknown' : 'none', label: null, share: null, source: null, collection_ids: [] },
    residency: { volume_id: null, tier: null, tier_basis: 'unknown' },
    permission:
      n.coverage === 'denied' ? { state: 'denied', reason: 'access_denied' }
      : n.coverage === 'partial' ? { state: 'partial', reason: 'mixed_children' }
      : { state: 'granted', reason: 'metadata_grant_active' },
  });

  const sliceNode = (n: SynthNode, parent: number | null, depth: number, basis: Basis): SliceNode => ({
    node_id: n.id, parent, kind: n.kind, name: n.name, depth,
    size_bytes: sizeOf(n, basis).toString(), size_unknown_files: n.unknownFiles,
    logical_bytes: n.logical.toString(), allocated_bytes: n.allocated === null ? null : n.allocated.toString(),
    files: n.files, dirs: n.dirs, child_count: n.kind === 'file' ? 0 : n.coverage === 'denied' ? null : n.children.length,
    folded_count: null, coverage: n.coverage, live: false, ext_family: n.family, modified_at: n.modified, threads: threadsOf(n),
  });

  const entryRow = (n: SynthNode, hint: boolean): EntryRow => ({
    node_id: n.id, kind: n.kind === 'file' ? 'file' : 'dir', name: n.name, extension: n.extension, ext_family: n.family,
    logical_bytes: n.logical.toString(), allocated_bytes: n.allocated === null ? null : n.allocated.toString(),
    files: n.kind === 'file' ? null : n.files, dirs: n.kind === 'file' ? null : n.dirs, modified_at: n.modified,
    attributes: [], flags: n.coverage === 'denied' ? ['access_denied'] : [], coverage: n.coverage,
    location_hint: hint ? { text: pathOf(tree.nodes[n.parent]!), truncated: false } : null,
  });

  const cursorOf = (offset: number) => `c_${offset}`;
  const offsetOf = (cursor: unknown): number => {
    if (cursor === null || cursor === undefined) return 0;
    const m = typeof cursor === 'string' ? /^c_(\d{1,7})$/.exec(cursor) : null;
    return m ? Number(m[1]) : fail('invalid_request', 'cursor is not valid for this request');
  };
  const page = (anchor: string | null, all: SynthNode[], offset: number, limit: number, hint: boolean): EntryPage => {
    const items = all.slice(offset, offset + limit).map((n) => entryRow(n, hint));
    const end = offset + items.length;
    return { anchor, generation: 1, items, next_cursor: end < all.length ? cursorOf(end) : null, total: all.length, budget_hit: false };
  };

  const slice = (p: TreeSliceRequest): TreeSlice => {
    const a = isObj(p.anchor) ? p.anchor : fail('invalid_request', 'anchor required');
    const anchor =
      a.kind === 'atlas' ? tree.nodes[0]!
      : a.kind === 'node' ? nodeFor(a.node_id)
      : a.kind === 'root' ? (tree.nodes.find((n) => n.kind === 'root' && rootId(n) === a.root_id) ?? fail('not_found', 'no such root'))
      : fail('invalid_request', 'unknown anchor kind');
    const depth = int(p.depth, 'depth', 1, 8);
    const max = int(p.max_nodes, 'max_nodes', 16, 6000);
    const basis: Basis = p.basis === 'allocated' ? 'allocated' : 'logical';
    const minShare = typeof p.min_share === 'number' && p.min_share >= 0 && p.min_share <= 0.1 ? p.min_share : fail('invalid_request', 'min_share');
    const anchorSize = sizeOf(anchor, basis);
    const out: SliceNode[] = [sliceNode(anchor, null, anchor.depth, basis)];
    let truncated = false;
    // Breadth-first, biggest children first, so a budget cut keeps the heaviest nodes (deterministic).
    const queue: [SynthNode, number][] = [[anchor, 0]];
    for (let q = 0; q < queue.length; q++) {
      const [n, parentIdx] = queue[q]!;
      if (out[parentIdx]!.depth - anchor.depth >= depth) continue;
      const kids = n.children
        .map((i) => tree.nodes[i]!)
        .filter((c) => (p.include_files || c.kind !== 'file') && (anchorSize === 0n || Number(sizeOf(c, basis)) / Number(anchorSize) >= minShare))
        .sort((x, y) => (sizeOf(y, basis) > sizeOf(x, basis) ? 1 : sizeOf(y, basis) < sizeOf(x, basis) ? -1 : x.index - y.index));
      for (const c of kids) {
        if (out.length >= max) { truncated = true; break; }
        out.push(sliceNode(c, parentIdx, c.depth, basis));
        queue.push([c, out.length - 1]);
      }
    }
    return { anchor_node_id: anchor.id, basis, root_generations: [], complete: !truncated, live: false, ordering: 'exact', truncated, nodes: out };
  };

  const children = (p: TreeChildrenRequest): EntryPage => {
    const n = nodeFor(p.node_id);
    const basis: Basis = p.basis === 'allocated' ? 'allocated' : 'logical';
    const kids = n.children.map((i) => tree.nodes[i]!);
    if (p.sort === 'name_asc') kids.sort((x, y) => (x.name < y.name ? -1 : x.name > y.name ? 1 : x.index - y.index));
    else if (p.sort === 'modified_desc') kids.sort((x, y) => ((y.modified ?? '') < (x.modified ?? '') ? -1 : (y.modified ?? '') > (x.modified ?? '') ? 1 : x.index - y.index));
    else kids.sort((x, y) => (sizeOf(y, basis) > sizeOf(x, basis) ? 1 : sizeOf(y, basis) < sizeOf(x, basis) ? -1 : x.index - y.index));
    return page(n.id, kids, offsetOf(p.cursor), int(p.limit, 'limit', 1, 200), false);
  };

  const search = (p: SearchRequest): EntryPage => {
    const text = str(p.text, 'text').toLowerCase();
    const limit = int(p.limit, 'limit', 1, 100);
    const hits = tree.nodes.filter((n) => n.kind !== 'atlas' && n.kind !== 'root' && n.name.toLowerCase().includes(text) && (p.kind === 'any' || p.kind === (n.kind === 'file' ? 'file' : 'dir')));
    return page(null, hits, offsetOf(p.cursor), limit, true);
  };

  const views = createViewMock(now, fail);
  const rootRow = (n: SynthNode): Root => ({
    root_id: rootId(n), display_path: { text: `[mock] ${n.name}`, truncated: false }, origin: 'fixture', dataset_class: DATASET,
    volume_id: n.index === tree.rootIndexes[0] ? 'vo_c' : 'vo_g',
    granted_at: GRANTED_AT, granted_via: 'fixture', grant_state: views.revokedRoots.has(rootId(n)) ? 'revoked' : 'active',
    scan: { state: 'complete', finished_at: GRANTED_AT, coverage: n.coverage },
    totals: { files: n.files, dirs: n.dirs, logical_bytes: n.logical.toString(), allocated_bytes: n.allocated === null ? null : n.allocated.toString(), skipped: 0, failed: 0, complete: n.coverage === 'complete' },
  });

  const handlers: Record<string, (payload: Record<string, unknown>) => unknown> = {
    'session.hello': (): SessionInfo => ({
      protocol: 'loomward/3', engine_version: 'mock-0.3 (no engine)', adapter: 'http', dataset_class: DATASET, session_started_at: t0,
      enumeration_strategy: 'std_read_dir',
      capabilities: {
        observation: { metadata_scan: false, process_observation: true, gpu_observation: false, disk_io_observation: false, teacher_disclosure: false },
        effects: { file_move: false, file_delete: false, file_rename: false, file_write: false, content_read: false, process_kill: false, process_suspend: false, process_priority: false, memory_trim: false, uninstall: false, elevation: false },
      },
      features: { grant_picker: false, disclosure_dialog: false, teacher_available: false, telemetry_available: true, gpu_available: false },
      limits: { max_request_bytes: 65536, max_slice_nodes: 6000, max_page_items: 200 },
    }),
    'health.get': (): Health => ({
      observed_at: now().toISOString(),
      engine: { private_commit_bytes: '96468992', working_set_bytes: '75497472', cpu_seconds: 12.4, threads: 18 },
      catalog: { schema_version: 3, db_bytes: null, wal_bytes: null, files: tree.nodes[0]!.files, dirs: tree.nodes[0]!.dirs, writer_queue_depth: 0, writer_queue_capacity: 1024 },
      jobs_running: 0, last_error: null,
      warnings: [{ code: 'telemetry_unavailable', message: 'The mock transport has no GPU or disk counters; those figures are unknown.', at: t0 }],
    }),
    'roots.list': (): RootList => ({ roots: tree.rootIndexes.map((i) => rootRow(tree.nodes[i]!)) }),
    'grants.list': (): GrantList => ({
      grants: [
        ...tree.rootIndexes.map((i, k) => ({ grant_id: `gr_mock_root_${k}`, kind: 'metadata_root' as const, root_id: rootId(tree.nodes[i]!), granted_at: GRANTED_AT, granted_via: 'fixture' as const, revoked_at: views.revokedRoots.get(rootId(tree.nodes[i]!)) ?? null })),
        { grant_id: 'gr_mock_teacher_0', kind: 'teacher_disclosure' as const, recipient: 'codex_cli', dataset_class: DATASET, fields: ['name', 'extension'], item_count: 25, payload_digest: 'sha256:' + '0'.repeat(64), created_at: GRANTED_AT, expires_at: '2026-10-01T10:00:00Z', revoked_at: views.revokedGrants.get('gr_mock_teacher_0') ?? null, used: false, confirmed_via: 'synthetic_policy' as const },
      ],
    }),
    'tree.slice': (p) => slice(p as unknown as TreeSliceRequest),
    'tree.children': (p) => children(p as unknown as TreeChildrenRequest),
    'search.query': (p) => search(p as unknown as SearchRequest),
    ...views.handlers,
  };

  const respond = (req: RequestEnvelope): ResponseEnvelope => {
    const base = { protocol: 'loomward/3' as const, request_id: req.request_id };
    const err = (code: ErrorCode, message: string): ResponseEnvelope => ({ ...base, ok: false, error: { code, message, retryable: false, detail: null } });
    if (req.protocol !== 'loomward/3') return err('unsupported_protocol', 'expected loomward/3');
    const h = Object.hasOwn(handlers, req.command) ? handlers[req.command] : undefined;
    if (!h) return err('capability_unavailable', `the mock transport does not implement ${req.command}`);
    try {
      return { ...base, ok: true, result: h((req.payload ?? {}) as Record<string, unknown>), meta: { served_at: now().toISOString(), elapsed_ms: 0, dataset_class: DATASET, budget_hit: false } };
    } catch (e) {
      if (e instanceof CommandFailure) return err(e.code, e.message);
      throw e;
    }
  };

  return {
    mode: 'mock',
    call: async (req) => respond(req),
    subscribe(onEvent, onState) {
      let live = true;
      // Asynchronous like a real stream: the caller has finished wiring before anything arrives.
      queueMicrotask(() => {
        if (!live) return;
        onState('open');
        const hello: EventEnvelope = { protocol: 'loomward/3', seq: 0, event: 'stream.hello', at: now().toISOString(), data: {} };
        onEvent(hello);
      });
      return () => { live = false; };
    },
  };
}
