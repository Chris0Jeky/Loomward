import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { EventEnvelope, Job, SessionInfo } from '../../src/lib/contracts.gen';
import { scans } from '../../src/lib/stores/scan.svelte';
import { session } from '../../src/lib/stores/session.svelte';
import { Client, TransportError } from '../../src/lib/transport/client';
import type { Transport } from '../../src/lib/transport/transport';

const running: Job = {
  job_id: 'jb_fixture', seq: 1, rev: 1, kind: 'scan', state: 'running', root_id: 'rt_fixture',
  created_at: '2026-10-13T00:00:00Z', started_at: '2026-10-13T00:00:01Z', finished_at: null,
  progress: null, coverage: null, error: null,
};
const operations = ['start', 'cancel', 'adopt', 'confirm'] as const;
type Operation = typeof operations[number];
const busy = { error: { code: 'busy', message: 'already scanning', detail: { job_id: running.job_id } } };

function connect(handler: (command: string) => unknown | Promise<unknown>): void {
  session.client = new Client({
    call: async (request: { request_id: string; command: string }) => {
      const result = await handler(request.command);
      const envelope = { protocol: 'loomward/3', request_id: request.request_id };
      if (result && typeof result === 'object' && 'error' in result) {
        return { ...envelope, ok: false, error: { retryable: false, ...result.error as object } };
      }
      return { ...envelope, ok: true, result, meta: {} };
    },
  } as unknown as Transport);
}

function perform(operation: Operation): Promise<void> {
  if (operation === 'cancel') return scans.cancel(running.root_id!);
  if (operation === 'confirm') return scans.sync();
  return scans.start(running.root_id!, 'full');
}

function precedes(command: string, operation: Operation): unknown | undefined {
  if (operation === 'adopt' && command === 'scan.start') return busy;
  if (operation === 'confirm' && command === 'jobs.list') return { jobs: [] };
  return undefined;
}

function restart(): void {
  scans.onEvent({
    protocol: 'loomward/3', epoch: 'fixture_next', seq: 1, event: 'stream.hello',
    at: '2026-10-13T01:00:00Z', catalog_rev: null, state_rev: null, data: {},
  } as EventEnvelope);
}

describe('unexpected scan failures remain visible without inventing job outcomes (#452)', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    session.state = 'connected';
    session.info = { session_started_at: '2026-10-13T00:00:00Z' } as SessionInfo;
    scans.onEvent({
      protocol: 'loomward/3', epoch: 'fixture_initial', seq: 1, event: 'stream.hello',
      at: '2026-10-13T00:00:00Z', catalog_rev: null, state_rev: null, data: {},
    } as EventEnvelope);
    scans.book = {};
    scans.problems = {};
    scans.pending = {};
    scans.change = null;
    scans.apply(running);
  });
  afterEach(() => {
    session.client = null;
    session.info = null;
    session.state = 'connecting';
    vi.clearAllTimers();
    vi.useRealTimers();
  });

  it.each(operations)('reports a malformed %s result and clears it after valid confirmation', async (operation) => {
    let valid = false;
    connect((command) => precedes(command, operation) ?? { job: valid ? running : null });
    const settled = scans.settled;
    await expect(perform(operation)).resolves.toBeUndefined();
    expect(session.state).toBe('connected');
    expect(scans.problems.rt_fixture).toMatch(/^Could not (start|cancel|confirm) this scan/);
    expect(scans.change?.text).toBe(scans.problems.rt_fixture);
    expect(scans.book.rt_fixture).toEqual(running);
    expect(scans.settled).toBe(settled);
    expect(Boolean(scans.pending.rt_fixture)).toBe(false);
    valid = true;
    await perform(operation);
    if (operation === 'adopt') expect(scans.problems.rt_fixture).toContain('already running');
    else expect(scans.problems.rt_fixture).toBeUndefined();
  });

  it.each(operations)('does not attach an old unexpected %s failure to a restarted engine', async (operation) => {
    let reject!: (error: Error) => void;
    let entered!: () => void;
    const started = new Promise<void>((resolve) => { entered = resolve; });
    connect((command) => precedes(command, operation) ?? new Promise((_, fail) => {
      reject = fail;
      entered();
    }));
    const pending = perform(operation);
    await started;
    restart();
    scans.apply({ ...running, job_id: 'jb_next', seq: 1 });
    reject(new Error('failure from the old generation'));
    await expect(pending).resolves.toBeUndefined();
    expect(scans.problems).toEqual({});
    expect(scans.book.rt_fixture?.job_id).toBe('jb_next');
    expect(session.state).toBe('connected');
  });

  it.each(operations)('still hands a %s transport failure to the shell', async (operation) => {
    connect((command) => {
      const first = precedes(command, operation);
      if (first !== undefined) return first;
      throw new TransportError('fixture transport disconnected');
    });
    await expect(perform(operation)).resolves.toBeUndefined();
    expect(session.state).toBe('unavailable');
    expect(scans.problems).toEqual({});
  });
});
