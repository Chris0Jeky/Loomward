import { LoomwardError, TransportError, type Client } from '../../lib/transport/client';
import { formatCount } from '../../lib/format/time';
import { SAVE_REFUSAL_WORDS } from './diagnostics';

export type SaveReport = { kind: 'notice' | 'problem'; text: string };

/** `sha256:` and the first 12 hex digits: enough to match the digest shown above the preview. */
export const shortDigest = (digest: string) => `${digest.slice(0, 19)}…`;

/**
 * One diagnostic save at a time, and never a second one while the first's outcome is unknown.
 *
 * The save keeps its request id until the service answers with an outcome. A save whose dialog outlived the wait
 * (`deadline_exceeded`) or whose answer was lost (a transport fault) stays pending: the next click sends the same
 * request id, so the service waits for or replays that one save instead of opening a second dialog. While a save
 * is running or pending, `frozen` is true and the view defers reloading the preview, so the text shown stays the
 * text being saved.
 */
export class SaveTracker {
  /** The outstanding save: the digest it writes and the request id that names it. */
  pending = $state<{ digest: string; requestId: string } | null>(null);
  running = $state(false);
  readonly frozen = $derived(this.running || this.pending !== null);

  async save(client: Client, digest: string, handle: (e: unknown) => string): Promise<SaveReport | null> {
    if (this.running) return null;
    const action = this.pending ?? { digest, requestId: client.nextRequestId() };
    this.pending = action;
    this.running = true;
    try {
      // Rust opens the save dialog and keeps the path; only the digest of what is shown goes out.
      const { result } = await client.call('diagnostics.save', { digest: action.digest }, { deadlineMs: 60000, requestId: action.requestId });
      this.pending = null;
      if (result.outcome === 'saved') {
        return { kind: 'notice', text: `Saved ${formatCount(Number(result.bytes_written))} bytes with digest ${shortDigest(action.digest)}, exactly the text shown. Nothing was uploaded.` };
      }
      if (result.outcome === 'cancelled_by_user') return { kind: 'notice', text: 'Save cancelled. No file was written.' };
      return { kind: 'problem', text: result.refusal ? SAVE_REFUSAL_WORDS[result.refusal] : 'The save was refused. No file was written.' };
    } catch (e) {
      if ((e instanceof LoomwardError && e.code === 'deadline_exceeded') || e instanceof TransportError) {
        return { kind: 'problem', text: 'The save is still pending in the dialog, or its answer was lost. Save… asks for that same save again; it never starts a second one.' };
      }
      this.pending = null;
      return { kind: 'problem', text: handle(e) };
    } finally {
      this.running = false;
    }
  }
}
