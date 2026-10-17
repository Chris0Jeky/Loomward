import resultFixture from '../../../../contracts/v3/examples/commands/clutter.developer.result.json';
import type { DeveloperClutterReport, ErrorCode, Root } from '../contracts.gen';

/**
 * `clutter.developer` on the mock transport: the contract's invented report for the first root, an
 * honest "budget hit" for the hostile-named root and a partial, empty answer for the rest. Nothing here is
 * measured, and no root is ever reported clean.
 */
export function createClutterMock(
  fail: (code: ErrorCode, message: string) => never,
  roots: () => Root[],
  budgetRoot: string,
) {
  return {
    'clutter.developer': (p: Record<string, unknown>): DeveloperClutterReport => {
      const id = p.root_id;
      const limit = p.limit;
      if (Object.keys(p).length !== 2 || typeof id !== 'string' || !Number.isInteger(limit) || (limit as number) < 1 || (limit as number) > 200) {
        fail('invalid_request', 'Expected root_id and a limit from 1 to 200.');
      }
      const list = roots();
      const at = list.findIndex((r) => r.root_id === id);
      if (at < 0 || list[at]!.grant_state !== 'active') fail('not_found', 'unknown root');
      const base = structuredClone(resultFixture) as unknown as DeveloperClutterReport;
      base.root_id = id as string;
      if (id === budgetRoot) {
        return { ...base, candidates: [], partial: true, budget_hit: true, budget_note: 'The search reached its work budget before it finished. No candidates are listed, and that says nothing about what this folder holds.' };
      }
      if (at > 0) return { ...base, candidates: [], partial: true };
      const candidates = base.candidates.slice(0, limit as number);
      return { ...base, candidates, truncated: candidates.length < base.candidates.length };
    },
  };
}
