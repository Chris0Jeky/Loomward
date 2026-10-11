import type { Root, RootGrantResult } from '../contracts.gen';
import { escapedName } from '../format/names';
import { LoomwardError } from '../transport/client';
import { session } from './session.svelte';

export type Phase = 'idle' | 'waiting' | 'granted' | 'cancelled' | 'deadline' | 'refused' | 'failed';

/** The most the contract allows a call to wait (semantics.md section 2): the owner may sit in the native dialog. */
export const PICKER_DEADLINE_MS = 60_000;

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
 * "Add a folder…": asks the desktop service to open its native folder picker. The webview never sees or
 * supplies a path; the service decides every outcome and this class only holds and words it. A request that
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

  /** The root ids on screen when the request went out, kept while the outcome is open so a root landing later is told from one already there. */
  private baseline: readonly string[] | null = null;

  /**
   * `known`: the root ids on screen before the click, or null while the first roots read has not finished. Without
   * that baseline an existing root would be reported as the new grant, so nothing is asked.
   */
  async ask(known: readonly string[] | null): Promise<void> {
    const c = session.client;
    if (!c || this.phase === 'waiting') return;
    if (!known) {
      this.set('failed', 'The list of folders has not loaded yet. Refresh it, then add a folder.');
      return;
    }
    this.baseline = known;
    this.set('waiting', 'Waiting for you to choose a folder in the Windows dialog.');
    try {
      const { result } = await c.call('roots.request_grant', { purpose: 'metadata_scan' }, { deadlineMs: PICKER_DEADLINE_MS });
      if (result.outcome === 'granted' && result.root) this.added(result.root);
      else if (result.outcome === 'cancelled_by_user') this.set('cancelled', 'No folder was added.');
      else if (result.outcome === 'refused' && result.refusal) this.set('refused', REFUSAL[result.refusal] ?? `The folder was refused: ${result.refusal.replaceAll('_', ' ')}.`);
      else this.set('failed', 'The service answered in a way this page does not understand. No folder was added.');
    } catch (e) {
      if (e instanceof LoomwardError && e.code === 'deadline_exceeded') await this.reread(known);
      else if (e instanceof LoomwardError && e.code === 'busy') this.set('deadline', 'A Windows folder dialog is still open. Choose a folder there and it will appear below.');
      else this.set('failed', e instanceof LoomwardError ? e.message : session.handle(e));
    }
  }

  /**
   * The roots on screen changed (a read finished, or `roots.changed` arrived). A root that lands after the request ran
   * out of time is worded as added, still without reopening the picker; an "Added" for a root that is gone is dropped.
   */
  sync(roots: readonly Root[]): void {
    if (this.phase === 'deadline' && this.baseline) {
      const base = this.baseline;
      const fresh = roots.find((r) => !base.includes(r.root_id));
      if (fresh) this.added(fresh);
    } else if (this.phase === 'granted' && this.rootId && !roots.some((r) => r.root_id === this.rootId && r.grant_state === 'active')) {
      this.set('idle', '');
    }
  }

  /** The page was opened again: an old outcome is not news. A dialog that may still be open stays worded. */
  dismiss(): void {
    if (this.phase === 'granted' || this.phase === 'cancelled' || this.phase === 'refused' || this.phase === 'failed') this.set('idle', '');
  }

  private async reread(known: readonly string[]): Promise<void> {
    try {
      const { result } = await session.client!.call('roots.list', {});
      const fresh = result.roots.find((r) => !known.includes(r.root_id));
      if (fresh) this.added(fresh);
      else this.set('deadline', 'No folder was added yet. If the Windows dialog is still open, choose a folder there and it will appear below.');
    } catch (e) {
      this.set('failed', e instanceof LoomwardError ? e.message : session.handle(e));
    }
  }
}

export const grantPicker = new GrantPicker();
