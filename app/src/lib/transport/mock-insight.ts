import resultFixture from '../../../../contracts/v3/examples/commands/insight.folder.result.json';
import type { CoverageState, ErrorCode, InsightFolder, Root } from '../contracts.gen';

export interface InsightNode {
  kind: string;
  root: string;
  coverage: CoverageState;
}

const COVERAGE_LINE: Partial<Record<CoverageState, string>> = {
  partial: 'Coverage is partial, so counts and dates may be incomplete.',
  stale: 'Coverage is stale: the figures describe an earlier scan.',
};

/**
 * `insight.folder` on the mock transport: the contract's invented insight for a complete folder, and an honest
 * "every axis abstains on coverage" answer for any other. Invented evidence only; nothing here is measured, a
 * label is never a probability or an approval, and the real wording comes from the engine's explainer.
 */
export function createInsightMock(
  fail: (code: ErrorCode, message: string) => never,
  roots: () => Root[],
  node: (id: string) => InsightNode | null,
) {
  return {
    'insight.folder': (p: Record<string, unknown>): InsightFolder => {
      const { root_id, node_id } = p;
      if (Object.keys(p).length !== 2 || typeof root_id !== 'string' || typeof node_id !== 'string') {
        fail('invalid_request', 'Expected root_id and node_id.');
      }
      if (!roots().some((r) => r.root_id === root_id && r.grant_state === 'active')) fail('not_found', 'unknown root');
      const found = node(node_id as string);
      if (found && found.kind !== 'dir' && found.kind !== 'root') fail('invalid_request', 'insight.folder needs a directory node');
      if (!found || found.root !== root_id) fail('not_found', 'unknown node');
      const report = structuredClone(resultFixture) as unknown as InsightFolder;
      report.root_id = root_id as string;
      report.node_id = node_id as string;
      if (found!.coverage === 'complete') return report;
      report.coverage = found!.coverage;
      report.partial = true;
      report.axes = report.axes.map((a) => ({ axis: a.axis, outcome: 'abstain', label: null, abstain_reason: 'coverage', votes: [] }));
      report.lines = [{
        text: COVERAGE_LINE[found!.coverage] ?? `Nothing was observed (coverage is ${found!.coverage}), so every figure is unknown.`,
        trace: ['/coverage'],
      }];
      return report;
    },
  };
}
