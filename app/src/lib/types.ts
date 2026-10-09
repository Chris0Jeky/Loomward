// TEMPORARY, hand-written subset of contracts/v3/view-service.schema.json.
// Lane L1 generates src/lib/contracts.gen.ts from the schema; when it lands, delete this file and
// re-point the imports at it. Do not extend this beyond what the shell and placeholder views use.

/** Exact unsigned decimal string (ADR-V3-04). Never a JSON number. */
export type ByteCount = string;
export type NullableBytes = ByteCount | null;
export type NodeId = string;
export type RootId = string;
export type Cursor = string;
export type Timestamp = string;
export type DatasetClass = 'synthetic' | 'personal';
export type Basis = 'logical' | 'allocated';
export type CoverageState =
  | 'complete' | 'partial' | 'stale' | 'unscanned' | 'denied' | 'excluded' | 'cancelled' | 'unknown';
export type ExtFamily =
  | 'document' | 'spreadsheet' | 'presentation' | 'image' | 'video' | 'audio' | 'archive' | 'code'
  | 'data' | 'model' | 'executable' | 'system' | 'font' | 'other' | 'none';
export type EntryFlag =
  | 'sensitive_name' | 'protected_context' | 'hardlink_suspected' | 'cloud_placeholder'
  | 'reparse_not_followed' | 'excluded_state_dir' | 'access_denied' | 'enumeration_error'
  | 'identity_unavailable' | 'name_lossy';

export type ErrorCode =
  | 'invalid_request' | 'unsupported_protocol' | 'unknown_command' | 'not_found' | 'permission_denied'
  | 'capability_unavailable' | 'stale_generation' | 'device_offline' | 'partial_coverage'
  | 'resource_budget' | 'busy' | 'cancelled' | 'deadline_exceeded' | 'internal_error';

export interface ErrorBody { code: ErrorCode; message: string; retryable: boolean; detail: Record<string, unknown> | null }
export interface RequestEnvelope {
  protocol: 'loomward/3';
  request_id: string;
  command: string;
  payload: object;
  deadline_ms?: number;
}
export interface ResponseMeta { served_at: Timestamp; elapsed_ms: number; dataset_class: DatasetClass; budget_hit: boolean }
export type ResponseEnvelope =
  | { protocol: 'loomward/3'; request_id: string; ok: true; result: unknown; meta: ResponseMeta }
  | { protocol: 'loomward/3'; request_id: string; ok: false; error: ErrorBody };

export type EventName =
  | 'stream.hello' | 'stream.lagged' | 'job.state' | 'scan.progress' | 'tree.invalidated'
  | 'telemetry.sample' | 'learning.updated' | 'roots.changed' | 'volumes.changed' | 'health.warning';
export interface EventEnvelope { protocol: 'loomward/3'; seq: number; event: EventName; at: Timestamp; data: Record<string, unknown> }

export interface SessionInfo {
  protocol: 'loomward/3';
  engine_version: string;
  adapter: 'tauri' | 'http';
  dataset_class: DatasetClass;
  session_started_at: Timestamp;
  enumeration_strategy: string;
  capabilities: { observation: Record<string, boolean>; effects: Record<string, false> };
  features: Record<'grant_picker' | 'disclosure_dialog' | 'teacher_available' | 'telemetry_available' | 'gpu_available', boolean>;
  limits: { max_request_bytes: number; max_slice_nodes: number; max_page_items: number };
}

export interface DisplayPath { text: string; truncated: boolean }
export interface Root {
  root_id: RootId;
  display_path: DisplayPath;
  origin: 'fixture' | 'lab_generated' | 'owner_granted';
  dataset_class: DatasetClass;
  volume_id: string | null;
  granted_at: Timestamp;
  granted_via: 'desktop_picker' | 'cli_flag' | 'fixture';
  grant_state: 'active' | 'revoked' | 'identity_changed';
  scan: { state: 'never_scanned' | 'scanning' | 'complete' | 'partial' | 'stale' | 'failed'; finished_at: Timestamp | null; coverage: CoverageState };
  totals: { files: number; dirs: number; logical_bytes: ByteCount; allocated_bytes: NullableBytes; skipped: number; failed: number; complete: boolean } | null;
}
export interface RootList { roots: Root[] }

export type Grant =
  | { grant_id: string; kind: 'metadata_root'; root_id: RootId; granted_at: Timestamp; granted_via: 'desktop_picker' | 'cli_flag' | 'fixture'; revoked_at: Timestamp | null }
  | { grant_id: string; kind: 'teacher_disclosure'; recipient: string; dataset_class: DatasetClass; fields: string[]; item_count: number; payload_digest: string; created_at: Timestamp; expires_at: Timestamp; revoked_at: Timestamp | null; used: boolean; confirmed_via: 'desktop_dialog' | 'synthetic_policy' };
export interface GrantList { grants: Grant[] }

export interface Health {
  observed_at: Timestamp;
  engine: { private_commit_bytes: NullableBytes; working_set_bytes: NullableBytes; cpu_seconds: number | null; threads: number | null };
  catalog: { schema_version: number; db_bytes: NullableBytes; wal_bytes: NullableBytes; files: number; dirs: number; writer_queue_depth: number; writer_queue_capacity: number };
  jobs_running: number;
  last_error: ErrorBody | null;
  warnings: { code: string; message: string; at: Timestamp }[];
}

export interface Threads {
  meaning: { state: 'labelled' | 'suggested' | 'mixed' | 'none' | 'pending' | 'unknown'; label: string | null; share: number | null; source: null | 'human' | 'student' | 'teacher'; collection_ids: string[] };
  residency: { volume_id: string | null; tier: number | null; tier_basis: 'declared' | 'device_hint' | 'unknown' };
  permission: { state: 'granted' | 'partial' | 'excluded' | 'denied' | 'revoked' | 'unknown'; reason: string | null };
}
export interface SliceNode {
  node_id: NodeId;
  parent: number | null;
  kind: 'atlas' | 'volume' | 'root' | 'dir' | 'file' | 'other';
  name: string;
  depth: number;
  size_bytes: ByteCount;
  size_unknown_files: number;
  logical_bytes: ByteCount;
  allocated_bytes: NullableBytes;
  files: number;
  dirs: number;
  child_count: number | null;
  folded_count: number | null;
  coverage: CoverageState;
  live: boolean;
  ext_family: ExtFamily | null;
  modified_at: Timestamp | null;
  threads: Threads;
}
export interface TreeSlice {
  anchor_node_id: NodeId;
  basis: Basis;
  root_generations: unknown[];
  complete: boolean;
  live: boolean;
  ordering: 'exact' | 'approximate_live' | 'approximate_files';
  truncated: boolean;
  nodes: SliceNode[];
}
export interface EntryRow {
  node_id: NodeId;
  kind: 'dir' | 'file';
  name: string;
  extension: string | null;
  ext_family: ExtFamily | null;
  logical_bytes: ByteCount;
  allocated_bytes: NullableBytes;
  files: number | null;
  dirs: number | null;
  modified_at: Timestamp | null;
  attributes: string[];
  flags: EntryFlag[];
  coverage: CoverageState;
  location_hint: DisplayPath | null;
}
export interface EntryPage { anchor: string | null; generation: string | null; items: EntryRow[]; next_cursor: Cursor | null; total: number | null; budget_hit: boolean }

// Requests the shell and placeholder views send.
export type Anchor = { kind: 'atlas' } | { kind: 'root'; root_id: RootId } | { kind: 'node'; node_id: NodeId };
export interface TreeSliceRequest { anchor: Anchor; depth: number; max_nodes: number; min_share: number; basis: Basis; include_files: boolean }
export interface TreeChildrenRequest { node_id: NodeId; sort: 'size_desc' | 'name_asc' | 'modified_desc'; basis: Basis; limit: number; cursor: Cursor | null }
export interface SearchRequest { root_id: RootId | null; text: string; extension: string | null; min_bytes: NullableBytes; kind: 'any' | 'file' | 'dir'; limit: number; cursor: Cursor | null }

/** Command name -> [request, result] for the commands this lane calls. */
export interface CommandMap {
  'session.hello': [Record<string, never>, SessionInfo];
  'health.get': [Record<string, never>, Health];
  'roots.list': [Record<string, never>, RootList];
  'grants.list': [Record<string, never>, GrantList];
  'tree.slice': [TreeSliceRequest, TreeSlice];
  'tree.children': [TreeChildrenRequest, EntryPage];
  'search.query': [SearchRequest, EntryPage];
}
export type CommandName = keyof CommandMap;
