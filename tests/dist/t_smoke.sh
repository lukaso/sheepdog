#!/bin/sh
# PHASE3.md S7: scripts/smoke.sh against a release build of sheepdog (it names what it tests; cell
# 1, cell 3 and the control all pass), and against stand-in "sheepdog"s, each of which must fail
# the smoke on the named row: one that only runs the command (cell 3: survived), one that runs it
# with no PATH so the escapee never starts (cell 3: did not start), one that changes the exit code
# (cell 1), and one that kills the escapee only after 31 s (cell 3: too slow; the smoke must tell
# a kill from a wait). Release-binary rules: a cleared environment, a temp HOME and state.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
(cd "$SD_ROOT" && env CARGO_TARGET_DIR="$FX/target" timeout 1200 cargo build -q --release --locked --bin sheepdog) || { fail "release build"; finish; }
mkdir -p "$FX/h"
sm() { env -i PATH=/usr/bin:/bin HOME="$FX/h" XDG_STATE_HOME="$FX/h/x" SHEEPDOG_STATE="$FX/h/s" TMPDIR="$FX/h" sh "$SD_ROOT/scripts/smoke.sh" "$1" > "$FX/o" 2>&1; }
B=$FX/target/release/sheepdog
"$SD_ROOT/scripts/lib/exec-guard.sh" check "$B" || { fail "the door refuses the build"; finish; }
sm "$B"; r=$?
[ $r = 0 ] && pass "smoke passes on a release build" || fail "smoke: $(tr '\n' ' ' < "$FX/o")"
for row in 'smoke: testing .*sheepdog [0-9]' 'ok: cell 1:' 'ok: cell 3:' 'ok: control:'; do
  grep -q "^$row" "$FX/o" && pass "release build: '$row'" || fail "release build: no '$row' line"
done
# the stand-ins: skip `run` and the options up to `--`, then act on the command
hd='#!/bin/sh
shift; while [ $# -gt 0 ] && [ "$1" != -- ]; do shift; done; shift'
fake() { printf '%s\n%s\n' "$hd" "$2" > "$FX/$1"; chmod +x "$FX/$1"; }
fake nokill 'exec "$@"'
fake nopath 'PATH=/nonexistent exec /bin/sh "$@"'
fake rc0 '"$@"; exit 0'
fake late '"$@"; r=$?; for a in "$@"; do f=$a; done; sleep 31
p=$(sed -n 1p "$f" 2>/dev/null); s=$(sed -n 2p "$f" 2>/dev/null)
[ -n "$p" ] && [ -n "$s" ] && [ "$(LC_ALL=C ps -o lstart= -p "$p" 2>/dev/null)" = "$s" ] && kill -KILL "$p"; exit $r'
for c in "nokill|cell 3: the escapee survived" "nopath|cell 3: the escapee did not start" "rc0|cell 1: exit 0, want 7" "late|cell 3: the job took"; do
  f=${c%%|*} want=${c#*|}
  sm "$FX/$f"; r=$?
  [ $r = 1 ] && grep -q "^FAIL: $want" "$FX/o" && pass "control: the '$f' stand-in fails the smoke ($want)" || fail "$f: rc=$r $(tr '\n' ' ' < "$FX/o")"
done
grep -q 'cell 3: the escapee survived' "$FX/o" && fail "late: the late kill was read as a survivor, not as too slow"
finish
