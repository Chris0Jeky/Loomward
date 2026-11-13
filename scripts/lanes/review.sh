#!/usr/bin/env bash
# Fresh-context Terra review of BASE...HEAD, defaulting to origin/main.
set -uo pipefail
[[ $# -ge 2 && $# -le 3 && $1 != *[!0-9]* && -n $1 ]] || { echo 'usage: review.sh <PR> <worktree> [BASE]' >&2; exit 2; }
PR=$1; WT=$2; BASE=${3:-origin/main}
LW_HOME=${LW_HOME:-C:/lw}
mkdir -p "$LW_HOME/logs"
: > "$LW_HOME/logs/rev-$PR.md"
cat <<PROMPT | bash ~/.claude/tools/codex_safe.sh terra high -C "$WT" \
  -o "$LW_HOME/logs/rev-$PR.md" - > "$LW_HOME/logs/rev-$PR.log" 2>&1
You are a fresh-context adversarial code reviewer for Loomward PR #$PR (repo root is the cwd, checked out at the PR head).
Read AGENTS.md (product invariants). Review ONLY the diff: \`git diff $BASE...HEAD\`. Do not edit any file; this is read-only.
Report findings ranked by severity (CRITICAL/HIGH/MEDIUM/LOW). HIGH or CRITICAL needs a realistic direct path from the changed lines to wrong behaviour, data loss, or an invariant violation; cite file:line and a concrete failure scenario. Check tests would fail if the behaviour broke.
End with exactly one line: VERDICT: BLOCKING (list ids) or VERDICT: NO BLOCKING FINDINGS.
PROMPT
rc=$?
echo "EXIT $rc" >> "$LW_HOME/logs/rev-$PR.log"
echo "$(date +%T) rev-$PR EXIT $rc $(grep -o 'VERDICT:.*' "$LW_HOME/logs/rev-$PR.md" 2>/dev/null | tail -1)" >> "$LW_HOME/logs/sol-wave.done"
exit "$rc"
