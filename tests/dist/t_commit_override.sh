#!/bin/sh
# PHASE3.md D7: in the release container the worktree's .git names a host path, so the release
# passes the commit in. build.rs uses SHEEPR_COMMIT_OVERRIDE when it is set (hex, 7-40
# characters; anything else fails the build), and a change to it rebuilds. Without it, a build
# outside a git checkout still says "unknown" (the control).
set -u
. "$(dirname "$0")/lib.sh"
T=$(mktemp -d /private/tmp/sr-p3-crate.XXXXXX) || exit 3
trap 'rm -rf "$T"' EXIT
# the tracked files as they are in the working tree (so a mutant of build.rs is what builds)
(cd "$SR_ROOT" && git ls-files -z | xargs -0 tar -cf -) | tar -xmf - -C "$T" || exit 3
[ -e "$T/.git" ] && { echo "the copy is a git checkout"; exit 3; }
b() { (cd "$T" && env "$@" CARGO_TARGET_DIR="$T/target" timeout 600 cargo build -q --bin sheepr 2>"$T/err"); }
ver() { "$SR_ROOT/scripts/lib/exec-guard.sh" exec "$T/target/debug/sheepr" --version 2>&1; }

b -u SHEEPR_COMMIT_OVERRIDE || { cat "$T/err"; exit 3; }
case $(ver) in *unknown*) pass "no override: unknown" ;; *) fail "no override: $(ver)" ;; esac
b SHEEPR_COMMIT_OVERRIDE=abc1234 || fail "override build failed: $(head -3 "$T/err")"
case $(ver) in *abc1234*) pass "override named" ;; *) fail "override not named: $(ver)" ;; esac
b SHEEPR_COMMIT_OVERRIDE=0123456789abcdef0123456789abcdef01234567 || fail "40-hex override failed"
case $(ver) in *0123456789abcdef0123456789abcdef01234567*) pass "a changed override rebuilds" ;; *) fail "stale: $(ver)" ;; esac
for bad in 'abc' 'xyz1234' 'abc1234 (evil)' '0123456789abcdef0123456789abcdef012345678'; do
  if b SHEEPR_COMMIT_OVERRIDE="$bad"; then fail "override '$bad' accepted"; else pass "override '$bad' fails the build"; fi
done
b SHEEPR_COMMIT_OVERRIDE= || fail "an empty override fails the build"
case $(ver) in *unknown*) pass "an empty override counts as unset" ;; *) fail "empty override: $(ver)" ;; esac
b -u SHEEPR_COMMIT_OVERRIDE || fail "rebuild without override failed"
case $(ver) in *unknown*) pass "unset again: unknown" ;; *) fail "unset again: $(ver)" ;; esac
finish
