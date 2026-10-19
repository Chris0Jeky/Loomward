import { formatTime } from '../format/time';
import { LoomwardError, TransportError } from '../transport/client';
import { session } from './session.svelte';

export type PendingForget = { kind: 'forget'; id: string; label: string; requestId?: string };
type Flight = { client: NonNullable<typeof session.client>; epoch: number; info: typeof session.info };

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

  /** Reconnects invalidate replies, but cannot establish whether a dispatched forget completed. */
  resume(): void {
    const f = this.flight;
    if (f && (f.client !== session.client || f.epoch !== session.connectionEpoch || f.info !== session.info)) {
      this.flight = null;
      this.working = false;
      this.unknown = true;
    }
  }

  ask(pending: PendingForget): void {
    this.resume();
    if (this.working || this.unknown) return;
    this.pending = { ...pending };
    this.done = ''; this.problem = ''; this.stopped = false;
  }

  cancel(): void {
    if (!this.working && !this.unknown) this.pending = null;
  }

  stopWaiting(): void {
    if (this.working || !this.unknown) return;
    this.pending = null;
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
    const current = () => { this.resume(); return this.flight === f; };
    this.working = true;
    this.problem = ''; this.done = ''; this.stopped = false;
    p.requestId ??= c.nextRequestId();
    let checkingRecord = false;
    try {
      let result;
      try {
        ({ result } = await c.call('roots.forget', { root_id: p.id }, { deadlineMs: 60000, requestId: p.requestId }));
      } catch (e) {
        if (!current()) return;
        if (!this.unknown || !(e instanceof LoomwardError) || e.code !== 'not_found') throw e;
        checkingRecord = true;
        const { result: records } = await c.call('grants.list', {});
        if (!current()) return;
        const record = records.grants.find(g => g.kind === 'metadata_root' && g.root_id === p.id);
        if (!record || record.kind !== 'metadata_root' || !record.forgotten_at) throw e;
        result = { root_id: p.id, forgotten_at: record.forgotten_at };
      }
      if (!current()) return;
      this.done = `Forgotten at ${formatTime(result.forgotten_at)}. ${checkingRecord ? "Its name and catalogue were removed from Loomward's database. Backup cleanup is unverified after the service receipt was lost." : "Its name and catalogue were removed from Loomward's database and its own backup copies; traces may remain in unused disk space until it is overwritten."} No scanned file was touched.`;
      this.pending = null;
      this.unknown = false;
    } catch (e) {
      if (!current()) return;
      this.unknown = checkingRecord || e instanceof TransportError || e instanceof LoomwardError && e.code === 'deadline_exceeded';
      if (!this.unknown) {
        // Definite answers are cached: a new attempt needs a new key (#736).
        p.requestId = undefined;
        this.problem = session.handle(e);
      }
    } finally {
      if (current()) { this.flight = null; this.working = false; }
    }
  }
}

export const forget = new Forget();
