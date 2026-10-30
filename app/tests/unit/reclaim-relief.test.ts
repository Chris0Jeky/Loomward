import { describe, expect, it } from 'vitest';
import sampleResult from '../../../contracts/v3/examples/commands/reclaim.sample.result.json';
import type { ReclaimRelief, ReclaimSelection } from '../../src/lib/contracts.gen';
import { Client, LoomwardError } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { reliefEstimate, reliefSummary, sampleRefusal } from '../../src/views/reclaim/model';

const relief = sampleResult as ReclaimRelief;

describe('relief check display model', () => {
  it('reports a rise beside the estimate and the Recycle Bin, without crediting the cleanup', () => {
    const s = reliefSummary({ ...relief, before: { ...relief.before, estimate_bytes: '20480' } });
    expect(s.headline).toBe('Free space rose by 9.77 KiB (10000 bytes) between the two samples.');
    expect(s.estimate).toContain('The recorded selection estimate kept with the before sample was 20.0 KiB (20480 bytes)');
    expect(s.bin).toBe('Recycle Bin: unknown (not read) at the before sample, 4.00 KiB at the second. Files sent to the Recycle Bin keep taking space on this volume until it is emptied.');
    expect(s.caveat).toContain('not proof of what a cleanup freed');
  });

  it('says so when the comparison spans a capacity change, and stays silent when capacity is equal or unknown', () => {
    expect(reliefSummary({ ...relief, capacity_changed: true }).caveat).toContain('spans a capacity change');
    expect(reliefSummary({ ...relief, capacity_changed: false }).caveat).not.toContain('capacity');
    expect(reliefSummary({ ...relief, capacity_changed: null }).caveat).not.toContain('capacity');
  });

  it('reports a fall as a fall, and no change as no change', () => {
    expect(reliefSummary({ ...relief, direction: 'less_free', change_bytes: '300' }).headline).toBe('Free space fell by 300 B (300 bytes) between the two samples.');
    expect(reliefSummary({ ...relief, direction: 'unchanged', change_bytes: '0' }).headline).toBe('Free space did not change between the two samples.');
  });

  it('never turns an unknown into a number: no estimate, no bin, unreadable space, a different volume', () => {
    const none = reliefSummary({ ...relief, before: { ...relief.before, estimate_bytes: null } });
    expect(none.estimate).toBe('No recorded selection estimate was kept with the before sample.');
    expect(reliefSummary({ ...relief, after: null, status: 'not_checked', direction: null, change_bytes: null }).bin)
      .toBe('Recycle Bin: unknown (not read) at the before sample. Files sent to the Recycle Bin keep taking space on this volume until it is emptied.');
    const unreadable = reliefSummary({ ...relief, status: 'unreadable', direction: null, change_bytes: null });
    expect(unreadable.headline).toContain('could not be read');
    expect(unreadable.headline).not.toMatch(/\d/);
    const other = reliefSummary({ ...relief, status: 'different_volume', direction: null, change_bytes: null });
    expect(other.headline).toContain('not compared');
    expect(other.headline).not.toMatch(/rose|fell|bytes/);
  });

  it('keeps only a recorded, known selection estimate for the before sample', () => {
    const selection: ReclaimSelection = { members: ['nd_a'], status: 'recorded', observed_at: '2026-10-29T00:00:00Z', reclaimable_bytes: '9007199254740993', reason: null };
    expect(reliefEstimate(selection)).toBe('9007199254740993');
    expect(reliefEstimate({ ...selection, reclaimable_bytes: null })).toBeUndefined();
    expect(reliefEstimate({ ...selection, status: 'expired' })).toBeUndefined();
    expect(reliefEstimate({ ...selection, status: 'unobserved' })).toBeUndefined();
    expect(reliefEstimate(null)).toBeUndefined();
  });

  it('puts refusals in plain words and says nothing was recorded', () => {
    expect(sampleRefusal(new LoomwardError({ code: 'device_offline', message: 'the volume is offline', retryable: false, detail: null }))).toContain('Nothing was recorded');
    const unreadable = sampleRefusal(new LoomwardError({ code: 'device_offline', message: 'could not be read', retryable: true, detail: { reason: 'volume_unreadable' } }));
    expect(unreadable).toContain('earlier before sample was kept');
    expect(unreadable).not.toContain('offline');
    expect(sampleRefusal(new LoomwardError({ code: 'invalid_request', message: 'record a before sample first', retryable: false, detail: null }))).toContain('Record a before sample first');
    expect(sampleRefusal(new LoomwardError({ code: 'invalid_request', message: 'no_active_grant', retryable: false, detail: null }))).toContain('active grant');
    expect(sampleRefusal(new LoomwardError({ code: 'internal_error', message: 'state store error', retryable: false, detail: null }))).toBe('state store error');
  });
});

describe('mock reclaim.sample', () => {
  async function volume(client: Client): Promise<string> {
    return (await client.call('volumes.list', {})).result.volumes[0]!.volume_id;
  }

  it('records a before and an after sample, and the plan shows the stored check', async () => {
    const client = new Client(createMockTransport());
    const volume_id = await volume(client);
    expect((await client.call('reclaim.plan', { volume_id, limit: 50 })).result.relief).toBeUndefined();
    const before = (await client.call('reclaim.sample', { volume_id, role: 'before', estimate_bytes: '20480' })).result;
    expect(before).toMatchObject({ status: 'not_checked', after: null, direction: null, change_bytes: null });
    expect(before.before.estimate_bytes).toBe('20480');
    const after = (await client.call('reclaim.sample', { volume_id, role: 'after' })).result;
    expect(after).toMatchObject({ status: 'compared', direction: 'more_free', change_bytes: '10240' });
    expect(after.after!.estimate_bytes).toBeNull();
    expect((await client.call('reclaim.plan', { volume_id, limit: 50 })).result.relief).toEqual(after);
    expect((await client.call('reclaim.sample', { volume_id, role: 'before' })).result.after).toBeNull();
  });

  it('refuses an after sample without a before sample, an estimate with after, and anything else', async () => {
    const client = new Client(createMockTransport());
    const volume_id = await volume(client);
    await expect(client.call('reclaim.sample', { volume_id, role: 'after' })).rejects.toMatchObject({ code: 'invalid_request' });
    await client.call('reclaim.sample', { volume_id, role: 'before' });
    for (const payload of [{ volume_id, role: 'after', estimate_bytes: '1' }, { volume_id, role: 'during' }, { volume_id, role: 'before', estimate_bytes: '-1' }, { volume_id, role: 'before', path: 'C:\\' }]) {
      await expect(client.call('reclaim.sample', payload as never)).rejects.toBeInstanceOf(LoomwardError);
    }
    await expect(client.call('reclaim.sample', { volume_id: 'missing', role: 'before' })).rejects.toMatchObject({ code: 'not_found' });
  });
});
