import { Client } from '../transport/client';
import { buildTransport, chooseTransport, tokenFromHash, type Choice } from '../transport/select';
import { TransportError, type Transport, type TransportMode } from '../transport/transport';
import type { DatasetClass, EventEnvelope, SessionInfo, StreamHello } from '../contracts.gen';

export type Connection = 'connecting' | 'connected' | 'unavailable';

const TOKEN_KEY = 'loomward.token';
const RETRY_MS = [1000, 2000, 4000, 8000, 15000];

function readStored(): string | null {
  try { return sessionStorage.getItem(TOKEN_KEY); } catch { return null; }
}

/**
 * Connection state for the whole shell. When it is anything but `connected` the shell renders
 * "unavailable" and views are unmounted, so no earlier data (and never demo data) stays on screen.
 */
class Session {
  state = $state<Connection>('connecting');
  mode = $state<TransportMode | 'none'>('none');
  info = $state<SessionInfo | null>(null);
  reason = $state('');
  /** Bumped whenever data may be stale (reconnect, `stream.lagged`, `tree.invalidated`, `roots.changed`). */
  epoch = $state(0);
  client: Client | null = null;

  private transport: Transport | null = null;
  private stopStream: (() => void) | null = null;
  private retryTimer: ReturnType<typeof setTimeout> | undefined;
  private attempt = 0;
  private busy = false;
  /** Epoch of the event stream (`stream.hello`), not to be confused with `epoch` above; a change means the engine restarted. */
  private streamEpoch: string | null = null;

  /** Dataset class of the connected session, or null when unknown. */
  get datasetClass(): DatasetClass | null {
    return this.state === 'connected' ? (this.info?.dataset_class ?? null) : null;
  }

  async start(): Promise<void> {
    // Read the token once, then scrub it from the address bar and history; keep it for reloads of this tab only.
    const token = tokenFromHash(location.hash);
    if (token) {
      try { sessionStorage.setItem(TOKEN_KEY, token); } catch { /* storage may be blocked */ }
      history.replaceState(null, '', location.pathname + location.search + '#/');
    }
    const choice: Choice = chooseTransport({
      search: location.search, hash: token ? `#token=${token}` : '', origin: location.origin,
      hasTauri: '__TAURI_INTERNALS__' in window, storedToken: readStored(),
    });
    if (choice.mode === 'none') return this.markUnavailable(choice.reason, false);
    this.mode = choice.mode;
    this.transport = await buildTransport(choice);
    this.client = new Client(this.transport);
    await this.connect();
  }

  /**
   * Views pass every caught error through here: a transport fault flips the whole shell to
   * "unavailable" (and retries); an engine error is just returned as display text.
   */
  handle(e: unknown): string {
    const message = e instanceof Error ? e.message : 'unexpected failure';
    if (e instanceof TransportError) this.markUnavailable(message, true);
    return message;
  }

  private markUnavailable(reason: string, retry: boolean): void {
    this.state = 'unavailable';
    this.info = null;
    this.reason = reason;
    clearTimeout(this.retryTimer);
    if (retry) this.retryTimer = setTimeout(() => void this.connect(), RETRY_MS[Math.min(this.attempt++, RETRY_MS.length - 1)]);
  }

  private async connect(): Promise<void> {
    if (!this.client || !this.transport || this.busy) return;
    this.busy = true;
    clearTimeout(this.retryTimer);
    this.state = 'connecting';
    try {
      const { result } = await this.client.call('session.hello', {});
      this.info = result;
      this.attempt = 0;
      this.reason = '';
      this.state = 'connected';
      this.epoch++;
    } catch (e) {
      this.markUnavailable(e instanceof Error ? e.message : 'engine not reachable', true);
      return;
    } finally {
      this.busy = false;
    }
    // One stream for the page's life; the transport reconnects it, and a reopen re-verifies the session.
    this.stopStream ??= this.transport.subscribe(
      (ev) => this.onEvent(ev),
      (s) => {
        if (s === 'closed') {
          if (this.state === 'connected') this.markUnavailable('event stream closed; reconnecting', false);
        } else if (this.state !== 'connected') void this.connect();
      },
    );
  }

  /** Resyncs (bumps `epoch`, so views refetch) on every signal that earlier data may be stale (semantics.md section 7). */
  private onEvent(ev: EventEnvelope): void {
    if (ev.event === 'stream.hello') {
      const next = (ev.data as unknown as StreamHello).epoch;
      if (typeof next !== 'string') return;
      if (this.streamEpoch !== null && this.streamEpoch !== next) this.epoch++; // restarted engine
      this.streamEpoch = next;
    } else if (ev.event === 'stream.lagged' || ev.event === 'tree.invalidated' || ev.event === 'roots.changed') {
      this.epoch++; // lagged: epoch_changed, replay_gap and subscriber_overflow all mean refetch
    }
  }
}

export const session = new Session();
