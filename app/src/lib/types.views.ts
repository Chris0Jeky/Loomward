// TEMPORARY, hand-written subset of contracts/v3/view-service.schema.json (PR #110 head) for the
// Tiers, Companion and Grants views (lane L13). Lane L1 generates src/lib/contracts.gen.ts; when it
// lands, delete this file and import the generated names (they are the schema's own `$defs` names).
// The commands are added to `CommandMap` by interface merging so types.ts stays untouched.
import type { ByteCount, CoverageState, NullableBytes, NodeId, RootId, Timestamp } from './types';

export type VolumeId = string;
export type GroupId = string;
export type ProcessRef = string;
export type Tier = number | null;
export type Fraction = number | null;

export interface TierInfo { tier: Tier; basis: 'declared' | 'device_hint' | 'unknown'; declared_tier: Tier; hint_tier: Tier; note: string }
export interface DeviceHint {
  bus_type: 'nvme' | 'sata' | 'sas' | 'scsi' | 'usb' | 'raid' | 'virtual' | 'sd' | 'other' | 'unknown';
  seek_penalty: boolean | null;
  multiple_disks: boolean | null;
  basis: 'ioctl_storage_query_property' | 'unavailable';
}
export interface Volume {
  volume_id: VolumeId;
  display_name: string;
  mount_points: string[];
  filesystem: string | null;
  label: string | null;
  online: boolean;
  read_only: boolean | null;
  removable: boolean | null;
  capacity_bytes: NullableBytes;
  free_bytes: NullableBytes;
  device: DeviceHint;
  tier: TierInfo;
  observed_at: Timestamp;
}
export interface VolumeList { volumes: Volume[] }

export interface TierVolume {
  volume_id: VolumeId;
  display_name: string;
  tier: TierInfo;
  online: boolean;
  writable: boolean | null;
  capacity_bytes: NullableBytes;
  free_bytes: NullableBytes;
  reserve_bytes: ByteCount;
  free_fraction: Fraction;
  pressure: 'ok' | 'watch' | 'pressure' | 'unknown';
}
export interface TierModel {
  volumes: TierVolume[];
  policy: { watch_free_fraction: number; pressure_free_fraction: number; reserve_note: string; note: string };
}

export type HeatBasis = 'unknown' | 'assumed' | 'mtime_proxy';
export type CandidateBasis = 'root_children' | 'largest_dirs';
export type HeatPolicy = 'unknown_is_ineligible' | 'mtime_proxy_whatif' | 'assumed_only';
export type ReliefPolicy = 'verified_only' | 'entry_allocation_whatif';

export interface PlacementCandidatesRequest { source_volume_id: VolumeId; basis: CandidateBasis; max_groups: number; min_bytes: ByteCount }
export interface CandidateGroup {
  group_id: GroupId;
  node_id: NodeId;
  root_id: RootId;
  name: string;
  source_bytes: ByteCount;
  destination_bytes: ByteCount;
  transfer_bytes: ByteCount;
  estimate_basis: 'allocated_entries' | 'logical_fallback' | 'unknown';
  estimated_relief_bytes: NullableBytes;
  relief_basis: 'verified_unique_allocation' | 'entry_allocation_whatif' | 'unknown';
  heat: Fraction;
  heat_basis: HeatBasis;
  newest_modified_at: Timestamp | null;
  pinned: boolean | null;
  active: boolean | null;
  protected: boolean | null;
  days_since_move: number | null;
  coverage: CoverageState;
}
export interface PlacementCandidates { source_volume_id: VolumeId; root_generations: unknown[]; groups: CandidateGroup[]; note: string }

export interface GroupOverride { group_id: GroupId; heat?: Fraction; pinned?: boolean | null; active?: boolean | null; protected?: boolean | null; days_since_move?: number | null }
export interface PlacementSimulateRequest {
  source_volume_id: VolumeId;
  target_free_bytes: ByteCount;
  max_transfer_bytes: ByteCount;
  heat_policy: HeatPolicy;
  relief_policy: ReliefPolicy;
  candidate_basis: CandidateBasis;
  max_groups: number;
  overrides: GroupOverride[];
  node_budget: number;
  save: boolean;
}
export interface PlacementProposal {
  group_id: GroupId;
  source_id: VolumeId;
  target_id: VolumeId;
  source_bytes_relieved: ByteCount;
  destination_bytes_required: ByteCount;
  transfer_bytes: ByteCount;
  reason: 'bounded_capacity_budget_allocation';
  requires_consent: true;
  executable: false;
}
export type RejectReason =
  | 'not_on_source' | 'pinned_active_protected_or_unspecified' | 'heat_unknown' | 'not_cold'
  | 'cooldown_or_history_unknown' | 'source_offline_or_readonly' | 'not_selected_by_bounded_allocator';
export type PreRejectReason = 'relief_unknown' | 'shares_objects_with_other_group' | 'group_coverage_incomplete';
export interface PlacementPlan {
  mode: 'simulation';
  algorithm: 'bounded_portfolio_search_v2';
  optimality_claim: boolean;
  optimality_scope: string;
  shortfall_optimal: boolean;
  search: {
    complete: boolean; nodes_visited: number; node_budget: number; reason: 'exhausted' | 'node_budget' | 'problem_size_limit';
    eligible_groups: number; eligible_targets: number; lower_bound_shortfall_bytes: ByteCount;
  };
  proposals: PlacementProposal[];
  rejected: { group_id: GroupId; reason: RejectReason }[];
  pre_rejected: { group_id: GroupId; reason: PreRejectReason }[];
  excluded_volumes: { volume_id: VolumeId; reason: 'tier_unknown' | 'capacity_unknown' | 'offline' }[];
  relief_policy: ReliefPolicy;
  projected_free_bytes: Record<VolumeId, ByteCount>;
  target_free_bytes: ByteCount;
  shortfall_bytes: ByteCount;
  satisfied: boolean;
  transfer_bytes: ByteCount;
  filesystem_changed: false;
  baseline_shortfall_bytes: ByteCount;
  shortfall_improvement_bytes: ByteCount;
  assumption: string;
  heat_policy: HeatPolicy;
  assumptions: string[];
  root_generations: unknown[];
  proposal_id: string | null;
}

export interface SystemSample {
  memory: { total_bytes: NullableBytes; available_bytes: NullableBytes; commit_bytes: NullableBytes; commit_limit_bytes: NullableBytes; load_fraction: Fraction };
  cpu: { logical_cpus: number; busy_fraction: Fraction };
}
export type PoolName = 'scan_enumerate' | 'catalog_write' | 'learning' | 'teacher' | 'telemetry';
export interface PoolUse { pool: PoolName; max_workers: number; busy_workers: number; queue_depth: number; queue_capacity: number }
export interface EngineSample { private_commit_bytes: NullableBytes; working_set_bytes: NullableBytes; cpu_fraction: Fraction; threads: number | null; pools: PoolUse[] }
export type TelemetryChannel = 'system' | 'processes' | 'gpu' | 'disks' | 'engine';
export interface TelemetrySnapshotRequest { channels: TelemetryChannel[] }
export type NullableRate = number | null;
/** Read by the Observatory gauges (lane L10). */
export interface GpuSample {
  state: 'observed' | 'unavailable' | 'denied';
  basis: 'pdh_gpu_counters' | 'unavailable';
  adapters: { adapter_id: string; name: string; dedicated_total_bytes: NullableBytes; dedicated_used_bytes: NullableBytes; shared_used_bytes: NullableBytes; engine_busy_fraction: Fraction }[];
}
export interface DiskSample {
  state: 'observed' | 'unavailable';
  disks: { disk_label: string; volume_ids: string[]; read_bytes_per_s: NullableRate; write_bytes_per_s: NullableRate; busy_fraction: Fraction; queue_length: NullableRate }[];
}
/** The channels the views read are typed; processes stays `unknown` here (processes.list serves rows). */
export interface TelemetrySample {
  sample_seq: number; observed_at: Timestamp; elapsed_ms: number | null;
  system: SystemSample | null; gpu: GpuSample | null; disks: DiskSample | null; engine: EngineSample | null; processes: unknown;
}
export interface TelemetrySubscribeRequest { subscription_id: string | null; channels: TelemetryChannel[]; interval_ms: 1000 | 2000 | 5000 | 10000 }
export interface TelemetrySubscription { subscription_id: string; channels: TelemetryChannel[]; interval_ms: number; expires_at: Timestamp }
export interface SubscriptionRefRequest { subscription_id: string }
export interface TelemetryUnsubscribed { subscription_id: string; stopped: boolean }

export interface ProcessRow {
  process_ref: ProcessRef;
  pid: number;
  name: string;
  started_at: Timestamp | null;
  private_commit_bytes: NullableBytes;
  private_working_set_bytes: NullableBytes;
  working_set_bytes: NullableBytes;
  cpu_fraction: Fraction;
  io_read_bytes_per_s: number | null;
  io_write_bytes_per_s: number | null;
  gpu_dedicated_bytes: NullableBytes;
  access: 'full' | 'limited' | 'denied';
  loomward_owned: boolean;
}
export type ProcessSort = 'private_desc' | 'working_set_desc' | 'cpu_desc' | 'io_desc' | 'gpu_desc' | 'name_asc';
export interface ProcessListRequest { sort: ProcessSort; limit: number }
export interface ProcessList { sample_seq: number; observed_at: Timestamp; rows: ProcessRow[]; observed_count: number; denied_count: number; truncated: boolean; note: string }
export interface ProcessRefRequest { process_ref: ProcessRef }
export interface ProcessExplanation {
  process_ref: ProcessRef; observed_at: Timestamp;
  facts: { code: string; text: string }[];
  summary: string; caveats: string[];
  /** Always empty in v0.3 (invariant 1). */
  available_actions: never[];
}
export interface PoolBudget { pool: PoolName; max_workers: number; default_workers: number; current_workers: number; queue_capacity: number; scope: 'loomward_own_threads' }
export interface OwnBudgets {
  pools: PoolBudget[];
  teacher: { max_in_flight: 1; timeout_s: number; memory_limit_bytes: NullableBytes; enforcement: 'job_object' | 'not_available' };
  enforcement_note: string;
}

export interface RootRevokeRequest { root_id: RootId; purge_catalog: boolean }
export interface RootRevokeResult { root_id: RootId; revoked_at: Timestamp; purged: boolean }
export interface GrantRevokeRequest { grant_id: string }
export interface GrantRevokeResult { grant_id: string; revoked_at: Timestamp }

declare module './types' {
  interface CommandMap {
    'volumes.list': [Record<string, never>, VolumeList];
    'tiers.model': [Record<string, never>, TierModel];
    'placement.candidates': [PlacementCandidatesRequest, PlacementCandidates];
    'placement.simulate': [PlacementSimulateRequest, PlacementPlan];
    'telemetry.subscribe': [TelemetrySubscribeRequest, TelemetrySubscription];
    'telemetry.unsubscribe': [SubscriptionRefRequest, TelemetryUnsubscribed];
    'telemetry.snapshot': [TelemetrySnapshotRequest, TelemetrySample];
    'processes.list': [ProcessListRequest, ProcessList];
    'processes.explain': [ProcessRefRequest, ProcessExplanation];
    'budgets.get': [Record<string, never>, OwnBudgets];
    'roots.revoke': [RootRevokeRequest, RootRevokeResult];
    'grants.revoke': [GrantRevokeRequest, GrantRevokeResult];
  }
}
