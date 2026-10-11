import listFixture from '../../../../contracts/v3/examples/commands/backups.list.result.json';
import type { BackupEvidence, BackupEvidenceList, BackupForgetResult, ErrorCode } from '../contracts.gen';

/**
 * Backups on the mock transport: the contract's synthetic evidence, which `backups.forget` removes.
 * There is no `backups.import` here: like HTTP, the mock has no Rust-owned file dialog, so the
 * command answers capability_unavailable.
 */
export function createBackupMock(now: () => Date, fail: (code: ErrorCode, message: string) => never) {
  let evidence = structuredClone(listFixture.evidence) as BackupEvidence[];
  return {
    'backups.list': (p: Record<string, unknown>): BackupEvidenceList => {
      if (Object.keys(p).length) fail('invalid_request', 'backups.list takes no fields');
      return { evidence: structuredClone(evidence) };
    },
    'backups.forget': (p: Record<string, unknown>): BackupForgetResult => {
      const id = p.evidence_id;
      if (Object.keys(p).some((k) => k !== 'evidence_id') || typeof id !== 'string' || !/^be_[A-Za-z0-9_-]{1,64}$/.test(id)) fail('invalid_request', 'Expected one evidence_id.');
      if (!evidence.some((e) => e.evidence_id === id)) fail('not_found', 'unknown backup evidence');
      evidence = evidence.filter((e) => e.evidence_id !== id);
      return { evidence_id: id as string, forgotten_at: now().toISOString() };
    },
  };
}
