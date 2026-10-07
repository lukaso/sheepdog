#!/bin/sh
# Issue #15 (first step), as its comment asks: a RELEASE build of `sweep` says a journal it cannot
# read: one stderr line that names the file, exit 0 (before, only a debug build's test trace ever
# saw it). The controls: a dead job's readable journal is read and swept, and a live job's is
# left, and no line names either. Release-binary rules, as t_smoke.sh: the build through the exec
# door, a cleared environment, a temp HOME and state.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
# the jobs this cell starts end at exit, also on a signal under dash (whose EXIT trap does not run
# on one): their roots wait for `go`, at most 30 s
trap 'touch "$FX/go"; wait; rm -rf "$FX"' EXIT
trap 'exit 1' INT TERM HUP
(cd "$SR_ROOT" && env CARGO_TARGET_DIR="$FX/target" timeout 1200 cargo build -q --release --locked --bin sheepr) || { fail "release build"; finish; }
B=$FX/target/release/sheepr
"$SR_ROOT/scripts/lib/exec-guard.sh" check "$B" || { fail "the door refuses the build"; finish; }
mkdir -p "$FX/h"
renv() { env -i PATH=/usr/bin:/bin HOME="$FX/h" XDG_STATE_HOME="$FX/h/x" SHEEPR_STATE="$FX/h/s" TMPDIR="$FX/h" "$B" "$@"; }
sr() { renv "$@" < /dev/null > /dev/null 2> "$FX/err"; }
wait_go="i=0; while [ ! -e '$FX/go' ] && [ \$i -lt 600 ]; do sleep 0.05; i=\$((i + 1)); done"
journals() { ls "$dir" | grep -c '\.journal$'; }
# one short job makes this boot's journal folder (its own journal is gone at its end)
sr run -- /bin/sh -c 'exit 0' || { fail "a first run: $(tr '\n' ' ' < "$FX/err")"; finish; }
dir=$(ls -d "$FX/h/s/jobs/"*/ 2>/dev/null | head -n 1)
[ -n "$dir" ] && [ -d "$dir" ] || { fail "no journal folder after a run"; finish; }
printf 'not a journal\n' > "${dir}j-b4d.journal" && chmod 600 "${dir}j-b4d.journal"
sr sweep; r=$?
n=$(grep -c 'j-b4d\.journal' "$FX/err")
[ "$r" = 0 ] && [ "$n" = 1 ] && pass "a release sweep says the unreadable journal once, exit 0" || fail "rc=$r, $n line(s): $(tr '\n' ' ' < "$FX/err")"
rm -f "${dir}j-b4d.journal"
# control 1: a dead job's readable journal (its supervisor SIGKILLed): read, swept, not named
# (env, not the renv function: a function in the background is a subshell, and $! must be sheepr)
env -i PATH=/usr/bin:/bin HOME="$FX/h" XDG_STATE_HOME="$FX/h/x" SHEEPR_STATE="$FX/h/s" TMPDIR="$FX/h" "$B" run --no-sweep -- /bin/sh -c "$wait_go" < /dev/null > /dev/null 2> "$FX/err.dead" &
sup=$!
i=0; while [ "$(journals)" -lt 1 ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
dj=$(ls "$dir" | grep '\.journal$' | head -n 1)
kill -KILL "$sup"; wait "$sup" 2> /dev/null
sr sweep; r=$?
[ -n "$dj" ] && [ "$r" = 0 ] && [ ! -e "$dir$dj" ] && grep -q 'swept 1 dead job' "$FX/err" && ! grep -q '\.journal' "$FX/err" \
  && pass "control: a dead job's readable journal is read and swept, named by no line, exit 0" || fail "control (dead): '$dj', rc=$r $(tr '\n' ' ' < "$FX/err")"
# control 2: a live job's readable journal: left, not named
renv run --no-sweep -- /bin/sh -c "$wait_go" < /dev/null > /dev/null 2> "$FX/err.live" &
i=0; while [ "$(journals)" -lt 1 ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
nj=$(journals)
sr sweep; r=$?
touch "$FX/go"; wait
[ "$nj" -ge 1 ] && [ "$r" = 0 ] && ! grep -q '\.journal' "$FX/err" && pass "control: a live job's readable journal ($nj) is named by no line, exit 0" || fail "control (live): $nj journal(s), rc=$r $(tr '\n' ' ' < "$FX/err")"
finish
