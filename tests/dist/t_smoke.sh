#!/bin/sh
# PHASE3.md S7: scripts/smoke.sh against a release build of sheepdog (cell 1, cell 3 and its
# control all pass), and against a stand-in "sheepdog" that only runs the command (no kill): the
# smoke must fail on cell 3. Release-binary rules: a cleared environment, a temp HOME and state.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
(cd "$SD_ROOT" && env CARGO_TARGET_DIR="$FX/target" timeout 1200 cargo build -q --release --locked --bin sheepdog) || { fail "release build"; finish; }
mkdir -p "$FX/h"
sm() { env -i PATH=/usr/bin:/bin HOME="$FX/h" XDG_STATE_HOME="$FX/h/x" SHEEPDOG_STATE="$FX/h/s" TMPDIR="$FX/h" sh "$SD_ROOT/scripts/smoke.sh" "$1" > "$FX/o" 2>&1; }
"$SD_ROOT/scripts/lib/exec-guard.sh" check "$FX/target/release/sheepdog" || { fail "the door refuses the build"; finish; }
sm "$FX/target/release/sheepdog"; r=$?
[ $r = 0 ] && pass "smoke passes on a release build" || fail "smoke: $(cat "$FX/o" | tr '\n' ' ')"
printf '#!/bin/sh\nshift; [ "$1" = -- ] && shift; exec "$@"\n' > "$FX/fake"; chmod +x "$FX/fake"
sm "$FX/fake"; r=$?
[ $r = 1 ] && grep -q 'cell 3: the escapee survived' "$FX/o" && pass "control: a sheepdog that kills nothing fails cell 3" || fail "fake: rc=$r $(cat "$FX/o" | tr '\n' ' ')"
finish
