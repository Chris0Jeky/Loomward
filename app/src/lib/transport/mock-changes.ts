import resultFixture from '../../../../contracts/v3/examples/commands/changes.new_since.result.json';
import type { ChangesNewSince, ErrorCode, NewSinceFile, NewSinceFolder, Root } from '../contracts.gen';

/** The synthetic catalogue's idea of what is new in one root: real mock node ids, invented as new. */
export interface MockNew {
  files: NewSinceFile[];
  folders: NewSinceFolder[];
  fileCount: number;
  dirCount: number;
  logical: bigint;
  /** Sum of the known allocations, or null when any new file's allocation is unknown. */
  allocated: bigint | null;
  allocationUnknown: number;
}

/**
 * `changes.new_since` on the mock transport: the first root answers with invented new rows and every other root has no
 * earlier scan (`no_baseline`, every figure null). Nothing here is measured; the shapes and honesty rules are the engine's.
 */
export function createChangesMock(
  fail: (code: ErrorCode, message: string) => never,
  roots: () => Root[],
  invented: (rootId: string, limit: number) => MockNew | null,
) {
  return {
    'changes.new_since': (p: Record<string, unknown>): ChangesNewSince => {
      const { root_id, baseline_run, limit } = p;
      if (Object.keys(p).sort().join(',') !== 'baseline_run,limit,root_id' || typeof root_id !== 'string'
        || !(baseline_run === null || (typeof baseline_run === 'string' && /^(0|[1-9][0-9]{0,19})$/.test(baseline_run)))
        || !Number.isInteger(limit) || (limit as number) < 1 || (limit as number) > 50) {
        fail('invalid_request', 'Expected root_id, baseline_run (or null) and a limit from 1 to 50.');
      }
      const list = roots().filter((r) => r.grant_state === 'active');
      const at = list.findIndex((r) => r.root_id === root_id);
      if (at < 0) fail('not_found', 'unknown root');
      const base = structuredClone(resultFixture) as unknown as ChangesNewSince;
      base.root_id = root_id as string;
      const found = at === 0 ? invented(root_id as string, limit as number) : null;
      if (!found) {
        return {
          ...base, outcome: 'no_baseline', baseline: null, new_files: null, new_dirs: null, logical_bytes: null, allocated_bytes: null,
          allocation_unknown_files: null, largest_files: [], largest_folders: [], partial: false, partial_reasons: [], caveats: [],
        };
      }
      return {
        ...base, outcome: 'ok', new_files: found.fileCount, new_dirs: found.dirCount, logical_bytes: found.logical.toString(),
        allocated_bytes: found.allocated === null ? null : found.allocated.toString(), allocation_unknown_files: found.allocationUnknown,
        largest_files: found.files, largest_folders: found.folders,
        partial: false, partial_reasons: [],
      };
    },
  };
}
