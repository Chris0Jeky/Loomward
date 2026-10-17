import type { Client } from '../../lib/transport/client';
import type { DiagnosticsPreviewResult } from '../../lib/contracts.gen';
import { canonicalText, matches } from './diagnostics';
import { SaveTracker } from './save.svelte';

type Handle = (e: unknown) => string;

/**
 * The diagnostic panel's state, kept outside the component so leaving Grants & health while a save dialog is open
 * loses nothing (#437): the outstanding save, its outcome and the frozen preview are still here on return.
 *
 * Reloads are coalesced: one preview call runs at a time and any number of asks while it runs (every scan
 * invalidation is one) cost one more call, never one each. A save that is running or pending also defers the reload,
 * so the text shown stays the text being saved.
 */
export class DiagnosticsPanel {
  preview = $state<DiagnosticsPreviewResult | null>(null);
  text = $state('');
  /** null while checking; false when the shown text is not exactly the bytes the service would write. */
  verified = $state<boolean | null>(null);
  loading = $state(false);
  notice = $state('');
  problem = $state('');
  readonly saves = new SaveTracker();
  /** Whether the panel has shown the current notice or problem: one nobody saw survives the next reload. */
  seen = false;
  private running = false;
  private dirty = false;

  async refresh(client: Client | null, handle: Handle): Promise<void> {
    if (!client) return;
    if (this.running || this.saves.frozen) { this.dirty = true; return; }
    this.running = true;
    this.loading = true;
    try {
      do {
        this.dirty = false;
        await this.loadOnce(client, handle);
      } while (this.dirty && !this.saves.frozen);
    } finally { this.running = false; this.loading = false; }
  }

  private async loadOnce(client: Client, handle: Handle): Promise<void> {
    if (this.seen) this.problem = '';
    this.verified = null;
    try {
      const { result } = await client.call('diagnostics.preview', {});
      const shown = canonicalText(result.bundle);
      const same = await matches(shown, result);
      if (this.dirty || this.saves.frozen) { this.dirty = true; return; } // older than the next ask: never shown
      if (this.seen && this.preview && result.digest !== this.preview.digest) { this.notice = ''; this.seen = false; } // that notice described other text
      this.preview = result; this.text = shown; this.verified = same;
    } catch (e) {
      if (!this.dirty) { this.problem = handle(e); this.seen = false; this.preview = null; }
    }
  }

  async save(client: Client | null, handle: Handle): Promise<void> {
    if (!client || !this.preview || !this.verified || this.saves.running) return;
    this.notice = ''; this.problem = '';
    const report = await this.saves.save(client, this.preview.digest, handle);
    if (report?.kind === 'notice') this.notice = report.text;
    else if (report) this.problem = report.text;
    this.seen = false;
    if (!this.saves.frozen && this.dirty) void this.refresh(client, handle);
  }
}

export const diagnosticsPanel = new DiagnosticsPanel();
