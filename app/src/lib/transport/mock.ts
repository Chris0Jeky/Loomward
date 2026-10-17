import type {
  EventKey, EventMap, Basis, DatasetClass, EntryPage, EntryRow, ErrorCode, EventEnvelope, GrantList, GrantRevokeResult, RootRevokeResult, Health, RequestEnvelope,
  ResponseEnvelope, Root, RootList, SearchHit, SearchPage, SearchRequest, SessionInfo, SliceNode, TreeChildrenRequest, TreeSlice,
  TreeSliceRequest, Threads,
} from '../contracts.gen';
import type { TelemetrySample } from '../contracts.gen';
import { createCollectionMock } from './mock-collections';
import { createScanMock } from './mock-scan';
import { createViewMock } from './mock-views';
import { createInventoryMock } from './mock-inventory';
import { createAlertMock } from './mock-alerts';
import { createAskMock } from './mock-ask';
import { createBackupMock } from './mock-backups';
import diagnosticsFixture from '../../../../contracts/v3/examples/commands/diagnostics.preview.result.json';
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
/** The synthetic catalogue's scan generation is fixed. Both database revisions come from the view mock. */
const GENERATION = '1';
const EPOCH = 'e_mock_epoch';
/** Which revision scopes (semantics.md section 5) a command's response is read at: catalogue, state.db, both, or neither. */
const REVS: Partial<Record<string, 'c' | 's' | 'cs'>> = {
  'collections.list': 'cs', 'collections.create': 'cs', 'collections.members': 'cs', 'collections.update_members': 'cs',
  'node.inspect': 'cs', 'threads.meaning': 'cs',
  'health.get': 'c', 'tree.children': 'c', 'search.query': 'c',
  'roots.list': 'cs', 'tree.slice': 'cs', 'placement.candidates': 'cs', 'placement.simulate': 'cs',
  'proposals.list': 'cs', 'proposals.get': 'cs', 'volumes.declare_tier': 'cs',
  'roots.revoke': 'cs', 'roots.forget': 'cs', 'backups.list': 's', 'backups.forget': 's', 'alerts.list': 's', 'alerts.configure': 's', 'grants.list': 's', 'grants.revoke': 's', 'volumes.list': 's', 'tiers.model': 's',
};

class CommandFailure extends Error {
  constructor(readonly code: ErrorCode, message: string, readonly detail: Record<string, unknown> | null = null) {
    super(message);
  }
}
const failWith = (code: ErrorCode, message: string, detail: Record<string, unknown> | null = null): never => {
  throw new CommandFailure(code, message, detail);
};
const fail = (code: ErrorCode, message: string): never => failWith(code, message);

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

  // A cursor binds catalog_rev (semantics 5): one issued before a catalogue commit is stale_generation.
  const cursorOf = (offset: number) => `c_${views.catalogRev()}_${offset}`;
  const offsetOf = (cursor: unknown): number => {
    if (cursor === null || cursor === undefined) return 0;
    const m = typeof cursor === 'string' ? /^c_(\d{1,9})_(\d{1,7})$/.exec(cursor) : null;
    if (!m) return fail('invalid_request', 'cursor is not valid for this request');
    return m[1] === views.catalogRev() ? Number(m[2]) : fail('stale_generation', 'the catalogue changed since this cursor was issued');
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
      anchor_node_id: anchor.id, basis, root_generations: rootsUnder(anchor).map((r) => ({ root_id: rootId(r), generation: GENERATION })), complete: !truncated, live,
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

  // The engine's tiers (semantics.md section 5, LW-113): exact, prefix, token (after a separator), substring; then
  // every word at a word start, every word anywhere. Folders before files inside a tier, then node order.
  const SEPARATORS = " _-.([{,+&@#~'=!";
  const at = (name: string, q: string, word: boolean): number => {
    for (let i = name.indexOf(q); i >= 0; i = name.indexOf(q, i + 1)) if (!word || i === 0 || SEPARATORS.includes(name[i - 1]!)) return i;
    return -1;
  };
  type SearchHitMatchKind = SearchHit['match_kind'];
  const classify = (name: string, q: string): [SearchHitMatchKind, number[][]] | null => {
    const n = name.toLowerCase();
    if (n === q || n.startsWith(`${q}.`)) return ['exact', [[0, q.length]]];
    if (q.length >= 2 && n.startsWith(q)) return ['prefix', [[0, q.length]]];
    const tok = q.length >= 2 ? at(n, q, true) : -1;
    if (tok >= 0) return ['token', [[tok, tok + q.length]]];
    const sub = q.length >= 3 ? n.indexOf(q) : -1;
    if (sub >= 0) return ['substring', [[sub, sub + q.length]]];
    const words = q.split(/\s+/);
    if (words.length < 2) return null;
    for (const word of [true, false]) {
      const spans = words.map((w) => (w.length >= (word ? 2 : 3) ? at(n, w, word) : -1));
      if (spans.every((s) => s >= 0)) return [word ? 'token' : 'substring', spans.map((s, i) => [s, s + words[i]!.length]).sort((x, y) => x[0]! - y[0]!)];
    }
    return null;
  };
  const TIER: Record<SearchHitMatchKind, number> = { exact: 0, prefix: 1, token: 2, substring: 3, filter: 4, semantic: 5 };
  const search = (p: SearchRequest): SearchPage => {
    // Like the engine (docs/46 F1): the federated fields are contract only until features.federated_search.
    if (p.sources !== undefined || p.terms !== undefined || p.extensions !== undefined || p.collection_id !== undefined || p.under_node_id !== undefined) {
      fail('capability_unavailable', 'federated search fields are not served (features.federated_search is false)');
    }
    const text = str(p.text, 'text').trim().toLowerCase();
    const limit = int(p.limit, 'limit', 1, 100);
    const ext = typeof p.extension === 'string' ? p.extension.toLowerCase() : null;
    const bytes = (v: unknown) => (typeof v === 'string' && /^\d{1,19}$/.test(v) ? BigInt(v) : null);
    const min = bytes(p.min_bytes) ?? 0n, max = bytes(p.max_bytes);
    const after = typeof p.modified_after === 'string' ? p.modified_after : null, before = typeof p.modified_before === 'string' ? p.modified_before : null;
    const root = p.root_id === null || p.root_id === undefined ? null : str(p.root_id, 'root_id');
    const live = (n: SynthNode) => { const r = topOf(n)[0]; return !!r && !views.revokedRoots.has(rootId(r)) && (root === null || rootId(r) === root); };
    const hits: [SynthNode, SearchHitMatchKind, number[][]][] = [];
    for (const n of tree.nodes) {
      if (n.kind === 'atlas' || n.kind === 'root' || !live(n)) continue;
      if (!(p.kind === 'any' || p.kind === (n.kind === 'file' ? 'file' : 'dir')) || (ext !== null && n.extension?.toLowerCase() !== ext)) continue;
      if (n.logical < min || (max !== null && n.logical > max)) continue;
      if ((after !== null || before !== null) && (n.modified === null || (after !== null && n.modified < after) || (before !== null && n.modified > before))) continue;
      const c: [SearchHitMatchKind, number[][]] | null = text ? classify(n.name, text) : ['filter', []];
      if (c) hits.push([n, c[0], c[1]]);
    }
    const byKind = (x: SynthNode, y: SynthNode) => Number(x.kind === 'file') - Number(y.kind === 'file') || x.index - y.index;
    hits.sort(([x, a], [y, b]) => TIER[a] - TIER[b] || (text || ext === null ? byKind(x, y) : y.logical > x.logical ? 1 : y.logical < x.logical ? -1 : x.index - y.index));
    const offset = offsetOf(p.cursor);
    const shown = hits.slice(offset, offset + limit);
    const end = offset + shown.length;
    const roots = (root === null ? tree.rootIndexes.map((i) => tree.nodes[i]!) : tree.rootIndexes.map((i) => tree.nodes[i]!).filter((r) => rootId(r) === root))
      .filter((r) => !views.revokedRoots.has(rootId(r)));
    const relative = (n: SynthNode) => topOf(n).slice(1).map((a) => a.name).join('\\');
    return {
      generation: '1',
      hits: shown.map(([n, match_kind, highlights]) => ({ row: entryRow(n, true), root_id: rootId(topOf(n)[0]!), path: { text: relative(n), truncated: false }, match_kind, highlights })),
      next_cursor: end < hits.length ? cursorOf(end) : null, budget_hit: false,
      index_state: 'current', partial: roots.some((r) => r.coverage !== 'complete'),
      substring_searched: text === '' || text.split(/\s+/).every((w) => w.length >= 3),
      roots: roots.map((r) => ({ root_id: rootId(r), scan_state: r.coverage === 'complete' ? 'complete' : 'partial', coverage: r.coverage })),
    };
  };

  const views = createViewMock(now, fail);
  // The stream: sequenced events from 1 after the hello (semantics.md section 7), to every open subscriber.
  const listeners = new Set<(ev: EventEnvelope) => void>();
  let seq = 0;
  const emit = <E extends EventKey>(event: E, data: EventMap[E]): void => {
    const ev: EventEnvelope = { protocol: 'loomward/3', epoch: EPOCH, seq: ++seq, event, at: now().toISOString(), catalog_rev: event === 'tree.invalidated' ? views.catalogRev() : null, state_rev: null, data: data as unknown as EventEnvelope['data'] };
    for (const l of listeners) l(ev);
  };
  const scan = createScanMock({ now, fail: failWith, emit, isRevoked: (id) => views.revokedRoots.has(id) });
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
    scan: { state: 'complete', last_job_id: null, generation: GENERATION, finished_at: GRANTED_AT, coverage: n.coverage },
    totals: {
      files: n.files, dirs: n.dirs, logical_bytes: n.logical.toString(), allocated_bytes: n.allocated === null ? null : n.allocated.toString(),
      allocation_unknown_files: n.unknownFiles, skipped: 0, failed: 0, complete: n.coverage === 'complete', stream_coverage: 'default_stream_only',
      unique_objects: null, unique_allocated_bytes: null, multi_link_entries: 0, // the schema's Count has no null: this 0 is a placeholder, the synthetic catalogue has no native file IDs to count
    },
  });

  const HOSTILE_ROOT_ID = 'rt_mock_9';
  const hostileRoot = (): Root => ({
    ...rootRow(tree.nodes[tree.rootIndexes[1]!]!), root_id: HOSTILE_ROOT_ID, display_path: { text: '[mock] reports‮txt.exe', truncated: false },
    grant_state: views.revokedRoots.has(HOSTILE_ROOT_ID) ? 'revoked' : 'active',
  });

  /** One grant per root, as in the engine: revoking either one revokes both, and both lists show it. */
  const rootGrants = () => [
    ...tree.rootIndexes.map((i, k) => ({ grant_id: `gr_mock_root_${k}`, root_id: rootId(tree.nodes[i]!) })),
    { grant_id: 'gr_mock_root_2', root_id: HOSTILE_ROOT_ID },
    ...scan.roots().map((r, k) => ({ grant_id: `gr_mock_lab_${k}`, root_id: r.root_id })),
  ];

  const handlers: Record<string, (payload: Record<string, unknown>) => unknown> = {
    'session.hello': (): SessionInfo => ({
      protocol: 'loomward/3', engine_version: 'mock-0.3 (no engine)', adapter: 'http', dataset_class: DATASET, session_started_at: t0,
      enumeration_strategy: 'std_read_dir',
      capabilities: {
        observation: { metadata_scan: true, process_observation: true, gpu_observation: false, disk_io_observation: false, teacher_disclosure: false },
        effects: { file_move: false, file_delete: false, file_rename: false, file_write: false, content_read: false, process_kill: false, process_suspend: false, process_priority: false, memory_trim: false, uninstall: false, elevation: false },
      },
      features: { grant_picker: false, disclosure_dialog: false, teacher_available: false, telemetry_available: true, gpu_available: true, diagnostics_save: false, federated_search: false },
      limits: { max_request_bytes: 65536, max_slice_nodes: 6000, max_page_items: 200 },
    }),
    'health.get': (): Health => ({
      observed_at: now().toISOString(),
      engine: { private_commit_bytes: '96468992', working_set_bytes: '75497472', cpu_seconds: 12.4, threads: 18 },
      catalog: { schema_version: 3, db_bytes: null, wal_bytes: null, files: tree.nodes[0]!.files, dirs: tree.nodes[0]!.dirs, writer_queue_depth: 0, writer_queue_capacity: 1024 },
      jobs_running: 0, last_error: null,
      warnings: [{ code: 'telemetry_unavailable', message: 'The mock transport has no GPU or disk counters; those figures are unknown.', at: t0 }],
    }),
    // A third root whose path hides a right-to-left override: Grants & health must show and speak it escaped.
    'roots.list': (): RootList => ({ roots: [...tree.rootIndexes.map((i) => rootRow(tree.nodes[i]!)), hostileRoot(), ...scan.roots()].filter((r) => !views.forgottenRoots.has(r.root_id)) }),
    'grants.list': (): GrantList => ({
      grants: [
        ...rootGrants().map((g) => ({ ...g, kind: 'metadata_root' as const, granted_at: GRANTED_AT, granted_via: 'fixture' as const, revoked_at: views.revokedGrants.get(g.grant_id) ?? views.revokedRoots.get(g.root_id) ?? null, forgotten_at: views.forgottenRoots.get(g.root_id) ?? null })),
        { grant_id: 'gr_mock_teacher_0', kind: 'teacher_disclosure' as const, recipient: 'codex_cli_gpt_6_1_sol', dataset_class: DATASET, fields: ['name', 'extension'], item_count: 25, payload_digest: 'sha256:' + '0'.repeat(64), created_at: GRANTED_AT, expires_at: '2026-10-01T10:00:00Z', revoked_at: views.revokedGrants.get('gr_mock_teacher_0') ?? null, used: false, confirmed_via: 'synthetic_policy' as const },
      ],
    }),
    'node.inspect': (p) => {
      const n = nodeFor(p.node_id);
      return { row: entryRow(n, true), root_id: rootId(topOf(n)[0]!), volume_id: null,
        display_path: { text: pathOf(n), truncated: false }, identity: { quality: 'native_file_id_128', volume_key: 'synthetic', file_id_hex: '0'.repeat(32), observed_generation: '1', authorises_effects: false, durable_reference: 'available' },
        timestamps: { created_at: null, modified_at: n.modified, accessed_at: null, changed_at: null }, subtree: null,
        threads: threadsOf(n), exclusions: [], denied_capabilities: [], memberships: [], suggestion: null };
    },
 'threads.meaning': (p) => {
      if (!Array.isArray(p.node_ids) || p.node_ids.length < 1 || p.node_ids.length > 6000 || new Set(p.node_ids).size !== p.node_ids.length) return fail('invalid_request', 'meaning requires 1..6000 unique node IDs');
      return { model_id: 'md_mock_meaning', budget_hit: p.node_ids.length > 1000, items: p.node_ids.slice(0, 1000).map(id => {
        const n = nodeFor(id);
        const eligible = n.kind === 'file' && n.coverage !== 'denied';
        const label = LABEL[n.family ?? ''] ?? 'Documents';
        const abstain = !LABEL[n.family ?? ''] || n.index % 7 === 0;
        return { node_id: n.id, meaning: { state: !eligible ? 'not_eligible' : abstain ? 'abstained' : 'suggested', label: eligible && !abstain ? label : null, share: eligible && !abstain ? 0.87 : null, source: eligible ? 'student' : null, collection_ids: [], ...(eligible ? { suggestion: { node_id: n.id, model_id: 'md_mock_meaning', ranked: [{ label, score: abstain ? 0.3 : 0.87 }], abstain, reasons: abstain ? ['unfamiliar_features'] : [], feature_coverage: abstain ? 0.1 : 0.9, review_priority: abstain ? 0.8 : 0.1, calibrated: false, autonomy_allowed: false, teacher: null } } : {}) } };
      }) };
    },
        'tree.slice': (p) => slice(p as unknown as TreeSliceRequest),
    'tree.children': (p) => children(p as unknown as TreeChildrenRequest),
    'search.query': (p) => search(p as unknown as SearchRequest),
    ...views.handlers,
    ...createInventoryMock(now, fail),
    ...createBackupMock(now, fail),
    ...createAlertMock(now, fail),
    // The contract's synthetic bundle, whose size and digest are those of its canonical text. Like HTTP, the mock
    // has no Rust-owned save dialog, so diagnostics.save answers capability_unavailable.
    'diagnostics.preview': (p) => {
      if (Object.keys(p).length) fail('invalid_request', 'diagnostics.preview takes no fields');
      return structuredClone(diagnosticsFixture);
    },
    'roots.revoke': (p) => {
      const r = views.handlers['roots.revoke']!(p) as RootRevokeResult;
      scan.revoke(r.root_id);
      return r;
    },
    'grants.revoke': (p) => {
      const r = views.handlers['grants.revoke']!(p) as GrantRevokeResult;
      const g = rootGrants().find((x) => x.grant_id === r.grant_id);
      if (g && !views.revokedRoots.has(g.root_id)) views.revokedRoots.set(g.root_id, r.revoked_at); // roots.list shows it too
      if (g) scan.revoke(g.root_id);
      return r;
    },
    ...scan.handlers,
    'telemetry.snapshot': telemetry, // after the spread: wraps the view mock's sample with gpu and disks
  };

  // LW-118: a scripted fake model over this transport's own roots and search; no endpoint, no network.
  Object.assign(handlers, createAskMock(now, failWith, { roots: () => handlers['roots.list']!({}) as RootList, search }));

  const collections = createCollectionMock(now, failWith, nodeFor, entryRow,
    (n) => topOf(n).some((a) => views.revokedRoots.has(rootId(a))), views.stateRev);
  const retained = new Map<string, { binding: string; at: number; response: Promise<ResponseEnvelope> }>();
  const respond = async (req: RequestEnvelope): Promise<ResponseEnvelope> => {
    const base = { protocol: 'loomward/3' as const, request_id: req.request_id };
    const err = (code: ErrorCode, message: string, detail: Record<string, unknown> | null = null): ResponseEnvelope => ({ ...base, ok: false, error: { code, message, retryable: code === 'busy', detail } });
    if (req.protocol !== 'loomward/3') return err('unsupported_protocol', 'expected loomward/3');
    // Like the engine (docs/47 C4): the copy commands are contract only until their effect gate opens.
    if (req.command.startsWith('copy.')) return err('capability_unavailable', 'copy-and-verify is not open: its effect gate (docs/47 lane C8) is closed', { reason: 'copy_effect_gated' });
    const h = Object.hasOwn(handlers, req.command) ? handlers[req.command] : undefined;
    if (!h && !req.command.startsWith('collections.')) return err('capability_unavailable', `the mock transport does not implement ${req.command}`);
    try {
      if (req.command === 'roots.forget' && req.expected_state_rev != null && String(req.expected_state_rev) !== collections.stateRev()) {
        failWith('stale_generation', 'state.db changed since your read; re-read and ask the owner again', { reason: 'state_rev_changed' });
      }
      const result = await (req.command.startsWith('collections.') ? collections.handle(req) : h!((req.payload ?? {}) as Record<string, unknown>)) as Record<string, unknown>;
      const scope = REVS[req.command] ?? '';
      // Read after the handler, so a mutation reports the revision it committed.
      const meta = {
        served_at: now().toISOString(), elapsed_ms: 0, dataset_class: DATASET, budget_hit: result.budget_hit === true,
        catalog_rev: scope.includes('c') ? views.catalogRev() : null, state_rev: scope.includes('s') ? collections.stateRev() : null,
      };
      return { ...base, ok: true, result, meta };
    } catch (e) {
      if (e instanceof CommandFailure) return err(e.code, e.message, e.detail);
      throw e;
    }
  };

  return {
    mode: 'mock',
    call: async (req) => {
      if (req.command !== 'collections.create' && req.command !== 'collections.update_members') return respond(req);
      const binding = JSON.stringify([req.command, req.payload, req.expected_state_rev]);
      const saved = retained.get(req.request_id);
      if (saved && now().getTime() - saved.at < 600_000) {
        if (saved.binding === binding) return structuredClone(await saved.response);
        return { protocol: 'loomward/3', request_id: req.request_id, ok: false, error: { code: 'invalid_request', message: 'This request_id was used for a different request.', retryable: false, detail: { reason: 'idempotency_conflict' } } };
      }
      const response = respond(req);
      if (retained.size >= 2048) retained.delete(retained.keys().next().value!);
      retained.set(req.request_id, { binding, at: now().getTime(), response });
      return structuredClone(await response);
    },
    subscribe(onEvent, onState, resume) {
      let live = true;
      listeners.add(onEvent);
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
      return () => { live = false; listeners.delete(onEvent); };
    },
  };
}
