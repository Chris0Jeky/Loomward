import resultFixture from '../../../../contracts/v3/examples/commands/reclaim.plan.result.json';
import type { ReclaimPlan, Volume } from '../contracts.gen';
import type { Fail } from './mock-views';

/** Invented contract evidence only; the selected volume changes the report's identity, not its observations. */
export function createReclaimMock(fail: Fail, volumes: () => Volume[]) {
  return {
    'reclaim.plan': (p: Record<string, unknown>): ReclaimPlan => {
      const { volume_id, limit } = p;
      if (Object.keys(p).length !== 2 || typeof volume_id !== 'string' || !Number.isInteger(limit) || (limit as number) < 1 || (limit as number) > 200) {
        fail('invalid_request', 'Expected volume_id and a limit from 1 to 200.');
      }
      if (!volumes().some((v) => v.volume_id === volume_id)) fail('not_found', 'unknown volume');
      const report = structuredClone(resultFixture) as ReclaimPlan;
      report.volume.volume_id = volume_id as string;
      for (const group of report.groups) {
        if (group.candidates.length > (limit as number)) report.truncated = true;
        group.candidates = group.candidates.slice(0, limit as number);
      }
      return report;
    },
  };
}
