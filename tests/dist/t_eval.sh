#!/bin/sh
# PLAN.md §10.9: the eval fixture. hang.sh under `sheepr run --timeout 3s` (a release build):
# check.sh PASSES. Control: hang.sh under a plain `timeout 3` (which kills only its child): check.sh
# FAILS (the escapee lives). An incomplete record (a live pid, no start time) is not a PASS: exit 2.
# Every escapee this cell made is killed at exit, by its recorded identity only.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
reap() { # record files: kill each pid whose start time is still the recorded one
  for f in "$@"; do
    [ -s "$f" ] || continue
    p=$(sed -n 1p "$f"); s=$(sed -n 2p "$f")
    [ -n "$p" ] && [ -n "$s" ] && [ "$(LC_ALL=C ps -o lstart= -p "$p" 2>/dev/null)" = "$s" ] && kill -KILL "$p" 2>/dev/null
  done
}
trap 'reap "$FX/e1" "$FX/eval/escapee"; [ -n "${sp:-}" ] && kill "$sp" 2>/dev/null; rm -rf "$FX"' EXIT
trap 'exit 143' TERM; trap 'exit 130' INT
(cd "$SR_ROOT" && env CARGO_TARGET_DIR="$FX/target" timeout 1200 cargo build -q --release --locked --bin sheepr) || { fail "release build"; finish; }
cp -R "$SR_ROOT/eval" "$FX/eval"; mkdir -p "$FX/h"
env -i PATH=/usr/bin:/bin HOME="$FX/h" XDG_STATE_HOME="$FX/h/x" SHEEPR_STATE="$FX/h/s" \
  "$FX/target/release/sheepr" run --no-sweep --timeout 3s -- "$FX/eval/hang.sh" >/dev/null 2>&1; r=$?
[ $r = 124 ] && pass "sheepr run --timeout: exit 124" || fail "exit $r, want 124"
cp "$FX/eval/escapee" "$FX/e1" 2>/dev/null
sh "$FX/eval/check.sh" > "$FX/c" 2>&1; r=$?
[ $r = 0 ] && pass "check.sh passes after sheepr" || fail "check after sheepr: $(cat "$FX/c")"
timeout 3 "$FX/eval/hang.sh" >/dev/null 2>&1
sh "$FX/eval/check.sh" > "$FX/c" 2>&1; r=$?
[ $r = 1 ] && pass "control: after a plain timeout, check.sh fails (the escapee lives)" || fail "control: check $r $(cat "$FX/c")"
# an incomplete record: this cell's own live sleep, no start time
sleep 60 & sp=$!
printf '%s\n\n' "$sp" > "$FX/eval/escapee.x"
mkdir -p "$FX/inc" && cp "$FX/eval/check.sh" "$FX/inc/" && cp "$FX/eval/escapee.x" "$FX/inc/escapee"
sh "$FX/inc/check.sh" > "$FX/c" 2>&1; r=$?
[ $r = 2 ] && pass "an incomplete record: exit 2, not a PASS" || fail "incomplete record: rc=$r $(cat "$FX/c")"
kill "$sp" 2>/dev/null; wait "$sp" 2>/dev/null; sp=""
finish
