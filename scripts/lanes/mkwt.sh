#!/bin/sh
# Create a detached worktree from origin/main, then attach a new branch.
set -eu
[ "$#" -eq 2 ] || { echo 'usage: mkwt.sh <lane> <branch>' >&2; exit 2; }
case "$1" in ''|*[!a-zA-Z0-9_-]*) echo 'bad lane' >&2; exit 2;; esac
LW_HOME=${LW_HOME:-C:/lw}
REPO=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
git -C "$REPO" check-ref-format --branch "$2" > /dev/null
if git -C "$REPO" show-ref -q --verify "refs/heads/$2"; then echo "branch $2 exists (earlier work?)" >&2; exit 3; fi
mkdir -p "$LW_HOME/wt"
git -C "$REPO" fetch -q origin
git -C "$REPO" worktree add -q --detach "$LW_HOME/wt/$1" origin/main
git -C "$LW_HOME/wt/$1" switch -q -c "$2"
echo "worktree $LW_HOME/wt/$1 branch $2"
