#!/bin/sh
# The phase-3 shell cells (PHASE3.md §3, the `bundle` leg): runs every tests/dist/t_*.sh, each
# with its own exit code, and exits 0 only if all passed. A cell that did not run is a failure,
# and so is a cell that skipped on macOS. Each cell writes to a file (a $( ) capture would wait for
# any process that kept the output open), printed whole when the cell ends. A TERM or INT to this
# runner stops the running cell, by the pid recorded here.
set -u
cd "$(dirname "$0")" || exit 3
logdir=$(mktemp -d "${TMPDIR:-/tmp}/sd-dist.XXXXXX") || exit 3
cp=""
trap 'rm -rf "$logdir"' EXIT
trap '[ -n "$cp" ] && kill -TERM "$cp" 2>/dev/null; rm -rf "$logdir"; exit 143' TERM
trap '[ -n "$cp" ] && kill -TERM "$cp" 2>/dev/null; rm -rf "$logdir"; exit 130' INT
n=0 bad=""
for t in t_*.sh; do
  [ -f "$t" ] || continue
  n=$((n + 1))
  echo "== $t"
  timeout 1200 sh "$t" > "$logdir/$n" 2>&1 & cp=$!
  wait "$cp"; rc=$?; cp=""
  cat "$logdir/$n"
  if [ $rc != 0 ]; then bad="$bad $t"
  elif [ "$(/usr/bin/uname -s 2>/dev/null || uname -s)" = Darwin ] && grep -q '^SKIP' "$logdir/$n"; then bad="$bad $t(skipped on macOS)"; fi
done
[ "$n" -gt 0 ] || { echo "no cells found"; exit 1; }
[ -z "$bad" ] && { echo "dist: $n cells PASS"; exit 0; }
echo "dist: RED:$bad"; exit 1
