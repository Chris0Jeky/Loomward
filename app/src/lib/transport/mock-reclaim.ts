import resultFixture from '../../../../contracts/v3/examples/commands/reclaim.plan.result.json';
import type { JobResult, ReclaimPlan, ReclaimRelief, ReclaimSelection, ReliefSample, Volume } from '../contracts.gen';
import { parseBytes } from '../format/bytes';
import type { Fail } from './mock-views';

/** Invented contract evidence only; the selected volume changes the report's identity, not its observations. */
export function createReclaimMock(fail: Fail, volumes: () => Volume[], observe: (onComplete: () => void) => JobResult) {
  const known = (volume_id: unknown) => volumes().some((v) => v.volume_id === volume_id);
  /** Volumes whose first candidate the fake observe job has finished: its relief becomes a number (the other candidates stay unknown). */
  const observed = new Set<string>();
  const selections = new Map<string, ReclaimSelection>();
  /** Invented samples per volume. Each later sample reads 10,240 more free bytes, so the panel has a change to show. */
  const samples = new Map<string, { before: ReliefSample; after: ReliefSample | null }>();
  let drift = 0n;
  const reliefOf = (id: string): ReclaimRelief | undefined => {
    const pair = samples.get(id);
    if (!pair) return undefined;
    const was = parseBytes(pair.before.free_bytes);
    const now = parseBytes(pair.after?.free_bytes);
    const compared = pair.after && was !== null && now !== null;
    return {
      before: pair.before, after: pair.after,
      status: !pair.after ? 'not_checked' : compared ? 'compared' : 'unreadable',
      direction: compared ? (now > was ? 'more_free' : now < was ? 'less_free' : 'unchanged') : null,
      change_bytes: compared ? String(now > was ? now - was : was - now) : null,
    };
  };
  const selectionOf = (p: Record<string, unknown>): string[] | undefined => {
    if (!('selection' in p)) return undefined;
    const ids = p.selection;
    const candidates = resultFixture.groups.flatMap((g) => g.candidates.map((c) => c.node_id));
    if (!Array.isArray(ids) || !ids.length || ids.length > 12 || new Set(ids).size !== ids.length
        || ids.some((id) => typeof id !== 'string' || !candidates.includes(id))) {
      fail('invalid_request', 'Select 1 to 12 distinct current candidates on this volume.');
    }
    return [...ids as string[]].sort();
  };
  return {
    'reclaim.plan': (p: Record<string, unknown>): ReclaimPlan => {
      const { volume_id, limit } = p;
      if (Object.keys(p).some((k) => !['volume_id', 'limit', 'selection'].includes(k)) || typeof volume_id !== 'string' || !Number.isInteger(limit) || (limit as number) < 1 || (limit as number) > 200) {
        fail('invalid_request', 'Expected volume_id and a limit from 1 to 200.');
      }
      if (!known(volume_id)) fail('not_found', 'unknown volume');
      const selection = selectionOf(p);
      const report = structuredClone(resultFixture) as ReclaimPlan;
      report.volume.volume_id = volume_id as string;
      if (observed.has(volume_id as string)) {
        const first = report.groups[0]!.candidates[0]!;
        first.reclaimable_bytes = '6144'; // 2 KiB of the candidate's 8,000 bytes are shared with files outside it
        first.reclaimable_reason = null;
      }
      for (const group of report.groups) {
        if (group.candidates.length > (limit as number)) report.truncated = true;
        group.candidates = group.candidates.slice(0, limit as number);
      }
      const relief = reliefOf(volume_id as string);
      if (relief) report.relief = structuredClone(relief);
      if (selection) {
        const saved = selections.get(volume_id as string);
        report.selection = saved?.members.join() === selection.join() ? structuredClone(saved)
          : { members: selection, status: 'unobserved', observed_at: null, reclaimable_bytes: null, reason: 'This selection has not been observed.' };
      }
      return report;
    },
    'reclaim.sample': (p: Record<string, unknown>): ReclaimRelief => {
      const { volume_id, role, estimate_bytes } = p;
      if (Object.keys(p).some((k) => !['volume_id', 'role', 'estimate_bytes'].includes(k)) || typeof volume_id !== 'string'
          || (role !== 'before' && role !== 'after') || (estimate_bytes !== undefined && (role !== 'before' || parseBytes(estimate_bytes as string) === null))) {
        fail('invalid_request', 'Expected volume_id, a role of before or after, and an estimate only with before.');
      }
      const volume = volumes().find((v) => v.volume_id === volume_id);
      if (!volume) fail('not_found', 'unknown volume');
      const free = parseBytes(volume!.free_bytes);
      const sample: ReliefSample = {
        sampled_at: new Date().toISOString(), capacity_bytes: volume!.capacity_bytes,
        free_bytes: free === null ? null : String(free + drift), recycle_bin_bytes: null,
        estimate_bytes: typeof estimate_bytes === 'string' ? estimate_bytes : null,
      };
      drift += 10240n;
      const saved = samples.get(volume_id);
      if (role === 'before') samples.set(volume_id, { before: sample, after: null });
      else if (!saved) fail('invalid_request', 'record a before sample first');
      else saved!.after = sample;
      return structuredClone(reliefOf(volume_id)!);
    },
    'reclaim.observe': (p: Record<string, unknown>): JobResult => {
      if (Object.keys(p).some((k) => !['volume_id', 'selection'].includes(k)) || typeof p.volume_id !== 'string') fail('invalid_request', 'Expected volume_id.');
      if (!known(p.volume_id)) fail('not_found', 'unknown volume');
      const volume_id = p.volume_id;
      const selection = selectionOf(p);
      if (selection) {
        const job = observe(() => void selections.set(volume_id, { members: selection, status: 'recorded',
          observed_at: new Date().toISOString(), reclaimable_bytes: '6144', reason: null }));
        selections.delete(volume_id);
        return job;
      }
      return observe(() => void observed.add(volume_id));
    },
  };
}
