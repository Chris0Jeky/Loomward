import type { Root, RootGrantResult } from '../contracts.gen';
import { escapedName } from '../format/names';
import { Client, LoomwardError } from '../transport/client';
import { session } from './session.svelte';

export type Phase = 'idle' | 'waiting' | 'granted' | 'cancelled' | 'deadline' | 'refused' | 'failed';

/** The most the contract allows a call to wait (semantics.md section 2): the owner may sit in the native dialog. */
export const PICKER_DEADLINE_MS = 60_000;
/** The service's busy message for a picker that is already open (`handlers.rs`); its other busy replies are not a dialog. */
const DIALOG_OPEN = 'a native dialog is already open';

/** Tolerate capitalization, padding and punctuation without swallowing other busy replies. */
function isDialogOpen(message: string): boolean {
  return message.trim().toLowerCase().includes(DIALOG_OPEN);
}
type Pick = { client: Client; epoch: number; info: typeof session.info; known: readonly string[]; expires: number };

const REFUSAL: Record<NonNullable<RootGrantResult['refusal']>, string> = {
  volume_root: 'That is a whole drive. Choose a folder on it instead.',
  reparse_or_placeholder: 'That folder is a link or a cloud placeholder, which Loomward does not follow.',
  not_a_directory: 'That is not a folder.',
  contains_state_dir: "That folder contains Loomward's own state folder.",
  inside_state_dir: "That folder is inside Loomward's own state folder.",
  already_granted: 'That folder, or one inside or around it, is already granted.',
  network_or_removable_unsupported: 'Network and removable drives are not supported.',
  protected_profile_or_store: 'That folder holds a user profile or a credential store, which Loomward does not read.',
  identity_unavailable: "Loomward could not verify that folder's identity, so it was not added.",
  synthetic_session_requires_lab_root: 'This synthetic session takes registered lab folders only.',
};

/**
 * "Add a folder…": asks the desktop service to open its native folder picker. The webview supplies no path;
 * Rust owns selection and grant validation, and this class only holds and words the result. A request that
 * ran out of time (`deadline_exceeded`) may still have been answered in the dialog, so the roots are read
 * again instead of the picker being opened a second time.
 */
class GrantPicker {
  phase = $state<Phase>('idle');
  /** What the status region says; empty while idle. */
  text = $state('');
  /** The root just added, so the list can point at it. */
  rootId = $state<string | null>(null);

  private set(phase: Phase, text: string, rootId: string | null = null): void {
    this.phase = phase;
    this.text = text;
    this.rootId = rootId;
  }

  private added(root: Root): void {
    this.set('granted', `Added ${escapedName(root.display_path.text)}. Nothing has been read yet: scan it from the list below.`, root.root_id);
  }

  private pending: Pick | null = null;

  private current(pick: Pick): boolean {
    if (this.pending !== pick) return false;
    if (session.client === pick.client && session.connectionEpoch === pick.epoch && session.info === pick.info) return true;
    this.pending = null;
    this.set('idle', '');
    return false;
  }

  // shortcut: a fresh root can come from another client, correlate request outcomes if exact attribution is needed.
  private reconcile(roots: readonly Root[], pick: Pick): boolean {
    const fresh = roots.filter((r) => r.grant_state === 'active' && !pick.known.includes(r.root_id));
    if (fresh.length === 1) this.added(fresh[0]!);
    else if (fresh.length > 1) this.set('idle', 'Folders changed. Check the list below.');
    return fresh.length > 0;
  }

  /**
   * `known`: the root ids on screen before the click, or null while the first roots read has not finished. Without
   * that baseline an existing root would be reported as the new grant, so nothing is asked.
   */
  async ask(known: readonly string[] | null): Promise<void> {
    const c = session.client;
    if (this.pending) this.current(this.pending);
    if (!c || this.phase === 'waiting') return;
    if (!known) {
      this.set('failed', 'The list of folders has not loaded yet. Refresh it, then add a folder.');
      return;
    }
    const pick: Pick = { client: c, epoch: session.connectionEpoch, info: session.info, known: [...known], expires: Infinity };
    this.pending = pick;
    this.set('waiting', 'Waiting for you to choose a folder in the Windows dialog.');
    try {
      const { result } = await c.call('roots.request_grant', { purpose: 'metadata_scan' }, { deadlineMs: PICKER_DEADLINE_MS });
      if (!this.current(pick)) return;
      if (result.outcome === 'granted' && result.root) this.added(result.root);
      else if (result.outcome === 'cancelled_by_user') this.set('cancelled', 'No folder was added.');
      else if (result.outcome === 'refused' && result.refusal) this.set('refused', REFUSAL[result.refusal] ?? `The folder was refused: ${result.refusal.replaceAll('_', ' ')}.`);
      else this.set('failed', 'The service answered in a way this page does not understand. No folder was added.');
    } catch (e) {
      if (!this.current(pick)) return;
      pick.expires = Date.now() + PICKER_DEADLINE_MS;
      if (e instanceof LoomwardError && e.code === 'deadline_exceeded') await this.reread(pick);
      // Only this busy means a dialog is up; the cooldown and mutation-capacity busy replies keep their own message.
      else if (e instanceof LoomwardError && e.code === 'busy' && isDialogOpen(e.message)) this.set('deadline', 'A Windows folder dialog may still be open. Check it, then refresh the folder list.');
      else this.set('failed', e instanceof LoomwardError ? e.message : session.handle(e));
    }
  }

  /**
   * The roots on screen changed (a read finished, or `roots.changed` arrived). A root that lands after the request ran
   * out of time is worded as added, still without reopening the picker; an "Added" for a root that is gone is dropped.
   */
  sync(roots: readonly Root[]): void {
    const pick = this.pending;
    if (pick && !this.current(pick)) return;
    if (this.phase === 'deadline' && pick) {
      if (Date.now() >= pick.expires) { this.dismiss(); return; }
      this.reconcile(roots, pick);
    } else if (this.phase === 'granted' && this.rootId && !roots.some((r) => r.root_id === this.rootId && r.grant_state === 'active')) {
      this.set('idle', '');
    }
  }

  /** Reopening Health cannot revive a receipt from a previous connection, even before its roots load. */
  resume(): void {
    if (this.pending) this.current(this.pending);
  }

  /** Leaving the page retires old outcomes; a request still awaiting its reply can finish offscreen. */
  dismiss(): void {
    if (this.phase !== 'waiting') {
      this.pending = null;
      this.set('idle', '');
    }
  }

  private async reread(pick: Pick): Promise<void> {
    try {
      const { result } = await pick.client.call('roots.list', {});
      if (!this.current(pick)) return;
      if (Date.now() >= pick.expires) {
        this.pending = null;
        this.set('idle', '');
        return;
      }
      if (!this.reconcile(result.roots, pick)) this.set('deadline', 'No folder was added yet. If the Windows dialog is still open, choose a folder there and it will appear below.');
    } catch (e) {
      if (!this.current(pick)) return;
      this.set('failed', e instanceof LoomwardError ? e.message : session.handle(e));
    }
  }
}

export const grantPicker = new GrantPicker();
