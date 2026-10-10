import type {
  EventKey, EventMap, Basis, DatasetClass, EntryPage, EntryRow, ErrorCode, EventEnvelope, GrantList, Health, RequestEnvelope,
  ResponseEnvelope, Root, RootList, SearchRequest, SessionInfo, SliceNode, TreeChildrenRequest, TreeSlice,
  TreeSliceRequest, Threads,
} from '../contracts.gen';
import type { TelemetrySample } from '../contracts.gen';
import { createViewMock } from './mock-views';
import { generateTree, prng, type SynthNode } from './synth';
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
/** The synthetic catalogue never changes, so its revision is fixed. The state.db revision comes from the view mock. */
const CATALOG_REV = '1';
const EPOCH = 'e_mock_epoch';
/** Which revision scopes (semantics.md section 5) a command's response is read at: catalogue, state.db, both, or neither. */
const REVS: Partial<Record<string, 'c' | 's' | 'cs'>> = {
  'health.get': 'c', 'tree.children': 'c', 'search.query': 'c',
  'roots.list': 'cs', 'tree.slice': 'cs', 'placement.candidates': 'cs', 'placement.simulate': 'cs',
  'roots.revoke': 's', 'grants.list': 's', 'grants.revoke': 's', 'volumes.list': 's', 'tiers.model': 's',
};

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

  // Synthetic threads: invented labels, tiers and grants so the atlas has something to weave.
  // Alpha sits on a hot volume (some subtrees on a warm one), Beta on a cold one; a few files have
  // unknown residency, a few are cloud placeholders, a few are suggestions or still pending.
  const LABEL: Partial<Record<string, string>> = {
    image: 'Media', video: 'Media', audio: 'Media', document: 'Documents', spreadsheet: 'Documents', presentation: 'Documents',
    code: 'Projects', data: 'Projects', archive: 'Archives', model: 'Models', executable: 'System', system: 'System', font: 'System',
  };
  const DIR_LABELS = ['Projects', 'Media', 'Documents', 'Archives', 'Models'];
  const topOf = (n: SynthNode): SynthNode[] => { const chain: SynthNode[] = []; for (let p = n; p.parent >= 0; p = tree.nodes[p.parent]!) chain.unshift(p); return chain; };
  const threadsOf = (n: SynthNode): Threads => {
    const chain = topOf(n);
    const root = chain[0], sub = chain[1];
    const meaning: Threads['meaning'] =
      n.kind === 'atlas' || n.kind === 'root' ? { state: 'mixed', label: null, share: null, source: null, collection_ids: [] }
      : n.kind === 'dir' ? { state: 'mixed', label: DIR_LABELS[n.index % DIR_LABELS.length]!, share: 0.6, source: 'human', collection_ids: [] }
      : n.index % 17 === 0 ? { state: 'unknown', label: null, share: null, source: null, collection_ids: [] }
      : !LABEL[n.family ?? ''] ? { state: 'none', label: null, share: null, source: null, collection_ids: [] }
      : n.index % 11 === 0 ? { state: 'suggested', label: LABEL[n.family!]!, share: null, source: 'student', collection_ids: [] }
      : n.index % 13 === 0 ? { state: 'pending', label: null, share: null, source: null, collection_ids: [] }
      : { state: 'labelled', label: LABEL[n.family!]!, share: null, source: 'human', collection_ids: [] };
    const residency: Threads['residency'] =
      !root || n.kind === 'atlas' ? { volume_id: null, tier: null, tier_basis: 'unknown' }
      : n.kind === 'file' && n.index % 97 === 0 ? { volume_id: null, tier: null, tier_basis: 'unknown' }
      : root.index === tree.rootIndexes[0] ? (sub && sub.index % 3 === 0 ? { volume_id: 'vol_mock_g', tier: 1, tier_basis: 'device_hint' } : { volume_id: 'vol_mock_c', tier: 0, tier_basis: 'declared' })
      : { volume_id: 'vol_mock_e', tier: 2, tier_basis: 'declared' };
    const permission: Threads['permission'] =
      n.coverage === 'denied' ? { state: 'denied', reason: 'access_denied' }
      : n.coverage === 'partial' ? { state: 'partial', reason: 'mixed_children' }
      : n.kind === 'file' && n.extension === 'docx' && n.index % 5 === 0 ? { state: 'granted', reason: 'cloud_placeholder' }
      : { state: 'granted', reason: 'metadata_grant_active' };
    return { meaning, residency, permission };
  };
  // Synthetic stand-in for a running scan: slices under the second root are provisional.
  const provisional = (n: SynthNode) => tree.rootIndexes[1] !== undefined && topOf(n)[0]?.index === tree.rootIndexes[1];

  const sliceNode = (n: SynthNode, parent: number | null, depth: number, basis: Basis): SliceNode => ({
    node_id: n.id, parent, kind: n.kind, name: n.name, depth,
    size_bytes: sizeOf(n, basis).toString(), size_unknown_files: n.unknownFiles,
    logical_bytes: n.logical.toString(), allocated_bytes: n.allocated === null ? null : n.allocated.toString(),
    files: n.files, dirs: n.dirs, child_count: n.kind === 'file' ? 0 : n.coverage === 'denied' ? null : n.children.length,
    folded_count: null, coverage: n.coverage, live: provisional(n), ext_family: n.family, modified_at: n.modified, threads: threadsOf(n),
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
    return { anchor, generation: '1', items, next_cursor: end < all.length ? cursorOf(end) : null, total: all.length, budget_hit: false };
  };

  /** Roots whose subtree the anchor covers: the atlas covers all, anything below covers its own root. */
  const rootsUnder = (n: SynthNode): SynthNode[] => (n.kind === 'atlas' ? tree.rootIndexes.map((i) => tree.nodes[i]!) : topOf(n).slice(0, 1));
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
    const live = provisional(anchor);
    return {
      anchor_node_id: anchor.id, basis, root_generations: rootsUnder(anchor).map((r) => ({ root_id: rootId(r), generation: CATALOG_REV })), complete: !truncated, live,
      aggregate_state: live ? 'provisional_live' : 'consistent', ordering: live ? 'approximate_live' : 'exact', truncated, nodes: out,
    };
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
    const ext = typeof p.extension === 'string' ? p.extension.toLowerCase() : null;
    const min = typeof p.min_bytes === 'string' && /^\d{1,19}$/.test(p.min_bytes) ? BigInt(p.min_bytes) : 0n;
    const hits = tree.nodes.filter(
      (n) => n.kind !== 'atlas' && n.kind !== 'root' && n.name.toLowerCase().includes(text) && (p.kind === 'any' || p.kind === (n.kind === 'file' ? 'file' : 'dir'))
        && (ext === null || n.extension?.toLowerCase() === ext) && n.logical >= min,
    );
    return page(null, hits, offsetOf(p.cursor), limit, true);
  };

  const views = createViewMock(now, fail);
  // Synthetic GPU and disk channels for the Observatory (L10), layered on the view mock's system and
  // engine channels (L13), so one telemetry.snapshot serves both. A deterministic random walk with
  // periodic "build" and "render" episodes; rates are null on the first sample, as the contract requires.
  const tRnd = prng((opts.seed ?? 1) ^ 0x7e1e);
  const GiB = 2 ** 30;
  const tel = { gpu: 0.08, vram: 6.2 * GiB, io: [[40e6, 12e6], [18e6, 6e6], [2e6, 0.4e6]] as [number, number][] };
  const walk = (v: number, target: number, k: number, noise: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v + (target - v) * k + (tRnd() - 0.5) * noise));
  const telemetry = (p: Record<string, unknown>): TelemetrySample => {
    const base = views.handlers['telemetry.snapshot']!(p) as TelemetrySample;
    const t = base.sample_seq, first = base.elapsed_ms === null;
    const build = Math.sin(t / 37) > 0.55, render = Math.sin(t / 53 + 1) > 0.7;
    tel.gpu = walk(tel.gpu, render ? 0.82 : 0.07, 0.15, 0.07, 0, 1);
    tel.vram = walk(tel.vram, (render ? 19.5 : 6.4) * GiB, 0.12, 0.4 * GiB, 2 * GiB, 24 * GiB);
    const targets: [number, number][] = [[build ? 620e6 : 35e6, build ? 410e6 : 10e6], [render ? 1400e6 : 20e6, 8e6], [t % 90 > 70 ? 160e6 : 1.5e6, 0.4e6]];
    tel.io = tel.io.map(([r, w], i) => [walk(r, targets[i]![0], 0.3, 40e6, 0, 3.5e9), walk(w, targets[i]![1], 0.3, 25e6, 0, 3e9)]);
    const b = (n: number) => String(Math.round(n));
    return {
      ...base,
      gpu: { state: 'observed', basis: 'pdh_gpu_counters', adapters: [{ adapter_id: 'gpu_mock_0', name: 'Synthetic GPU', dedicated_total_bytes: b(24 * GiB), dedicated_used_bytes: b(tel.vram), shared_used_bytes: b(0.4 * GiB), engine_busy_fraction: first ? null : tel.gpu }] },
      disks: {
        state: 'observed',
        disks: (['C', 'G', 'E'] as const).map((l, i) => ({ disk_label: `Disk ${i} (${l}:)`, volume_ids: [`vol_mock_${l.toLowerCase()}`], read_bytes_per_s: first ? null : tel.io[i]![0], write_bytes_per_s: first ? null : tel.io[i]![1], busy_fraction: first ? null : Math.min(1, (tel.io[i]![0] + tel.io[i]![1]) / (i === 2 ? 2.2e8 : 3.5e9)), queue_length: first ? null : 0.1 })),
      },
    };
  };
  const rootRow = (n: SynthNode): Root => ({
    root_id: rootId(n), display_path: { text: `[mock] ${n.name}`, truncated: false }, origin: 'fixture', dataset_class: DATASET,
    volume_id: n.index === tree.rootIndexes[0] ? 'vo_c' : 'vo_g',
    granted_at: GRANTED_AT, granted_via: 'fixture', grant_state: views.revokedRoots.has(rootId(n)) ? 'revoked' : 'active',
    scan: { state: 'complete', last_job_id: null, generation: CATALOG_REV, finished_at: GRANTED_AT, coverage: n.coverage },
    totals: {
      files: n.files, dirs: n.dirs, logical_bytes: n.logical.toString(), allocated_bytes: n.allocated === null ? null : n.allocated.toString(),
      allocation_unknown_files: n.unknownFiles, skipped: 0, failed: 0, complete: n.coverage === 'complete', stream_coverage: 'default_stream_only',
      unique_objects: null, unique_allocated_bytes: null, multi_link_entries: 0, // the synthetic catalogue has no native file IDs: unknown, not zero
    },
  });

  const handlers: Record<string, (payload: Record<string, unknown>) => unknown> = {
    'session.hello': (): SessionInfo => ({
      protocol: 'loomward/3', engine_version: 'mock-0.3 (no engine)', adapter: 'http', dataset_class: DATASET, session_started_at: t0,
      enumeration_strategy: 'std_read_dir',
      capabilities: {
        observation: { metadata_scan: false, process_observation: true, gpu_observation: false, disk_io_observation: false, teacher_disclosure: false },
        effects: { file_move: false, file_delete: false, file_rename: false, file_write: false, content_read: false, process_kill: false, process_suspend: false, process_priority: false, memory_trim: false, uninstall: false, elevation: false },
      },
      features: { grant_picker: false, disclosure_dialog: false, teacher_available: false, telemetry_available: true, gpu_available: true },
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
        { grant_id: 'gr_mock_teacher_0', kind: 'teacher_disclosure' as const, recipient: 'codex_cli_gpt_6_1_sol', dataset_class: DATASET, fields: ['name', 'extension'], item_count: 25, payload_digest: 'sha256:' + '0'.repeat(64), created_at: GRANTED_AT, expires_at: '2026-10-01T10:00:00Z', revoked_at: views.revokedGrants.get('gr_mock_teacher_0') ?? null, used: false, confirmed_via: 'synthetic_policy' as const },
      ],
    }),
    'tree.slice': (p) => slice(p as unknown as TreeSliceRequest),
    'tree.children': (p) => children(p as unknown as TreeChildrenRequest),
    'search.query': (p) => search(p as unknown as SearchRequest),
    ...views.handlers,
    'telemetry.snapshot': telemetry, // after the spread: wraps the view mock's sample with gpu and disks
  };

  const respond = (req: RequestEnvelope): ResponseEnvelope => {
    const base = { protocol: 'loomward/3' as const, request_id: req.request_id };
    const err = (code: ErrorCode, message: string): ResponseEnvelope => ({ ...base, ok: false, error: { code, message, retryable: false, detail: null } });
    if (req.protocol !== 'loomward/3') return err('unsupported_protocol', 'expected loomward/3');
    const h = Object.hasOwn(handlers, req.command) ? handlers[req.command] : undefined;
    if (!h) return err('capability_unavailable', `the mock transport does not implement ${req.command}`);
    try {
      const result = h((req.payload ?? {}) as Record<string, unknown>) as Record<string, unknown>;
      const scope = REVS[req.command] ?? '';
      // Read after the handler, so a mutation reports the revision it committed.
      const meta = {
        served_at: now().toISOString(), elapsed_ms: 0, dataset_class: DATASET, budget_hit: false,
        catalog_rev: scope.includes('c') ? CATALOG_REV : null, state_rev: scope.includes('s') ? views.stateRev() : null,
      };
      return { ...base, ok: true, result, meta };
    } catch (e) {
      if (e instanceof CommandFailure) return err(e.code, e.message);
      throw e;
    }
  };

  return {
    mode: 'mock',
    call: async (req) => respond(req),
    subscribe(onEvent, onState, resume) {
      let live = true;
      // Asynchronous like a real stream: the caller has finished wiring before anything arrives.
      queueMicrotask(() => {
        if (!live) return;
        onState('open');
        const at = now().toISOString();
        const env = <E extends EventKey>(event: E, data: EventMap[E]): EventEnvelope => ({ protocol: 'loomward/3', epoch: EPOCH, seq: 0, event, at, catalog_rev: null, state_rev: null, data: data as unknown as EventEnvelope['data'] });
        // The mock emits no sequenced events, so last_seq stays 0 and a resume in this epoch has nothing to replay.
        onEvent(env('stream.hello', { session_started_at: t0, epoch: EPOCH, last_seq: 0, oldest_replayable_seq: 1, dataset_class: DATASET }));
        if (resume && resume.epoch !== EPOCH) onEvent(env('stream.lagged', { reason: 'epoch_changed', dropped: null, resync: ['roots', 'volumes', 'jobs', 'tree', 'learning'] }));
      });
      return () => { live = false; };
    },
  };
}
