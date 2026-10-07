#!/bin/sh
# Issue #15 (first step), as its comment asks: a RELEASE build of `sweep` says a journal it cannot
# read: one stderr line that names the file, exit 0 (before, only a debug build's test trace ever
# saw it). The control, the same state without that file, names no journal. Release-binary rules,
# as t_smoke.sh: the build through the exec door, a cleared environment, a temp HOME and state.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
(cd "$SR_ROOT" && env CARGO_TARGET_DIR="$FX/target" timeout 1200 cargo build -q --release --locked --bin sheepr) || { fail "release build"; finish; }
B=$FX/target/release/sheepr
"$SR_ROOT/scripts/lib/exec-guard.sh" check "$B" || { fail "the door refuses the build"; finish; }
mkdir -p "$FX/h"
sr() { env -i PATH=/usr/bin:/bin HOME="$FX/h" XDG_STATE_HOME="$FX/h/x" SHEEPR_STATE="$FX/h/s" TMPDIR="$FX/h" "$B" "$@" < /dev/null > /dev/null 2> "$FX/err"; }
# one short job makes this boot's journal folder (its own journal is gone at its end)
sr run -- /bin/sh -c 'exit 0' || { fail "a first run: $(tr '\n' ' ' < "$FX/err")"; finish; }
dir=$(ls -d "$FX/h/s/jobs/"*/ 2>/dev/null | head -n 1)
[ -n "$dir" ] && [ -d "$dir" ] || { fail "no journal folder after a run"; finish; }
printf 'not a journal\n' > "${dir}j-b4d.journal" && chmod 600 "${dir}j-b4d.journal"
sr sweep; r=$?
n=$(grep -c 'j-b4d\.journal' "$FX/err")
[ "$r" = 0 ] && [ "$n" = 1 ] && pass "a release sweep says the unreadable journal once, exit 0" || fail "rc=$r, $n line(s): $(tr '\n' ' ' < "$FX/err")"
rm -f "${dir}j-b4d.journal"
sr sweep; r=$?
[ "$r" = 0 ] && ! grep -q '\.journal' "$FX/err" && pass "control: no line names a journal, exit 0" || fail "control: rc=$r $(tr '\n' ' ' < "$FX/err")"
finish
