// "About this folder" (LW-138, #877): plain-language rows for an `insight.folder` result. The engine's
// explanation lines are shown exactly as returned; only the three axis rows are worded here, and only
// as inference (invariant 3: a label is never an approval; invariant 4: an abstention stays visible).
import type { ErrorCode, InsightAxis, InsightFolder } from '../contracts.gen';

export const INSIGHT_FOOTER = 'Inferred from folder facts; not a recommendation and not an approval to change anything.';

export interface AxisRow {
  axis: string;
  name: string;
  /** What the axis says, or why it does not decide. */
  text: string;
  /** Names of the rules that voted for the label (empty when the axis abstains). */
  rules: string[];
  decided: boolean;
}

const NAME: Record<string, string> = { meaning: 'Meaning', lifecycle: 'Lifecycle', recreatability: 'Recreatability' };
const words = (s: string) => s.replaceAll('_', ' ');
const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

function undecided(a: InsightAxis): string {
  switch (a.abstain_reason) {
    case 'no_vote': return 'not decided — no rule had enough evidence';
    case 'coverage': return 'not decided — coverage is not complete, so no rule ran';
    case 'conflict': {
      const seen = [...new Set(a.votes.map((v) => words(v.label)))];
      return `not decided — rules disagree${seen.length ? ` (${seen.join(', ')})` : ''}`;
    }
    default: return 'not decided';
  }
}

/** One row per returned axis, in the order the engine sent them. */
export function axisRows(r: InsightFolder): AxisRow[] {
  return r.axes.map((a) => {
    const decided = a.outcome === 'label' && a.label !== null;
    return {
      axis: a.axis,
      name: NAME[a.axis] ?? cap(words(a.axis)),
      text: decided ? `${words(a.label!)} (rule inference, not a recorded fact)` : undecided(a),
      rules: decided ? a.votes.map((v) => v.function) : [],
      decided,
    };
  });
}

/** Why a request failed, in words that say what was and was not done. Unlisted codes keep the engine's message. */
export function insightFailure(code: ErrorCode | null, message: string): string {
  switch (code) {
    case 'permission_denied': return 'Describing a folder is not available to this session.';
    case 'not_found': return 'This folder is not in the catalogue any more, or its grant was revoked. Nothing was described.';
    case 'invalid_request': return 'Only folders can be described.';
    case 'resource_budget': return 'This folder is too large to describe within the budget. Nothing was guessed.';
    case 'deadline_exceeded': return 'Describing took too long and was stopped. You can try again.';
    default: return message || 'Describing failed.';
  }
}
