import { afterEach, describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { Client } from '../../src/lib/transport/client';
import { createMockTransport } from '../../src/lib/transport/mock';
import { session } from '../../src/lib/stores/session.svelte';
import { TransportError, type Transport } from '../../src/lib/transport/transport';
import type { BackupEvidence, BackupRefusal } from '../../src/lib/contracts.gen';
import {
  conditions, coverage, formatAge, IMPORT_REFUSAL_WORDS, observations, parseRequired, REFUSAL_WORDS, topology,
} from '../../src/views/backups/model';

const ALL_REFUSALS: BackupRefusal[] = [
  'snapshot_missing', 'version_not_covered', 'target_not_independent', 'credentials_unavailable', 'integrity_not_checked',
  'restore_not_succeeded', 'partial_restore', 'unsupported_metadata', 'stale_observation', 'invalid_observation_time',
];

async function fixture(): Promise<BackupEvidence[]> {
  return (await new Client(createMockTransport()).call('backups.list', {})).result.evidence;
}

it('keeps snapshot, integrity and restore drill separate, each with its own refusals in plain words', async () => {
  const [nas] = await fixture();
  const rows = observations(nas!);
  expect(rows.map((r) => r.name)).toEqual(['Snapshot taken', 'Integrity checked', 'Restore drill']);
  const [snap, check, drill] = rows;
  expect(snap!.good).toBe(true);
  expect(snap!.status).toContain('2026-10-09T21:00:00.123456789+01:00');
  // a clean structural check is not a full-data check
  expect(check!.good).toBe(false);
  expect(check!.status).toBe('Structure only, or full data read not shown');
  expect(check!.why).toEqual([REFUSAL_WORDS.integrity_not_checked]);
  expect(drill!.good).toBe(false);
  expect(drill!.why).toEqual([REFUSAL_WORDS.version_not_covered, REFUSAL_WORDS.restore_not_succeeded, REFUSAL_WORDS.partial_restore, REFUSAL_WORDS.unsupported_metadata]);
  expect(conditions(nas!)).toEqual([REFUSAL_WORDS.target_not_independent, REFUSAL_WORDS.credentials_unavailable]);
  // every refusal lands in exactly one place, so none is ever silently dropped
  const all = { ...nas!, refusals: ALL_REFUSALS };
  const shown = [...observations(all).flatMap((o) => o.why), ...conditions(all)];
  expect(shown.sort()).toEqual(ALL_REFUSALS.map((r) => REFUSAL_WORDS[r]).sort());
  expect(Object.keys(REFUSAL_WORDS).sort()).toEqual([...ALL_REFUSALS].sort());
  expect(Object.values(IMPORT_REFUSAL_WORDS).every((w) => w.length > 10)).toBe(true);
});

it('shows a green-looking repository as unshown recovery, and coverage, topology and unknown ages honestly', async () => {
  const [nas, usb] = await fixture();
  expect(nas!.reported_recovery_evidence).toBe(false);
  expect(coverage(nas!)).toBe('0 of 2 needed versions covered by a restore');
  expect(topology(nas!)).toEqual({ label: 'Unknown', basis: 'Different volumes; Loomward cannot yet tell whether they share a physical disk.' });
  expect(topology(usb!).label).toBe('Same volume as the source');
  expect(conditions(usb!)).toContain(REFUSAL_WORDS.stale_observation);
  expect(observations(usb!)[1]!.status).toBe('No clean check imported');
  expect(formatAge(null)).toBe('unknown');
  expect(formatAge(-1)).toBe('unknown');
  expect(formatAge(0)).toBe('0 min');
  expect(formatAge(57_300)).toBe('15 h');
  expect(formatAge(604_800)).toBe('7 days');
});

it('parses needed files strictly and names the refused lines', () => {
  const ok = parseRequired(`fixture/ledger.ods ${'A'.repeat(64)}\n\n  fixture/a b.txt   sha256:${'b'.repeat(64)}  \r\n`);
  expect(ok.errors).toEqual([]);
  expect(ok.required).toEqual([
    { relative_name: 'fixture/ledger.ods', version: `sha256:${'a'.repeat(64)}` },
    { relative_name: 'fixture/a b.txt', version: `sha256:${'b'.repeat(64)}` },
  ]);
  expect(parseRequired('').errors).toEqual(['List at least one file you need back.']);
  expect(parseRequired(`fixture/x ${'a'.repeat(63)}\nok ${'a'.repeat(64)}`).errors).toEqual(['Line 1: expected a relative name, a space and a 64-digit SHA-256.']);
  expect(parseRequired(Array.from({ length: 65 }, (_, i) => `f${i} ${'a'.repeat(64)}`).join('\n')).errors).toEqual(['At most 64 files per repository.']);
});

it('the mock forgets a record, refuses an unknown one and has no file dialog to import with', async () => {
  const c = new Client(createMockTransport());
  const forgot = await c.call('backups.forget', { evidence_id: 'be_2' });
  expect(forgot.result.evidence_id).toBe('be_2');
  expect((await c.call('backups.list', {})).result.evidence.map((e) => e.evidence_id)).toEqual(['be_1']);
  await expect(c.call('backups.forget', { evidence_id: 'be_2' })).rejects.toMatchObject({ code: 'not_found' });
  await expect(c.call('backups.import', {
    repository_label: 'synthetic-nas', snapshot_id: '1'.repeat(64), source_volume_id: null, target_volume_id: null,
    required: [{ relative_name: 'fixture/ledger.ods', version: `sha256:${'a'.repeat(64)}` }], integrity_check: false,
  })).rejects.toMatchObject({ code: 'capability_unavailable' });
  // a fresh transport starts from the synthetic fixture again
  expect((await new Client(createMockTransport()).call('backups.list', {})).result.evidence).toHaveLength(2);
});

describe('backups volumes read errors', () => {
  afterEach(() => {
    session.client = null;
    session.state = 'connecting';
    session.info = null;
    session.reason = '';
    vi.clearAllTimers();
    vi.useRealTimers();
  });

  it('surfaces a volumes.list rejection in error via session.handle and marks unavailable on TransportError', async () => {
    vi.useFakeTimers();
    session.state = 'connected';
    // Mirrors View.svelte load: backups.list succeeds, volumes.list rejects with a transport fault.
    const client = new Client({
      call: async (req: { request_id: string; command: string }) => {
        if (req.command === 'backups.list') {
          return { protocol: 'loomward/3', request_id: req.request_id, ok: true, result: { evidence: [] }, meta: {} };
        }
        throw new TransportError('engine not reachable');
      },
    } as unknown as Transport);
    session.client = client;
    expect((await client.call('backups.list', {})).result.evidence).toEqual([]);
    let error = '';
    let volumes: unknown = [{ volume_id: 'vol_old' }];
    try {
      volumes = (await client.call('volumes.list', {})).result.volumes;
    } catch (e) {
      error = session.handle(e);
      volumes = [];
    }
    expect(error).toContain('engine not reachable');
    expect(volumes).toEqual([]);
    expect(session.state).toBe('unavailable');
    expect(session.reason).toContain('engine not reachable');
  });

  it('routes the View volumes.list catch through session.handle with a generation guard', () => {
    const source = readFileSync(new URL('../../src/views/backups/View.svelte', import.meta.url), 'utf8');
    expect(source).toContain('volumes.list');
    const volumesCatch = source.slice(source.indexOf('volumes.list'));
    expect(volumesCatch).toContain('session.handle');
    expect(volumesCatch).toContain('mine !== generation');
    expect(volumesCatch).toContain('volumes = []');
    expect(source).not.toContain('catch { volumes = []; }');
  });
});
