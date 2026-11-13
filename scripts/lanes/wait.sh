#!/bin/sh
# Wake the driver when any named lane finishes; print the status of every lane.
[ "$#" -gt 0 ] || { echo 'usage: wait.sh <lane>...' >&2; exit 2; }
LW_HOME=${LW_HOME:-C:/lw}
while true; do
  n=0
  for lane in "$@"; do grep -q '^EXIT ' "$LW_HOME/logs/$lane.log" 2>/dev/null || n=$((n+1)); done
  [ "$n" -lt "$#" ] && break
  sleep 120
done
for lane in "$@"; do
  printf '%s: ' "$lane"
  grep '^EXIT ' "$LW_HOME/logs/$lane.log" 2>/dev/null || echo running
done
