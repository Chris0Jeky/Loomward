import { formatTime } from '../format/time';
import { LoomwardError, TransportError } from '../transport/client';
import { session } from './session.svelte';

export type PendingForget = { kind: 'forget'; id: string; label: string; requestId?: string };
type Flight = { client: NonNullable<typeof session.client>; epoch: number; info: typeof session.info };
const sameIdentity = (f: Flight) => f.client === session.client && f.epoch === session.connectionEpoch && f.info === session.info;

/** The action key and its uncertain outcome belong to the shell lifetime, not the Health view. */
class Forget {
  pending = $state<PendingForget | null>(null);
  working = $state(false);
  unknown = $state(false);
  done = $state('');
  /** `done` holds the stopped-waiting notice: an unknown outcome, not a success (invariant 4). */
  stopped = $state(false);
  problem = $state('');
  private flight: Flight | null = null;
  private promptIdentity: Flight | null = null;
  private attempted = false;
  private key: { mintedAt: number; identity: Flight; expectedStateRev?: string } | null = null;

  /** Reconnects invalidate replies, but cannot establish whether a dispatched forget completed. */
  resume(): void {
    const f = this.flight;
    if (f && !sameIdentity(f)) {
      this.flight = null;
      this.working = false;
      this.unknown = true;
    }
    // An unused or definitely failed prompt is still tied to the root IDs it was shown for.
    // A dispatched request with an uncertain outcome retains its action key across identities.
    if (!this.working && !this.unknown && this.promptIdentity && !sameIdentity(this.promptIdentity)) this.cancel();
  }

  ask(pending: PendingForget): void {
    this.resume();
    const c = session.client;
    if (!c || this.working || this.unknown) return;
    this.pending = { ...pending };
    this.promptIdentity = { client: c, epoch: session.connectionEpoch, info: session.info };
    this.attempted = false;
    this.done = ''; this.problem = ''; this.stopped = false;
  }

  cancel(): void {
    if (this.working || this.unknown) return;
    this.pending = null;
    this.promptIdentity = null;
    this.attempted = false;
    this.dismissNotices();
  }

  /** Unused confirmations leave with the view; dispatched attempts keep their retry context. */
  dismiss(): void {
    if (!this.attempted) this.cancel();
    this.dismissNotices();
  }

  /** Leaving Health acknowledges visible notices, while pending work can still finish offscreen. */
  dismissNotices(): void {
    this.done = ''; this.problem = ''; this.stopped = false;
  }

  stopWaiting(): void {
    if (this.working || !this.unknown) return;
    this.pending = null;
    this.promptIdentity = null;
    this.attempted = false;
    this.unknown = false;
    this.problem = '';
    this.stopped = true;
    this.done = 'Stopped waiting. The forget outcome is unknown; it may already have completed. This does not cancel the request. No scanned file was touched.';
  }

  async confirm(): Promise<void> {
    this.resume();
    const c = session.client, p = this.pending;
    if (!c || !p || this.working) return;
    const f: Flight = { client: c, epoch: session.connectionEpoch, info: session.info };
    this.flight = f;
    this.promptIdentity = f;
    const current = () => { this.resume(); return this.flight === f; };
    this.working = true;
    this.attempted = true;
    this.problem = ''; this.done = ''; this.stopped = false;
    let checkingRecord = false;
    const readRecord = async () => {
      const { result, meta } = await c.call('grants.list', {});
      return { record: result.grants.find(g => g.kind === 'metadata_root' && g.root_id === p.id), meta };
    };
    try {
      let result;
      let expectedStateRev: string | undefined;
      if (p.requestId && (!this.key || Date.now() - this.key.mintedAt >= 600_000 || !sameIdentity(this.key.identity))) {
        checkingRecord = true;
        const { record, meta } = await readRecord();
        if (!current()) return;
        // Missing/bounded history or a later owner choice cannot authorise replay (semantics §3).
        if (!record || record.kind !== 'metadata_root') {
          this.problem = 'Loomward cannot tell whether this folder was forgotten: its record is no longer in the grant history. Nothing was sent again. Check Grants & health before trying again.';
          return;
        }
        if (record.forgotten_at) result = { root_id: p.id, forgotten_at: record.forgotten_at };
        else {
          if (!record.revoked_at || meta.state_rev == null) {
            this.problem = "This folder's access changed after the forget was sent, so it was not sent again. Review the folder before forgetting it.";
            return;
          }
          expectedStateRev = meta.state_rev;
          p.requestId = undefined;
        }
      }
      if (!result) {
        checkingRecord = false;
        if (!p.requestId) {
          p.requestId = c.nextRequestId();
          this.key = { mintedAt: Date.now(), identity: f, expectedStateRev };
        }
        try {
          ({ result } = await c.call('roots.forget', { root_id: p.id }, {
            deadlineMs: 60000, requestId: p.requestId,
            ...(this.key?.expectedStateRev !== undefined ? { expectedStateRev: this.key.expectedStateRev } : {}),
          }));
        } catch (e) {
          if (!current()) return;
          if (!this.unknown || !(e instanceof LoomwardError) || e.code !== 'not_found') throw e;
          checkingRecord = true;
          const { record } = await readRecord();
          if (!current()) return;
          if (!record || record.kind !== 'metadata_root' || !record.forgotten_at) throw e;
          result = { root_id: p.id, forgotten_at: record.forgotten_at };
        }
      }
      if (!current()) return;
      this.done = `Forgotten at ${formatTime(result.forgotten_at)}. ${checkingRecord ? "Its name and catalogue were removed from Loomward's database. Backup cleanup is unverified after the service receipt was lost." : "Its name and catalogue were removed from Loomward's database and its own backup copies; traces may remain in unused disk space until it is overwritten."} No scanned file was touched.`;
      this.pending = null;
      this.promptIdentity = null;
      this.attempted = false;
      this.unknown = false;
    } catch (e) {
      if (!current()) return;
      this.unknown = checkingRecord || e instanceof TransportError || e instanceof LoomwardError && e.code === 'deadline_exceeded';
      if (e instanceof TransportError) session.handle(e);
      if (!this.unknown) {
        // Definite answers are cached: a new attempt needs a new key (#736). A send bound to a state
        // read keeps its key, expired, so the next attempt reads state again first (semantics §3).
        if (this.key?.expectedStateRev !== undefined) this.key.mintedAt = -Infinity;
        else p.requestId = undefined;
        this.problem = session.handle(e);
      }
    } finally {
      if (current()) { this.flight = null; this.working = false; }
    }
  }
}

export const forget = new Forget();
