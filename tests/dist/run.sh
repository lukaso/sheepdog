#!/bin/sh
# The phase-3 shell cells (PHASE3.md §3, the `bundle` leg): runs every tests/dist/t_*.sh, each
# with its own exit code, and exits 0 only if all passed. A cell that did not run is a failure.
set -u
cd "$(dirname "$0")" || exit 3
n=0 bad=""
log=$(mktemp -d "${TMPDIR:-/tmp}/sd-dist.XXXXXX")/cell || exit 3
for t in t_*.sh; do
  [ -f "$t" ] || continue
  n=$((n + 1))
  echo "== $t"
  # streamed, and kept in a file for the SKIP check (a $( ) capture would wait for any process
  # that kept the output open)
  timeout 1200 sh "$t" > "$log.$n" 2>&1 & p=$!
  tail -f "$log.$n" & tp=$!
  wait $p; rc=$?
  sleep 0.2; kill $tp 2>/dev/null; wait $tp 2>/dev/null
  if [ $rc != 0 ]; then bad="$bad $t"
  elif [ "$(/usr/bin/uname -s)" = Darwin ] && grep -q '^SKIP' "$log.$n"; then bad="$bad $t(skipped on macOS)"; fi
done
[ "$n" -gt 0 ] || { echo "no cells found"; exit 1; }
[ -z "$bad" ] && { echo "dist: $n cells PASS"; exit 0; }
echo "dist: RED:$bad"; exit 1
