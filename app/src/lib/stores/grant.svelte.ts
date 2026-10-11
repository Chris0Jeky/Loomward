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

  /** `known`: the root ids on screen before the click, to tell a root that lands later from one already there. */
  async ask(known: readonly string[]): Promise<void> {
    const c = session.client;
    if (!c || this.phase === 'waiting') return;
    this.set('waiting', 'Waiting for you to choose a folder in the Windows dialog.');
    try {
      const { result } = await c.call('roots.request_grant', { purpose: 'metadata_scan' }, { deadlineMs: PICKER_DEADLINE_MS });
      if (result.outcome === 'granted' && result.root) this.added(result.root);
      else if (result.outcome === 'cancelled_by_user') this.set('cancelled', 'No folder was added.');
      else if (result.outcome === 'refused' && result.refusal) this.set('refused', REFUSAL[result.refusal] ?? `The folder was refused: ${result.refusal.replaceAll('_', ' ')}.`);
      else this.set('failed', 'The service answered in a way this page does not understand. No folder was added.');
    } catch (e) {
      if (e instanceof LoomwardError && e.code === 'deadline_exceeded') await this.reread(known);
      else this.set('failed', e instanceof LoomwardError ? e.message : session.handle(e));
    }
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
