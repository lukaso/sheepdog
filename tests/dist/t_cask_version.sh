#!/bin/sh
# scripts/lib/cask-version.sh OLD NEW: how a release's version compares with the tap's cask, so
# `publish` never moves the cask back. The grammar is render-cask.sh's (X.Y.Z or X.Y.Z-rc.N); each
# field compares as a number (0.10.0 is after 0.9.0, which a string compare gets wrong), and an rc
# comes before its final. It prints newer, older or same; a version off the grammar (or a second
# line) is refused with exit 2.
set -u
. "$(dirname "$0")/lib.sh"
V="$SR_ROOT/scripts/lib/cask-version.sh"
row() { # old new want
  got=$(sh "$V" "$1" "$2" 2>/dev/null); rc=$?
  [ $rc = 0 ] && [ "$got" = "$3" ] && pass "$1 -> $2: $3" || fail "$1 -> $2: '$got' (rc $rc), want $3"
}
row 0.9.0 0.10.0 newer
row 0.10.0 0.9.0 older
row 0.1.0-rc.2 0.1.0 newer
row 0.1.0 0.1.0-rc.2 older
row 0.2.0-rc.1 0.1.2 older
row 0.1.2 0.2.0-rc.1 newer
row 0.1.0-rc.9 0.1.0-rc.10 newer
row 0.1.2 0.1.2 same
row 0.1.0-rc.3 0.1.0-rc.3 same
row 1.0.0 0.99.99 older
nl='
'
for bad in "0.1" "v0.1.0" "0.1.0-beta.1" "0.1.0-rc.0" "" "0.1.0${nl}0.2.0" "0.1.0 "; do
  for args in "old" "new"; do
    if [ $args = old ]; then out=$(sh "$V" "$bad" 0.1.0 2>&1); else out=$(sh "$V" 0.1.0 "$bad" 2>&1); fi; rc=$?
    [ $rc = 2 ] && printf '%s' "$out" | grep -q 'cask-version: bad version' \
      && pass "a bad $args version '$(printf '%s' "$bad" | tr '\n' '|')': refused (2)" || fail "bad $args '$bad': rc=$rc '$out'"
  done
done
out=$(sh "$V" 0.1.0 2>&1); rc=$?
[ $rc = 2 ] && pass "one argument: usage (2)" || fail "one argument: rc=$rc"
finish
