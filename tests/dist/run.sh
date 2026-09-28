#!/bin/sh
# The phase-3 shell cells (PHASE3.md §3, the `bundle` leg): runs every tests/dist/t_*.sh, each
# with its own exit code, and exits 0 only if all passed. A cell that did not run is a failure.
set -u
cd "$(dirname "$0")" || exit 3
n=0 bad=""
for t in t_*.sh; do
  [ -f "$t" ] || continue
  n=$((n + 1))
  echo "== $t"
  if timeout 1200 sh "$t"; then :; else bad="$bad $t"; fi
done
[ "$n" -gt 0 ] || { echo "no cells found"; exit 1; }
[ -z "$bad" ] && { echo "dist: $n cells PASS"; exit 0; }
echo "dist: RED:$bad"; exit 1
