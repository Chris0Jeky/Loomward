#!/bin/sh
# List ignored files, remove owned build/state directories, then plain worktree remove.
# Copy anything that must survive out first; run from another worktree.
set -eu
[ "$#" -eq 1 ] || { echo 'usage: rmwt.sh <lane>' >&2; exit 2; }
case "$1" in ''|*[!a-zA-Z0-9_-]*) echo 'bad lane' >&2; exit 2;; esac
LW_HOME=${LW_HOME:-C:/lw}
REPO=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
WT=$LW_HOME/wt/$1
if [ -d "$WT" ]; then
  [ ! -L "$WT" ] && [ -f "$WT/.git" ] || { echo 'not an owned worktree' >&2; exit 3; }
  WT=$(CDPATH= cd "$WT" && pwd -P)
  ROOT=$(CDPATH= cd "$LW_HOME/wt" && pwd -P)
  TOP=$(git -C "$WT" rev-parse --show-toplevel)
  TOP=$(CDPATH= cd "$TOP" && pwd -P)
  [ "$WT" = "$ROOT/$1" ] && [ "$WT" = "$TOP" ] || { echo 'worktree path mismatch' >&2; exit 3; }
  [ "$(git -C "$WT" rev-parse --path-format=absolute --git-common-dir)" = \
    "$(git -C "$REPO" rev-parse --path-format=absolute --git-common-dir)" ] || { echo 'different repository' >&2; exit 3; }
  case "$(pwd -P)/" in "$WT/"*) echo 'cannot remove current worktree' >&2; exit 3;; esac
  git -c core.fscache=false -C "$WT" status --porcelain --ignored
  [ -z "$(git -c core.fscache=false -C "$WT" status --porcelain)" ] || { echo 'work remains in worktree' >&2; exit 3; }
  [ ! -L "$WT/app" ] || { echo 'app is a symlink' >&2; exit 3; }
  for path in "$WT/target" "$WT/app/node_modules" "$WT/app/dist" "$WT/.loomward"; do
    [ ! -L "$path" ] || { echo "refusing symlink $path" >&2; exit 3; }
  done
  if [ -d "$WT/.loomward" ]; then
    STATE_ENTRY=$(find "$WT/.loomward" -mindepth 1 -maxdepth 1 -print -quit)
    if [ -n "$STATE_ENTRY" ]; then
      KEPT=$LW_HOME/kept/$1/.loomward-$(date -u +%Y%m%dT%H%M%SZ)
      (CDPATH= cd "$LW_HOME" && mkdir -p "kept/$1")
      [ ! -e "$KEPT" ] && [ ! -L "$KEPT" ] || { echo "kept state already exists: $KEPT" >&2; exit 3; }
      mv "$WT/.loomward" "$KEPT"
      echo "kept state at $KEPT"
    fi
  fi
  rm -rf "$WT/target" "$WT/app/node_modules" "$WT/app/dist" "$WT/.loomward"
  git -C "$REPO" worktree remove "$WT"
fi
echo "removed $1"
