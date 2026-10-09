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
TD=""   # a dead registration folder this cell plants in /private/tmp (removed at exit)
trap 'touch "$FX/go"; wait; rm -rf "$FX" ${TD:+"$TD"}' EXIT
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
# issue #20: a release sweep removes a dead registration folder where its own listener would put
# it (this TMPDIR), and never one in /private/tmp (the listener's place only when TMPDIR is unset
# or too long): the host's /tmp is not this cell's
dead_folder() { # dir: a registration folder whose owner is gone (0700, an owner record, a dead socket)
  chmod 700 "$1" && printf 'v1 999998 1099511627776 0\n' > "$1/owner" \
    && /usr/bin/python3 -I -c 'import socket, sys; socket.socket(socket.AF_UNIX).bind(sys.argv[1])' "$1/s"
}
TD=$(mktemp -d /private/tmp/sr-XXXXXXXX) && dead_folder "$TD" || { fail "a /tmp decoy"; finish; }
OD=$(mktemp -d "$FX/h/sr-XXXXXXXX") && dead_folder "$OD" || { fail "a TMPDIR folder"; finish; }
sr sweep; r=$?
[ "$r" = 0 ] && [ ! -e "$OD" ] && [ -S "$TD/s" ] && [ -f "$TD/owner" ] && grep -q 'removed 1 registration folder' "$FX/err" \
  && pass "a release sweep removes a dead registration folder in its TMPDIR, and leaves /private/tmp's" || fail "rc=$r, TMPDIR folder $( [ -e "$OD" ] && echo kept || echo removed), /tmp folder $( [ -S "$TD/s" ] && echo whole || echo touched): $(tr '\n' ' ' < "$FX/err")"
rm -rf "$TD"; TD=""
# control 1: a dead job's readable journal (its supervisor SIGKILLed): read, swept, not named
# (env, not the renv function: a function in the background is a subshell, and $! must be sheepr)
env -i PATH=/usr/bin:/bin HOME="$FX/h" XDG_STATE_HOME="$FX/h/x" SHEEPR_STATE="$FX/h/s" TMPDIR="$FX/h" "$B" run --no-sweep -- /bin/sh -c "$wait_go" < /dev/null > /dev/null 2> "$FX/err.dead" &
sup=$!
# the SIGKILL only once the journal names the root (on macOS the root is spawned stopped and the
# supervisor journals it, then resumes it: a SIGKILL before that would leave it stopped for ever)
i=0; while ! cat "$dir"*.journal 2> /dev/null | grep -q '"root":true' && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
dj=$(grep -l '"root":true' "$dir"*.journal 2> /dev/null | head -n 1)
rp=$(grep -h '"root":true' "$dir"*.journal 2> /dev/null | head -n 1 | sed -n 's/.*"pid":\([0-9]*\).*/\1/p')
kill -KILL "$sup"; wait "$sup" 2> /dev/null
# a root that has ended may still be a zombie until launchd reaps it: gone = no such pid, or Z
gone() { ! kill -0 "$1" 2> /dev/null || ps -o stat= -p "$1" 2> /dev/null | grep -q '^Z'; }
alive_before=no; [ -n "$rp" ] && ! gone "$rp" && alive_before=yes
sr sweep; r=$?
gone=no; i=0
while [ -n "$rp" ] && [ $i -lt 20 ]; do gone "$rp" && { gone=yes; break; }; sleep 0.1; i=$((i + 1)); done
[ "$alive_before" = yes ] || fail "control (dead): the root '$rp' was not alive before the sweep (the check proves nothing)"
[ -n "$dj" ] && [ "$r" = 0 ] && [ ! -e "$dj" ] && [ "$gone" = yes ] && grep -q 'swept 1 dead job ([1-9]' "$FX/err" && ! grep -q '\.journal' "$FX/err" \
  && pass "control: a dead job's readable journal is read and swept (its root $rp ended), named by no line, exit 0" || fail "control (dead): '$dj', root $rp gone=$gone, rc=$r $(tr '\n' ' ' < "$FX/err")"
# control 2: a live job's readable journal: left, not named
renv run --no-sweep -- /bin/sh -c "$wait_go" < /dev/null > /dev/null 2> "$FX/err.live" &
i=0; while [ "$(journals)" -lt 1 ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
nj=$(journals)
sr sweep; r=$?
touch "$FX/go"; wait
[ "$nj" -ge 1 ] && [ "$r" = 0 ] && ! grep -q '\.journal' "$FX/err" && pass "control: a live job's readable journal ($nj) is named by no line, exit 0" || fail "control (live): $nj journal(s), rc=$r $(tr '\n' ' ' < "$FX/err")"
finish
