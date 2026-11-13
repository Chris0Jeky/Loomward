#!/usr/bin/env bash
# Launch when free RAM >= MINMB (3000 MB), disk >= 25 GB, running lanes < MAX (3).
# One queue process owns logs/queue.lanes; an EXIT line marks a completed lane.
set -euo pipefail
[[ $# -gt 0 ]] || { echo 'usage: queue.sh <lane>...' >&2; exit 2; }
for lane in "$@"; do
  [[ -n $lane && $lane != *[!a-zA-Z0-9_-]* ]] || { echo 'bad lane' >&2; exit 2; }
done
export LW_HOME=${LW_HOME:-C:/lw}
DIR=$(CDPATH= cd "$(dirname "$0")" && pwd)
MAX=${MAX:-3}; MINMB=${MINMB:-3000}; STATE=$LW_HOME/logs/queue.lanes
mkdir -p "$LW_HOME/logs"
touch "$STATE"
for lane in "$@"; do
  while true; do
    n=0
    while IFS= read -r running; do
      if [[ -f "$LW_HOME/logs/$running.log" ]] && ! grep -q '^EXIT ' "$LW_HOME/logs/$running.log"; then n=$((n+1)); fi
    done < <(sort -u "$STATE")
    h=$(powershell -NoProfile -Command '[int]((Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory/1024)' | tr -d '\r')
    d=$(df -Pk "$LW_HOME" | tail -1 | awk '{print $4}')
    [[ $h -ge $MINMB && $d -ge 26214400 && $n -lt $MAX ]] && break
    sleep 90
  done
  if [[ -f "$LW_HOME/logs/$lane.log" ]] && ! grep -q '^EXIT ' "$LW_HOME/logs/$lane.log"; then
    echo "$lane is already running" >&2; exit 3
  fi
  echo "$lane" >> "$STATE"
  : > "$LW_HOME/logs/$lane.log"
  echo "$(date +%T) launching $lane free $h MB disk $((d/1048576)) GB running $n"
  (P=$lane bash "$DIR/lane.sh" "$lane" &)
  sleep 120
done
