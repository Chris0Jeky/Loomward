import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';

it('pages synthetic apps without changing unknown sizes, provenance or provider coverage', async () => {
  const c = new Client(createMockTransport());
  const a = await c.call('apps.list', { limit: 1, cursor: null });
  expect(a.meta.dataset_class).toBe('synthetic');
  expect(a.result.applications[0]?.estimated_size_bytes).toBe('1048576');
  expect(a.result.providers.some((p) => p.state === 'denied')).toBe(true);
  const b = await c.call('apps.list', { limit: 200, cursor: a.result.next_cursor });
  expect(b.result.applications).toHaveLength(1);
  expect(b.result.applications[0]?.estimated_size_bytes).toBeNull();
  expect(b.result.applications[0]?.name.value).toContain('<img');
  expect(b.result.next_cursor).toBeNull();
  expect(b.result.observed_at).toBe(a.result.observed_at);
  await expect(c.call('desktop.contexts', { limit: 1, cursor: a.result.next_cursor })).rejects.toMatchObject({ code: 'invalid_request' });
  const other = new Client(createMockTransport());
  await expect(other.call('apps.list', { limit: 1, cursor: a.result.next_cursor })).rejects.toMatchObject({ code: 'stale_generation' });
  await expect(c.call('apps.list', { limit: 201, cursor: null })).rejects.toMatchObject({ code: 'invalid_request' });
});

it('keeps redirected and public desktop contexts separate from shortcut target health', async () => {
  const c = new Client(createMockTransport());
  const { result } = await c.call('desktop.contexts', { limit: 200, cursor: null });
  expect(result.contexts[0]?.redirected).toBe(true);
  expect(result.contexts[1]?.ownership).toBe('public');
  expect(result.contexts[4]?.location).toBe('unavailable');
  expect(result.shortcuts.map((s) => s.health)).toEqual(['missing', 'volume_unavailable', 'network_unreachable_unknown', 'unknown']);
  expect(result.shortcuts[0]?.arguments.value).toBe('--synthetic');
});

it('shows an unknown reason under publisher, version and install location, as it does for the name', () => {
  const src = readFileSync(join(import.meta.dirname, '..', '..', 'src', 'lib', 'ui', 'InventoryView.svelte'), 'utf8');
  const lines = src.split('\n');
  for (const field of ['name', 'publisher', 'version', 'install_location']) {
    const row = lines.find((l) => l.includes(`evidence(app.${field})`));
    expect(row, field).toBeDefined();
    expect(row, field).toContain(`{#if app.${field}.reason}`);
  }
});
