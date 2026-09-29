#!/bin/sh
# The phase-3 shell cells (PHASE3.md §3, the `bundle` leg): runs every tests/dist/t_*.sh, each
# with its own exit code, and exits 0 only if all passed. A cell that did not run is a failure,
# and so is a cell that skipped on macOS. Each cell writes to a file (a $( ) capture would wait for
# any process that kept the output open), printed whole when the cell ends. A TERM or INT to this
# runner stops the running cell (by the pid recorded here) and prints what it wrote so far.
set -u
cd "$(dirname "$0")" || exit 3
logdir=$(mktemp -d "${TMPDIR:-/tmp}/sd-dist.XXXXXX") || exit 3
cp="" gp="" cur=""
# stop the running cell: TERM to its `timeout`, then, after at most 5 s, KILL its whole process
# group (GNU timeout leads a group of its own; this runner started it and confirmed the group id
# when it started, so no other group can be hit). Then print what the cell wrote.
stop() {
  if [ -n "$cp" ]; then
    kill -TERM "$cp" 2>/dev/null
    i=0; while kill -0 "$cp" 2>/dev/null && [ $i -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
    if [ -n "$gp" ] && [ "$gp" -gt 1 ]; then kill -KILL -- "-$gp" 2>/dev/null; else kill -KILL "$cp" 2>/dev/null; fi
    [ -n "$cur" ] && [ -f "$cur" ] && { cat "$cur"; echo "(stopped by a signal)"; }
  fi
  rm -rf "$logdir"; exit "$1"
}
trap 'rm -rf "$logdir"' EXIT
trap 'stop 143' TERM
trap 'stop 130' INT
n=0 bad=""
for t in t_*.sh; do
  [ -f "$t" ] || continue
  n=$((n + 1))
  echo "== $t"
  cur="$logdir/$n"
  timeout -k 10 1200 sh "$t" > "$cur" 2>&1 & cp=$!
  # the group id, once timeout has made its group (bounded; unconfirmed means a pid-only kill)
  gp="" i=0
  while [ $i -lt 20 ]; do
    g=$(ps -o pgid= -p "$cp" 2>/dev/null | tr -d ' ')
    [ "$g" = "$cp" ] && { gp=$cp; break; }
    [ -n "$g" ] || break
    sleep 0.05; i=$((i + 1))
  done
  wait "$cp"; rc=$?; cp="" gp=""
  cat "$logdir/$n"
  if [ $rc != 0 ]; then bad="$bad $t"
  elif [ "$(/usr/bin/uname -s 2>/dev/null || uname -s)" = Darwin ] && grep -q '^SKIP' "$logdir/$n"; then bad="$bad $t(skipped on macOS)"; fi
done
[ "$n" -gt 0 ] || { echo "no cells found"; exit 1; }
[ -z "$bad" ] && { echo "dist: $n cells PASS"; exit 0; }
echo "dist: RED:$bad"; exit 1
