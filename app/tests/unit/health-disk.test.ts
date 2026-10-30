import { afterEach, expect, it } from 'vitest';
import { render } from 'svelte/server';
import View from '../../src/views/health/View.svelte';
import { volumes } from '../../src/lib/stores/volumes.svelte';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';

afterEach(() => { volumes.items = null; volumes.error = ''; });

/** LW-137: Grants & health says one thing per volume about its disk, and unknown never reads as healthy. */
it('renders one disk-health line per volume and never calls an unreported disk healthy', async () => {
  const { result } = await new Client(createMockTransport()).call('volumes.list', {});
  volumes.items = result.volumes;
  const body = render(View).body;
  expect(body).toContain('id="h-volumes"');
  expect(body.match(/Disk health: /g)).toHaveLength(result.volumes.length);
  expect(body).toContain('Disk health: unknown (not reported through this USB bridge)');
  expect(body).toContain('Disk health: counters reported, not a verdict');
  expect(body).toContain('Disk health: unknown (not observed)');
  expect(body.replace(/<[^>]*>/g, ' ')).not.toMatch(/Disk health: [^<]*\b(healthy|good|passed?)\b/i);
});

it('says disk health is unknown while volumes are unavailable', () => {
  volumes.error = 'refused';
  expect(render(View).body).toContain('Disk health unknown: volumes unavailable.');
});
