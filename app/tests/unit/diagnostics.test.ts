import { expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import type { DiagnosticsSaveRefusal } from '../../src/lib/contracts.gen';
import { canonicalText, matches, SAVE_REFUSAL_WORDS } from '../../src/views/health/diagnostics';

const ALL_REFUSALS: DiagnosticsSaveRefusal[] = ['preview_changed', 'elevated_session', 'unsupported_target', 'cloud_placeholder', 'write_failed', 'dialog_busy', 'dialog_unavailable', 'dialog_no_selection'];

it('describes an ambiguous save dialog result without claiming the owner cancelled', () => {
  expect(SAVE_REFUSAL_WORDS).toHaveProperty('dialog_no_selection', expect.stringContaining('cancelled or failed'));
});

it('explains that an elevated session must restart without administrator rights to save', () => {
  expect(SAVE_REFUSAL_WORDS).toHaveProperty('elevated_session',
    'Loomward is running as administrator, so nothing was saved. Restart Loomward without administrator rights, then preview and save again.');
});

it('reproduces the exact previewed bytes from the received bundle and checks them against the digest', async () => {
  const c = new Client(createMockTransport());
  const { result } = await c.call('diagnostics.preview', {});
  const text = canonicalText(result.bundle);
  expect(await matches(text, result)).toBe(true);
  // keys are sorted at every depth, as the service writes them
  expect(text.indexOf('"counts"')).toBeLessThan(text.indexOf('"format"'));
  // one changed character, or a reordered key, is not the previewed bytes
  expect(await matches(text.replace('"ntfs"', '"refs"'), result)).toBe(false);
  expect(await matches(JSON.stringify(result.bundle, null, 2), result)).toBe(false);
  expect(await matches(text, { ...result, bytes: String(Number(result.bytes) + 1) })).toBe(false);
});

it('keeps unknowns visible and carries no path or name', async () => {
  const { result } = await new Client(createMockTransport()).call('diagnostics.preview', {});
  const text = canonicalText(result.bundle);
  expect(result.bundle.recent_errors).toEqual({ unknown: 'not_retained' });
  expect(result.bundle.host.memory_total_bytes).toEqual({ unknown: 'physical_memory_query_failed' });
  expect(text).not.toMatch(/\\|display_path|"name"|label"/);
});

it('has no save on the mock (no Rust dialog), and every save refusal in plain words', async () => {
  const c = new Client(createMockTransport());
  expect((await c.call('session.hello', {})).result.features.diagnostics_save).toBe(false);
  const { result } = await c.call('diagnostics.preview', {});
  await expect(c.call('diagnostics.save', { digest: result.digest })).rejects.toMatchObject({ code: 'capability_unavailable' });
  expect(Object.keys(SAVE_REFUSAL_WORDS).sort()).toEqual([...ALL_REFUSALS].sort());
  expect(Object.values(SAVE_REFUSAL_WORDS).every((w) => w.includes('nothing was saved') || w.includes('left as it was'))).toBe(true);
});
