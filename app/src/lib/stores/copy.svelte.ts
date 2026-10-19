import type { CopyApproval, CopyPreview, CopyRefusalCode, CopyStatus, NodeId } from '../contracts.gen';
import { Client, LoomwardError } from '../transport/client';
import { session } from './session.svelte';

export interface CopyItem { node_id: NodeId; name: string }

const REASONS: Record<CopyRefusalCode, string> = {
  identity_unverified: 'The source or destination has no verified NTFS or ReFS identity.',
  cloud_placeholder: 'This item would need a cloud download. Loomward will not recall it.',
  reparse_point: 'A link, junction or other redirected entry is inside the selection.',
  unsupported_attribute: 'System, temporary, compressed, sparse, encrypted or integrity-stream items are not supported.',
  extended_attributes: 'Extended attributes cannot yet be preserved and verified.',
  alternate_streams_unsupported: 'Named data streams cannot be preserved on this destination.',
  too_many_streams: 'An item exceeds the limit of 64 named data streams.',
  special_file: 'This item is not an ordinary disk file.',
  source_changed: 'The source changed. Abandon this operation and make a new preview.',
  staging_name_taken: 'An entry Loomward did not record occupies a staging name. It is left untouched; remove it outside Loomward. A run that needs attention can then resume; a refused one needs a new preview.',
  staging_in_use: 'Another program has a staged file open for writing. Close it before resuming.',
  changed_after_verification: 'The published copy changed after verification. It is kept, but is not verified.',
  source_in_use: 'Another program is writing to the source. Close it before resuming.',
  access_denied: 'The source cannot be read or the destination cannot be written. Check access before resuming.',
  insufficient_space: 'The destination lacks the required space and reserve. Make room before resuming.',
  destination_inside_source: 'The destination parent is inside a selected source folder.',
  destination_cloud_synced: 'The destination is cloud synced; this operation requires a local destination.',
  destination_unsupported_volume: 'The destination is removable, networked, or not on a supported NTFS or ReFS volume.',
  destination_protected: 'The destination is protected or belongs to Loomward’s state.',
  destination_changed: 'The destination folder was moved, replaced or removed after approval. Nothing was written. Choose the destination again and make a new preview.',
  case_collision: 'Two source names collide under the destination’s case rules.',
  name_unrepresentable: 'A source name cannot be represented safely at the destination.',
  path_too_long: 'A final destination path exceeds the supported path length.',
  publish_in_use: 'Another program is holding the staged copy open. Close it before resuming.',
  verification_mismatch: 'Bytes or metadata failed verification twice. The copy is not verified.',
  grant_revoked: 'Permission was revoked or emergency stop was requested. A new grant needs a new preview.',
  elevated: 'Copying is refused while Loomward is running with elevated privileges.',
  approval_expired: 'Approval expired. Make a new preview and confirm it again.',
  digest_mismatch: 'The manifest no longer matches the approved digest. Make a new preview.',
  manifest_limit: 'The selection exceeds the limit of 200,000 entries. Select fewer items.',
};
export const refusalReason = (code: CopyRefusalCode): string => REASONS[code] ?? 'The engine refused this operation. No verified result is available.';
const EDITABLE = new Set(['draft', 'approved', 'committed', 'cancelled', 'refused', 'expired']);
const RESUMABLE = new Set<CopyRefusalCode>(['staging_name_taken', 'staging_in_use', 'source_in_use', 'access_denied', 'insufficient_space', 'publish_in_use']);

/** UI consent only: native identities, authority and every effect remain the engine's responsibility. */
export class CopyFlow {
  items = $state<CopyItem[]>([]);
  parent = $state<CopyItem | null>(null);
  preview = $state<CopyPreview | null>(null);
  approval = $state<CopyApproval | null>(null);
  status = $state<CopyStatus | null>(null);
  busy = $state(false);
  problem = $state('');
  notice = $state('');
  gated = $state(false);
  error = $state<unknown>(null);
  uncertain = $state(false);
  private missing = $state(false);
  private engineRestarts = 0;
  private generation = 0;
  private action = '';
  private epoch: number | null = null;

  get locked(): boolean { return this.uncertain || (!!this.status && !EDITABLE.has(this.status.state)); }
  get canApprove(): boolean {
    if (this.busy || this.uncertain || this.gated || this.approval || !this.preview || this.preview.refusal_count !== 0 || this.preview.refusals.length) return false;
    if (this.status && !['draft', 'approved'].includes(this.status.state)) return ['blocked', 'needs_attention'].includes(this.status.state) && !!this.status.refusal && RESUMABLE.has(this.status.refusal.code);
    return this.preview.state === 'draft';
  }
  get canStart(): boolean {
    return !this.busy && !!this.approval && !!this.preview && this.approval.digest === this.preview.digest
      && this.approval.operation_id === this.preview.operation_id && Date.parse(this.approval.expires_at) > Date.now();
  }
  get canCancel(): boolean { return !this.uncertain && !!this.status && ['approved', 'preparing', 'copying', 'staged', 'blocked'].includes(this.status.state); }
  get canAbandon(): boolean {
    return !this.busy && !!this.preview && (this.missing || (!this.uncertain && (!this.status || ['draft', 'approved', 'refused', 'expired', 'blocked', 'needs_attention'].includes(this.status.state))));
  }

  private invalidate(): void {
    if (this.preview || this.approval || this.busy) this.notice = 'Approval invalidated: the selection or destination changed. Build a new preview.';
    this.generation++; this.preview = null; this.approval = null; this.status = null;
    this.problem = ''; this.error = null; this.gated = false;
    this.missing = false;
  }
  select(items: CopyItem[]): boolean {
    if (this.locked || (this.busy && this.action !== 'preview')) return false;
    this.invalidate();
    this.items = [...new Map(items.map(item => [item.node_id, item])).values()];
    return true;
  }
  destination(parent: CopyItem): boolean {
    if (this.locked || (this.busy && this.action !== 'preview')) return false;
    this.invalidate(); this.parent = parent; return true;
  }
  invalidateObservations(): void {
    this.generation++;
    this.approval = null;
    if (this.locked) this.notice = 'Observations changed. Refresh status; nothing resumes automatically.';
    else { this.invalidate(); this.notice = 'Observations changed. Approval invalidated; build a new preview.'; }
  }
  observe(epoch: number): void {
    if (this.epoch !== null && this.epoch !== epoch) this.invalidateObservations();
    this.epoch = epoch;
  }
  /** `stale` tells whether the observation this call answers has changed; such a failure is dropped with its text. */
  private async run(action: string, task: () => Promise<void>, stale: () => boolean = () => false): Promise<void> {
    if (this.busy) return;
    this.busy = true; this.action = action; this.problem = ''; this.error = null; this.gated = false;
    try { await task(); }
    catch (error) {
      if (stale()) return;
      this.error = error;
      this.problem = error instanceof Error ? error.message : 'Unexpected copy response.';
      if (error instanceof LoomwardError) {
        this.problem = `${error.code}: ${this.problem}${error.detail?.reason ? ` (${String(error.detail.reason)})` : ''}`;
        this.gated = error.code === 'capability_unavailable' || error.detail?.reason === 'copy_effect_gated';
        if (this.gated) this.uncertain = false;
      }
    } finally { this.busy = false; this.action = ''; }
  }
  async review(client: Client): Promise<void> {
    if (this.locked || !this.items.length || !this.parent) return;
    await this.run('preview', async () => {
      this.approval = null; this.preview = null; this.status = null;
      const generation = ++this.generation;
      const engineRestarts = session.engineRestarts;
      const { result } = await client.call('copy.preview', { sources: this.items.map(i => i.node_id), destination_node_id: this.parent!.node_id });
      if (generation === this.generation) { this.preview = result; this.engineRestarts = engineRestarts; this.notice = 'Preview ready. Review every refusal before approving.'; }
    });
  }
  async approve(client: Client): Promise<void> {
    if (!this.canApprove) return;
    await this.run('approve', async () => {
      this.approval = null;
      const p = this.preview!;
      const generation = this.generation;
      const { result } = await client.call('copy.approve', { operation_id: p.operation_id, digest: p.digest });
      if (generation !== this.generation) return;
      if (result.operation_id !== p.operation_id || result.digest !== p.digest || !(Date.parse(result.expires_at) > Date.now())) throw new Error('Approval does not match this preview or has expired. Preview and approve again.');
      this.approval = result; this.notice = 'Approved for this digest only. Start copying when ready; approval is single use.';
    });
  }
  private accept(status: CopyStatus): void {
    if (status.operation_id !== this.preview?.operation_id || status.digest !== this.preview.digest) throw new Error('Status does not match this operation and preview. Outcome unknown; refresh status.');
    this.status = status; this.uncertain = false; this.approval = null; this.missing = false;
    this.notice = `Engine reports ${status.state.replaceAll('_', ' ')}. Nothing resumes automatically.`;
  }
  async start(client: Client): Promise<void> {
    if (!this.canStart) return;
    const generation = this.generation;
    await this.run('start', async () => {
      const p = this.preview!;
      this.approval = null; this.uncertain = true;
      const { result } = await client.call('copy.start', { operation_id: p.operation_id, digest: p.digest });
      if (generation === this.generation) this.accept(result);
    }, () => generation !== this.generation);
  }
  async refresh(client: Client): Promise<void> {
    if (!this.preview) return;
    const generation = this.generation;
    const restarted = this.engineRestarts !== session.engineRestarts;
    await this.run('status', async () => {
      const { result } = await client.call('copy.status', { operation_id: this.preview!.operation_id });
      if (generation === this.generation) this.accept(result);
    }, () => generation !== this.generation);
    if (generation === this.generation && restarted && this.uncertain && this.error instanceof LoomwardError && this.error.code === 'not_found') {
      this.missing = true; this.status = null; this.approval = null;
      this.notice = 'The restarted engine has no record of this operation. Its outcome is not verified. Abandon operation to reset this selection locally; no files will be changed.';
    }
  }
  async cancel(client: Client): Promise<void> { if (this.canCancel) await this.stop(client, 'copy.cancel'); }
  async abandon(client: Client): Promise<void> {
    if (!this.canAbandon) return;
    if (this.missing) {
      this.uncertain = false; this.invalidate(); this.items = []; this.parent = null;
      this.notice = 'Selection reset locally. The missing operation was not verified; no files were changed by this reset. Select again and build a new preview.';
    } else await this.stop(client, 'copy.abandon');
  }
  private async stop(client: Client, command: 'copy.cancel' | 'copy.abandon'): Promise<void> {
    if (this.busy) return;
    const generation = this.generation;
    await this.run('stop', async () => {
      this.approval = null; this.uncertain = true;
      const { result } = await client.call(command, { operation_id: this.preview!.operation_id });
      if (generation === this.generation) this.accept(result);
    }, () => generation !== this.generation);
    // A refused stop does not confirm the last status, so it is cleared rather than shown as current (as a refused start is).
    if (this.gated) {
      this.status = null;
      this.notice = 'The engine refused this stop. The last status it reported is no longer shown; refresh status to see the current state.';
    }
  }
}

// Kept for view navigation only; reload never silently resumes an operation.
export const copy = new CopyFlow();
