/**
 * The opt-in update check (HUMAN_TODO q-30, q-31, q-33) over the desktop's `lw_update` command
 * (`native/src/updates.rs`). Nothing here reaches the network itself: the desktop does, and only
 * for a click on "Check" or a start the owner opted into. Loading, toggling and switching channel
 * never check. Installing asks again in a native box the page cannot answer.
 */
export type Channel = 'preview' | 'nightly';
export interface Prefs { check_on_start: boolean; channel: Channel }
export interface Offer { version: string; channel: Channel; notes: string; date: string | null; previous: boolean }
export interface UpdateStatus {
  current_version: string;
  /** Why updates cannot run in this build or mode, or null. */
  unavailable: string | null;
  prefs: Prefs;
  offer: Offer | null;
  last_error: string | null;
}
export type Invoke = (cmd: string, args: Record<string, unknown>) => Promise<unknown>;
export type Busy = '' | 'loading' | 'saving' | 'checking' | 'installing';

const reason = (e: unknown) => (typeof e === 'string' ? e : e instanceof Error ? e.message : 'the desktop did not answer');

export class Updates {
  status = $state<UpdateStatus | null>(null);
  busy = $state<Busy>('');
  /** What the status region says. */
  message = $state('');
  /** False outside the desktop app (browser or mock): there is no updater to ask. */
  desktop = $state(false);

  constructor(private invoke: Invoke | null = null) {
    this.desktop = invoke !== null;
  }

  /** Binds to the desktop's IPC; `null` in a browser. */
  use(invoke: Invoke | null): void {
    this.invoke = invoke;
    this.desktop = invoke !== null;
    this.status = null;
    this.message = '';
  }

  private async send(busy: Busy, request: Record<string, unknown>): Promise<unknown> {
    if (!this.invoke || this.busy) return null;
    this.busy = busy;
    try {
      return await this.invoke('lw_update', { request });
    } finally {
      this.busy = '';
    }
  }

  async load(): Promise<void> {
    try {
      const s = (await this.send('loading', { action: 'status' })) as UpdateStatus | null;
      if (!s) return;
      this.status = s;
      // A failed check on start must not read as "no check happened".
      if (s.last_error) this.message = `The last check failed: ${s.last_error}`;
      else if (s.offer) this.message = `${s.offer.previous ? 'Previous nightly' : 'Update'} ${s.offer.version} is available on the ${s.offer.channel} channel.`;
    } catch (e) {
      this.message = `Could not read the update settings: ${reason(e)}`;
    }
  }

  private async save(prefs: Prefs): Promise<void> {
    try {
      const s = (await this.send('saving', { action: 'set_prefs', ...prefs })) as UpdateStatus | null;
      if (s) this.status = s;
    } catch (e) {
      this.message = `Could not save the update settings: ${reason(e)}`;
    }
  }

  setCheckOnStart(on: boolean): Promise<void> {
    return this.status ? this.save({ ...this.status.prefs, check_on_start: on }) : Promise.resolve();
  }

  setChannel(channel: Channel): Promise<void> {
    if (!this.status || this.status.prefs.channel === channel) return Promise.resolve();
    this.message = '';
    return this.save({ ...this.status.prefs, channel });
  }

  /** The one action that makes a request. `previous`: the nightly before the current one. */
  async check(previous = false): Promise<void> {
    if (!this.status || this.status.unavailable) return;
    this.message = previous ? 'Looking for the previous nightly…' : 'Checking for updates…';
    try {
      const s = (await this.send('checking', { action: 'check', previous })) as UpdateStatus | null;
      if (!s) return;
      this.status = s;
      const o = s.offer;
      this.message = o
        ? `${o.previous ? 'Previous nightly' : 'Update'} ${o.version} is available on the ${o.channel} channel.`
        : previous
          ? 'No other previous nightly is published.'
          : `You have the newest ${s.prefs.channel} build.`;
    } catch (e) {
      this.message = `The check failed: ${reason(e)}`;
    }
  }

  /** Opens the native confirmation; the app closes and restarts if the owner accepts. */
  async install(): Promise<void> {
    const offer = this.status?.offer;
    if (!offer || offer.previous) return; // rollback needs the matching state copy (LW-122): no in-app install
    this.message = 'Waiting for your answer in the confirmation box…';
    try {
      const r = (await this.send('installing', { action: 'install', version: offer.version })) as { installed: boolean } | null;
      if (r) this.message = r.installed ? 'Installing; Loomward restarts when it is done.' : 'Not installed: you cancelled.';
    } catch (e) {
      this.message = `The update was not installed: ${reason(e)}`;
    }
  }
}

export const updates = new Updates();
