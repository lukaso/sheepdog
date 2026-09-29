#!/bin/sh
# PLAN.md §10.9: the eval fixture. hang.sh under `sheepdog run --timeout 3s` (a release build):
# check.sh PASSES. Control: hang.sh under a plain `timeout 3` (which kills only its child): check.sh
# FAILS (the escapee lives; it is then killed here by its recorded identity).
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
(cd "$SD_ROOT" && env CARGO_TARGET_DIR="$FX/target" timeout 1200 cargo build -q --release --locked --bin sheepdog) || { fail "release build"; finish; }
cp -R "$SD_ROOT/eval" "$FX/eval"; mkdir -p "$FX/h"
env -i PATH=/usr/bin:/bin HOME="$FX/h" XDG_STATE_HOME="$FX/h/x" SHEEPDOG_STATE="$FX/h/s" \
  "$FX/target/release/sheepdog" run --no-sweep --timeout 3s -- "$FX/eval/hang.sh" >/dev/null 2>&1; r=$?
[ $r = 124 ] && pass "sheepdog run --timeout: exit 124" || fail "exit $r, want 124"
sh "$FX/eval/check.sh" > "$FX/c" 2>&1; r=$?
[ $r = 0 ] && pass "check.sh passes after sheepdog" || fail "check after sheepdog: $(cat "$FX/c")"
timeout 3 "$FX/eval/hang.sh" >/dev/null 2>&1
sh "$FX/eval/check.sh" > "$FX/c" 2>&1; r=$?
[ $r = 1 ] && pass "control: after a plain timeout, check.sh fails (the escapee lives)" || fail "control: check $r $(cat "$FX/c")"
p=$(sed -n 1p "$FX/eval/escapee"); s=$(sed -n 2p "$FX/eval/escapee")
[ "$(LC_ALL=C ps -o lstart= -p "$p" 2>/dev/null)" = "$s" ] && kill -KILL "$p" 2>/dev/null
finish
