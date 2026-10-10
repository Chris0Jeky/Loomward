// Names the generated contract inlines instead of exporting; derived from it, never restated.
import type { GrantList, PlacementPlan, PlacementSimulateRequest, ProcessListRequest } from './contracts.gen';

export type HeatPolicy = PlacementSimulateRequest['heat_policy'];
export type ReliefPolicy = PlacementSimulateRequest['relief_policy'];
export type RejectReason = PlacementPlan['rejected'][number]['reason'];
export type PreRejectReason = PlacementPlan['pre_rejected'][number]['reason'];
export type ProcessSort = ProcessListRequest['sort'];
export type Grant = GrantList['grants'][number];
