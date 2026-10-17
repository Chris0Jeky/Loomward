import { expect, it } from 'vitest';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { COMMANDS } from '../../src/lib/contracts.gen';

it('refuses every copy command like the engine: no effect is reachable (docs/47 C4)', async () => {
  const c = new Client(createMockTransport());
  const copy = Object.keys(COMMANDS).filter((name) => name.startsWith('copy.'));
  expect(copy.sort()).toEqual(['copy.abandon', 'copy.approve', 'copy.cancel', 'copy.preview', 'copy.start', 'copy.status']);
  const effects = (await c.call('session.hello', {})).result.capabilities.effects;
  expect(Object.values(effects).every((v) => v === false)).toBe(true);
  const digest = `sha256:${'a'.repeat(64)}`;
  await expect(c.call('copy.preview', { sources: ['nd_1'], destination_node_id: 'nd_2' }))
    .rejects.toMatchObject({ code: 'capability_unavailable', detail: { reason: 'copy_effect_gated' } });
  await expect(c.call('copy.start', { operation_id: 'cp_1', digest }))
    .rejects.toMatchObject({ code: 'capability_unavailable', detail: { reason: 'copy_effect_gated' } });
});
