import { describe, expect, it, vi } from 'vitest';
import { CopyFlow, refusalReason } from '../../src/lib/stores/copy.svelte';
import { Client, LoomwardError } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import type { CopyPreview, CopyStatus } from '../../src/lib/contracts.gen';

const digest = `sha256:${'a'.repeat(64)}`;
const manifest: CopyPreview = {
  operation_id: 'cp_demo', state: 'draft', digest, folder_name: 'Synthetic copy',
  files: 2, folders: 1, bytes: '2048', space_needed: '4096', space_reserve: '1073741824',
  same_volume: true, explicit_acl_items: 1, hard_link_names: 0, refusals: [], refusal_count: 0,
};
const status = (state: CopyStatus['state']): CopyStatus => ({
  operation_id: manifest.operation_id, digest, state, items_total: 3,
  items_verified: state === 'committed' ? 3 : 0, staging_folder: null, leftover_staging_folders: [], refusal: null,
});
function scripted() {
  const call = vi.fn(async (command: string) => {
    if (command === 'copy.preview') return { result: structuredClone(manifest) };
    if (command === 'copy.approve') return { result: {
      operation_id: manifest.operation_id, digest, approved_at: '2026-10-17T00:00:00Z',
      expires_at: '2099-10-17T00:30:00Z',
    } };
    return { result: status(command === 'copy.start' ? 'copying' : command === 'copy.status' ? 'committed' : 'cancelled') };
  });
  return { call, client: { call } as unknown as Client };
}
function selected() {
  const flow = new CopyFlow();
  flow.select([{ node_id: 'nd_source', name: '<source>\u202e' }]);
  flow.destination({ node_id: 'nd_parent', name: 'Synthetic destination' });
  return flow;
}

describe('copy UI lifecycle (scripted transport only; no file effects)', () => {
  it('requires a preview, explicit approval, then a separate start bound to the same digest', async () => {
    const f = selected(), { call, client } = scripted();
    await f.start(client);
    expect(call).not.toHaveBeenCalled();
    await f.review(client);
    expect(call).toHaveBeenLastCalledWith('copy.preview', { sources: ['nd_source'], destination_node_id: 'nd_parent' });
    expect(f.canApprove).toBe(true);
    await f.start(client);
    expect(call).toHaveBeenCalledTimes(1);
    await f.approve(client);
    expect(call).toHaveBeenLastCalledWith('copy.approve', { operation_id: 'cp_demo', digest });
    expect(f.canStart).toBe(true);
    await f.start(client);
    expect(call).toHaveBeenLastCalledWith('copy.start', { operation_id: 'cp_demo', digest });
    expect(f.approval).toBeNull();
    expect(f.status?.state).toBe('copying');
    expect(f.canCancel).toBe(true);
    await f.refresh(client);
    expect(f.status?.state).toBe('committed');
    expect(f.canCancel).toBe(false);
  });

  it('visibly invalidates approval when destination or sources change, and rebuilds the manifest', async () => {
    const f = selected(), { client } = scripted();
    await f.review(client); await f.approve(client);
    f.destination({ node_id: 'nd_other', name: 'Other destination' });
    expect(f.preview).toBeNull(); expect(f.approval).toBeNull();
    expect(f.notice).toContain('Approval invalidated');
    await f.review(client); await f.approve(client);
    f.select([{ node_id: 'nd_other_source', name: 'Other source' }]);
    expect(f.canStart).toBe(false); expect(f.notice).toContain('Approval invalidated');
  });

  it('drops a delayed preview after selection changes', async () => {
    const f = selected();
    let reply!: (r: { result: CopyPreview }) => void;
    const client = { call: () => new Promise(resolve => { reply = resolve; }) } as unknown as Client;
    const pending = f.review(client);
    f.select([{ node_id: 'nd_new', name: 'New source' }]);
    reply({ result: manifest }); await pending;
    expect(f.preview).toBeNull(); expect(f.busy).toBe(false);
  });

  it('keeps the existing mock gate honest and exposes its code and reason', async () => {
    const f = selected();
    await f.review(new Client(createMockTransport()));
    expect(f.gated).toBe(true);
    expect(f.problem).toContain('copy_effect_gated');
    expect(f.preview).toBeNull(); expect(f.canApprove).toBe(false);
  });

  it('never approves a refused or incompletely reported manifest', async () => {
    const f = selected(), { client, call } = scripted();
    call.mockResolvedValueOnce({ result: { ...manifest, refusal_count: 2, refusals: [{ code: 'cloud_placeholder', node_id: 'nd_source' }] } });
    await f.review(client); await f.approve(client);
    expect(call).toHaveBeenCalledTimes(1); expect(f.canApprove).toBe(false);
    expect(refusalReason('cloud_placeholder')).toContain('download');
    expect(refusalReason('source_changed')).toContain('new preview');
  });

  it('rejects approval for a different digest or operation and expired approval', async () => {
    for (const extra of [{ digest: 'sha256:other' }, { operation_id: 'cp_other' }, { expires_at: '2000-01-01T00:00:00Z' }]) {
      const f = selected(), { client, call } = scripted();
      await f.review(client);
      call.mockResolvedValueOnce({ result: { operation_id: 'cp_demo', digest, approved_at: '2000-01-01T00:00:00Z', expires_at: '2099-01-01T00:00:00Z', ...extra } } as never);
      await f.approve(client);
      expect(f.canStart).toBe(false); expect(f.problem).not.toBe('');
    }
  });

  it('stops offering cancellation during publish and never calls cancel then', async () => {
    const f = selected(), { client, call } = scripted();
    await f.review(client); await f.approve(client);
    call.mockResolvedValueOnce({ result: status('publishing') } as never);
    await f.start(client); await f.cancel(client);
    expect(f.canCancel).toBe(false); expect(call).toHaveBeenCalledTimes(3);
    expect(f.select([])).toBe(false);
  });

  it('cancels or abandons only the current operation, retaining the returned outcome', async () => {
    const f = selected(), { client, call } = scripted();
    await f.review(client); await f.approve(client); await f.start(client);
    await f.cancel(client);
    expect(call).toHaveBeenLastCalledWith('copy.cancel', { operation_id: 'cp_demo' });
    expect(f.status?.state).toBe('cancelled');
    await f.review(client); await f.abandon(client);
    expect(call).toHaveBeenLastCalledWith('copy.abandon', { operation_id: 'cp_demo' });
  });

  it('retains an uncertain start for status lookup without silently starting again', async () => {
    const f = selected(), { client, call } = scripted();
    await f.review(client); await f.approve(client);
    call.mockRejectedValueOnce(new Error('response lost'));
    await f.start(client); await f.start(client);
    expect(f.locked).toBe(true); expect(f.canStart).toBe(false);
    expect(f.canApprove).toBe(false);
    expect(call).toHaveBeenCalledTimes(3);
    await f.refresh(client);
    expect(f.status?.state).toBe('committed');
  });

  it('requires renewed explicit approval for a blocked resume and only abandonment for source changes', async () => {
    const f = selected(), { client, call } = scripted();
    await f.review(client); await f.approve(client);
    call.mockResolvedValueOnce({ result: { ...status('blocked'), refusal: { code: 'source_in_use', node_id: 'nd_source' } } } as never);
    await f.start(client);
    expect(f.canApprove).toBe(true); expect(f.canStart).toBe(false);
    await f.approve(client); await f.start(client);
    call.mockResolvedValueOnce({ result: { ...status('needs_attention'), refusal: { code: 'source_changed', node_id: 'nd_source' } } } as never);
    await f.refresh(client);
    expect(f.canApprove).toBe(false); expect(f.canAbandon).toBe(true); expect(f.canCancel).toBe(false);
  });

  it('invalidates consent after an engine refusal and reports unknown status honestly', async () => {
    const f = selected(), { client, call } = scripted();
    await f.review(client); await f.approve(client);
    call.mockRejectedValueOnce(new LoomwardError({ code: 'capability_unavailable', message: 'closed', retryable: false, detail: { reason: 'copy_effect_gated' } }));
    await f.start(client);
    expect(f.approval).toBeNull(); expect(f.gated).toBe(true);
    expect(f.status).toBeNull();
  });

  it('invalidates approval when observations change while the copy view is unmounted', async () => {
    const f = selected(), { client } = scripted();
    f.observe(1);
    await f.review(client); await f.approve(client);
    f.observe(2);
    expect(f.approval).toBeNull(); expect(f.preview).toBeNull();
    expect(f.notice).toContain('Observations changed');
  });

  it('drops an approval response after observations invalidate its preview', async () => {
    const f = selected(), { client, call } = scripted();
    await f.review(client);
    let reply!: (r: unknown) => void;
    call.mockImplementationOnce(() => new Promise(resolve => { reply = resolve as never; }) as never);
    const pending = f.approve(client);
    f.invalidateObservations();
    reply({ result: { operation_id: 'cp_demo', digest, expires_at: '2099-10-17T00:30:00Z' } });
    await pending;
    expect(f.approval).toBeNull(); expect(f.canStart).toBe(false);
  });

  it('rejects status for another operation and keeps the outcome unknown', async () => {
    const f = selected(), { client, call } = scripted();
    await f.review(client); await f.approve(client);
    call.mockResolvedValueOnce({ result: { ...status('committed'), operation_id: 'cp_other' } } as never);
    await f.start(client);
    expect(f.status).toBeNull(); expect(f.uncertain).toBe(true);
    expect(f.problem).toContain('does not match');
  });

  it('keeps a draft editable after a read-only status refresh', async () => {
    const f = selected(), { client, call } = scripted();
    await f.review(client);
    call.mockResolvedValueOnce({ result: status('draft') } as never);
    await f.refresh(client);
    expect(f.locked).toBe(false); expect(f.canApprove).toBe(true);
    expect(f.select([{ node_id: 'nd_new', name: 'New choice' }])).toBe(true);
    expect(f.preview).toBeNull();
  });

  it('drops a status reply that arrives after an observation change, without a mismatch error', async () => {
    const f = selected(), { client, call } = scripted();
    f.observe(1);
    await f.review(client);
    let reply!: (r: unknown) => void;
    call.mockImplementationOnce(() => new Promise(resolve => { reply = resolve as never; }) as never);
    const pending = f.refresh(client);
    f.observe(2);
    reply({ result: status('committed') });
    await pending;
    expect(f.preview).toBeNull(); expect(f.status).toBeNull();
    expect(f.problem).toBe(''); expect(f.uncertain).toBe(false); expect(f.busy).toBe(false);
  });

  it('drops a start reply that arrives after an observation change and keeps the outcome unknown', async () => {
    const f = selected(), { client, call } = scripted();
    f.observe(1);
    await f.review(client); await f.approve(client);
    let reply!: (r: unknown) => void;
    call.mockImplementationOnce(() => new Promise(resolve => { reply = resolve as never; }) as never);
    const pending = f.start(client);
    f.observe(2);
    reply({ result: status('copying') });
    await pending;
    expect(f.status).toBeNull(); expect(f.uncertain).toBe(true);
    expect(f.preview?.operation_id).toBe('cp_demo'); expect(f.problem).toBe('');
  });

  it('drops a cancel reply that arrives after an observation change, so no state is applied', async () => {
    const f = selected(), { client, call } = scripted();
    f.observe(1);
    await f.review(client); await f.approve(client); await f.start(client);
    let reply!: (r: unknown) => void;
    call.mockImplementationOnce(() => new Promise(resolve => { reply = resolve as never; }) as never);
    const pending = f.cancel(client);
    f.observe(2);
    reply({ result: status('cancelled') });
    await pending;
    expect(f.status?.state).toBe('copying'); expect(f.uncertain).toBe(true);
    expect(f.problem).toBe('');
  });

  it('drops a refused cancel that arrives after an observation change, error and problem text included', async () => {
    const f = selected(), { client, call } = scripted();
    f.observe(1);
    await f.review(client); await f.approve(client); await f.start(client);
    let fail!: (e: unknown) => void;
    call.mockImplementationOnce(() => new Promise((_, reject) => { fail = reject; }) as never);
    const pending = f.cancel(client);
    f.observe(2);
    fail(new LoomwardError({ code: 'capability_unavailable', message: 'closed', retryable: false, detail: { reason: 'copy_effect_gated' } }));
    await pending;
    expect(f.problem).toBe(''); expect(f.gated).toBe(false); expect(f.uncertain).toBe(true);
  });

  it('clears the last status when the engine refuses a cancel, instead of showing it as current', async () => {
    const f = selected(), { client, call } = scripted();
    await f.review(client); await f.approve(client); await f.start(client);
    expect(f.status?.state).toBe('copying');
    call.mockRejectedValueOnce(new LoomwardError({ code: 'capability_unavailable', message: 'closed', retryable: false, detail: { reason: 'copy_effect_gated' } }));
    await f.cancel(client);
    expect(f.status).toBeNull(); expect(f.gated).toBe(true); expect(f.uncertain).toBe(false);
    expect(f.canCancel).toBe(false); expect(f.notice).toContain('refused');
    expect(f.problem).toContain('copy_effect_gated');
  });

  it('clears a blocked status when the engine refuses an abandon', async () => {
    const f = selected(), { client, call } = scripted();
    await f.review(client); await f.approve(client);
    call.mockResolvedValueOnce({ result: { ...status('blocked'), refusal: { code: 'source_in_use', node_id: 'nd_source' } } } as never);
    await f.start(client);
    expect(f.canAbandon).toBe(true);
    call.mockRejectedValueOnce(new LoomwardError({ code: 'capability_unavailable', message: 'closed', retryable: false, detail: { reason: 'copy_effect_gated' } }));
    await f.abandon(client);
    expect(f.status).toBeNull(); expect(f.gated).toBe(true);
  });

  it('keeps the left-in-place staging folder and the leftover staging names a cancel reports, for the receipt', async () => {
    const f = selected(), { client, call } = scripted();
    await f.review(client); await f.approve(client); await f.start(client);
    call.mockResolvedValueOnce({ result: { ...status('cancelled'), staging_folder: '~lw-staging-cp_demo', leftover_staging_folders: ['~lw-staging-cp_old'] } } as never);
    await f.cancel(client);
    expect(f.status?.state).toBe('cancelled');
    expect(f.status?.staging_folder).toBe('~lw-staging-cp_demo');
    expect(f.status?.leftover_staging_folders).toEqual(['~lw-staging-cp_old']);
    expect(f.canCancel).toBe(false);
  });
});
