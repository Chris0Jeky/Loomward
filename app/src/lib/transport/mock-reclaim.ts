import resultFixture from '../../../../contracts/v3/examples/commands/reclaim.plan.result.json';
import type { JobResult, ReclaimPlan, Volume } from '../contracts.gen';
import type { Fail } from './mock-views';

/** Invented contract evidence only; the selected volume changes the report's identity, not its observations. */
export function createReclaimMock(fail: Fail, volumes: () => Volume[], observe: (onComplete: () => void) => JobResult) {
  const known = (volume_id: unknown) => volumes().some((v) => v.volume_id === volume_id);
  /** Volumes whose first candidate the fake observe job has finished: its relief becomes a number (the other candidates stay unknown). */
  const observed = new Set<string>();
  return {
    'reclaim.plan': (p: Record<string, unknown>): ReclaimPlan => {
      const { volume_id, limit } = p;
      if (Object.keys(p).length !== 2 || typeof volume_id !== 'string' || !Number.isInteger(limit) || (limit as number) < 1 || (limit as number) > 200) {
        fail('invalid_request', 'Expected volume_id and a limit from 1 to 200.');
      }
      if (!known(volume_id)) fail('not_found', 'unknown volume');
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
      return report;
    },
    'reclaim.observe': (p: Record<string, unknown>): JobResult => {
      if (Object.keys(p).length !== 1 || typeof p.volume_id !== 'string') fail('invalid_request', 'Expected volume_id.');
      if (!known(p.volume_id)) fail('not_found', 'unknown volume');
      const volume_id = p.volume_id;
      return observe(() => void observed.add(volume_id));
    },
  };
}
