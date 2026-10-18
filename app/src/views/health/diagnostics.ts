import type { DiagnosticsPreviewResult, DiagnosticsSaveRefusal } from '../../lib/contracts.gen';

/** Every save refusal in plain words. Nothing was written in any of them. */
export const SAVE_REFUSAL_WORDS: Record<DiagnosticsSaveRefusal, string> = {
  preview_changed: 'The bundle changed since this preview, so nothing was saved. Preview again, then save.',
  elevated_session: 'Loomward is running as administrator, so nothing was saved. Restart Loomward without administrator rights, then preview and save again.',
  unsupported_target: 'That location is not a plain local file (network, device, link or folder), so nothing was saved.',
  cloud_placeholder: 'That file is online-only in a cloud folder, so nothing was saved. Choose another file, or make it available offline first.',
  write_failed: 'The file could not be written (read-only, full, locked, or it appeared while saving). The chosen file was left as it was.',
  dialog_busy: 'Another Loomward dialog is open, so the save dialog did not open and nothing was saved. Close it, then save again.',
  dialog_unavailable: 'The save dialog is unavailable, so nothing was saved. Try again in a moment.',
  dialog_no_selection: 'The save dialog returned no location (it may have been cancelled or failed), so nothing was saved.',
};

const sorted = (v: unknown): unknown =>
  Array.isArray(v) ? v.map(sorted)
  : v && typeof v === 'object' ? Object.fromEntries(Object.keys(v).sort().map((k) => [k, sorted((v as Record<string, unknown>)[k])]))
  : v;

/**
 * The text the service writes: the bundle with its keys sorted, indented by two spaces. The bundle holds no
 * floating-point number, so this is byte for byte the serialisation the digest covers.
 */
export const canonicalText = (bundle: DiagnosticsPreviewResult['bundle']): string => JSON.stringify(sorted(bundle), null, 2);

/** Whether `text` is exactly the previewed bytes: same UTF-8 length and SHA-256 as the service reported. */
export async function matches(text: string, preview: Pick<DiagnosticsPreviewResult, 'bytes' | 'digest'>): Promise<boolean> {
  const bytes = new TextEncoder().encode(text);
  if (String(bytes.length) !== preview.bytes) return false;
  const hash = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes));
  return `sha256:${Array.from(hash, (b) => b.toString(16).padStart(2, '0')).join('')}` === preview.digest;
}
