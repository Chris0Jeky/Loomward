#!/usr/bin/env bash
# Run one Sol lane; logs and completion notices live under LW_HOME.
set -uo pipefail
[[ $# == 1 && $1 != *[!a-zA-Z0-9_-]* && -n $1 ]] || { echo 'usage: lane.sh <lane>' >&2; exit 2; }
lane=$1
export LW_HOME=${LW_HOME:-C:/lw}
DIR=$(CDPATH= cd "$(dirname "$0")" && pwd)
mkdir -p "$LW_HOME/logs"
: > "$LW_HOME/logs/$lane.last.md"
unset CARGO_TARGET_DIR
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=line-tables-only
export CARGO_PROFILE_TEST_DEBUG=line-tables-only CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-3}
{
  if [[ -r "$DIR/brief.md" && -r "$LW_HOME/prompts/${P:-$lane}.md" ]]; then
    cat "$DIR/brief.md" "$LW_HOME/prompts/${P:-$lane}.md" |
      bash ~/.claude/tools/codex_safe.sh sol high -C "$LW_HOME/wt/$lane" -s workspace-write \
        -o "$LW_HOME/logs/$lane.last.md" -
    rc=$?
  else
    echo "missing brief or prompt for $lane"
    rc=2
  fi
  echo "EXIT $rc"
} > "$LW_HOME/logs/$lane.log" 2>&1
echo "$(date +%T) $lane EXIT $rc" >> "$LW_HOME/logs/sol-wave.done"
exit "$rc"
