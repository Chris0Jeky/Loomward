import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { expect, it } from 'vitest';
import { render } from 'svelte/server';
import InventoryView from '../../src/lib/ui/InventoryView.svelte';
import type { ApplicationPage, ErrorCode, InstalledApplication } from '../../src/lib/contracts.gen';
import appFixture from '../../../contracts/v3/examples/commands/apps.list.result.json';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { createInventoryMock, sortLargestFirst } from '../../src/lib/transport/mock-inventory';

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

it('orders largest estimate first, unknown last (never zero), ties in registry order', () => {
  const row = (key: string, size: string | null) => ({ registry_key: key, estimated_size_bytes: size }) as InstalledApplication;
  const rows = [row('a', '5'), row('b', null), row('c', '100'), row('d', '5'), row('e', '0'), row('f', null), row('g', '9007199254740993'), row('h', '9007199254740992')];
  expect(sortLargestFirst(rows).map((r) => r.registry_key).join('')).toBe('ghcadebf');
  expect(rows.map((r) => r.registry_key).join('')).toBe('abcdefgh');
});

it('mock apps.list sorts the whole snapshot once and binds each cursor to its sort', async () => {
  const c = new Client(createMockTransport());
  const first = await c.call('apps.list', { limit: 1, cursor: null, sort: 'size_desc' });
  expect(first.result.applications[0]?.estimated_size_bytes).toBe('1048576');
  const second = await c.call('apps.list', { limit: 1, cursor: first.result.next_cursor, sort: 'size_desc' });
  expect(second.result.applications[0]?.estimated_size_bytes).toBeNull();
  expect(second.result.next_cursor).toBeNull();
  await expect(c.call('apps.list', { limit: 1, cursor: first.result.next_cursor })).rejects.toMatchObject({ code: 'invalid_request' });
  await expect(c.call('apps.list', { limit: 1, cursor: null, sort: 'name' as never })).rejects.toMatchObject({ code: 'invalid_request' });
  await expect(c.call('apps.list', { limit: 1, cursor: null, sort: null as never })).rejects.toMatchObject({ code: 'invalid_request' });
  await expect(c.call('desktop.contexts', { limit: 1, cursor: null, sort: 'size_desc' } as never)).rejects.toMatchObject({ code: 'invalid_request' });
});

it('mock apps.list size_desc reorders a snapshot whose registry order differs from size order', () => {
  // The contract fixture is already largest-first, so a test on it passes with the mock sort removed.
  const [known, unknown] = appFixture.applications;
  const apps = [
    { ...known, registry_key: 'Small', estimated_size_bytes: '10' },
    { ...unknown!, registry_key: 'Unknown' },
    { ...known, registry_key: 'Large', estimated_size_bytes: '99' },
  ];
  const fail = (code: ErrorCode, message: string): never => {
    throw new Error(`${code}: ${message}`);
  };
  const mock = createInventoryMock(() => new Date('2026-10-01T00:00:00Z'), fail, { apps: { ...appFixture, applications: apps } });
  const list = (p: Record<string, unknown>) => mock['apps.list'](p) as ApplicationPage;
  const keys = (page: ApplicationPage) => page.applications.map((a) => a.registry_key);
  expect(keys(list({ limit: 200, cursor: null }))).toEqual(['Small', 'Unknown', 'Large']);
  const first = list({ limit: 1, cursor: null, sort: 'size_desc' });
  expect(keys(first)).toEqual(['Large']);
  const second = list({ limit: 1, cursor: first.next_cursor, sort: 'size_desc' });
  expect(keys(second)).toEqual(['Small']);
  const third = list({ limit: 1, cursor: second.next_cursor, sort: 'size_desc' });
  expect(keys(third)).toEqual(['Unknown']);
  expect(third.applications[0]?.estimated_size_bytes).toBeNull();
  expect(third.next_cursor).toBeNull();
});

it('offers a Largest first control on Applications only, off by default', () => {
  const apps = render(InventoryView, { props: { kind: 'applications' } }).body;
  expect(apps).toContain('Largest first');
  expect(apps).toMatch(/<input[^>]*type="checkbox"/);
  expect(apps).not.toMatch(/<input[^>]*checked/);
  expect(render(InventoryView, { props: { kind: 'desktop' } }).body).not.toContain('Largest first');
});
