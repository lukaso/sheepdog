#!/bin/sh
# The release leg of ./test-all (PHASE2.md §0.5): a release build refuses to run in a test
# environment. Builds a release sheepdog into its own target directory and runs it with HOME,
# XDG_STATE_HOME and SHEEPDOG_STATE all in a fresh temporary directory, so even a release build
# that ignored the rule could touch nothing real. Exit 0 only if:
#   control: no test variable -> it runs the command (exit 0, the marker exists)
#   SHEEPDOG_TEST_TAG set     -> exit 125, the command never ran
#   SHEEPDOG_TEST_STATE set   -> exit 125, the command never ran
set -u
cd "$(dirname "$0")/.." || exit 3
tmp=$(mktemp -d "${TMPDIR:-/tmp}/sd-release.XXXXXX") || exit 3
CARGO_TARGET_DIR="$tmp/target" cargo build -q --release --bin sheepdog || exit 3
b="$tmp/target/release/sheepdog"
mkdir -p "$tmp/home"
case_run() { # name want-rc want-marker extra-env...
  name=$1 want=$2 marker=$3; shift 3
  rm -f "$tmp/ran"
  env -i PATH=/usr/bin:/bin HOME="$tmp/home" XDG_STATE_HOME="$tmp/home/xdg" SHEEPDOG_STATE="$tmp/home/state" "$@" \
    "$b" run -- /bin/sh -c ": > '$tmp/ran'" </dev/null
  rc=$?
  has=no; [ -e "$tmp/ran" ] && has=yes
  echo "$name: rc=$rc ran=$has (want rc=$want ran=$marker)"
  [ "$rc" = "$want" ] && [ "$has" = "$marker" ]
}
ok=0
case_run control 0 yes || ok=1
case_run tag 125 no SHEEPDOG_TEST_TAG=0123456789abcdef0123456789abcdef || ok=1
case_run state 125 no SHEEPDOG_TEST_STATE="$tmp/state" || ok=1
# every entry refuses, doctor's probe children too (one would open $HOME/Documents)
env -i PATH=/usr/bin:/bin HOME="$tmp/home" SHEEPDOG_TEST_TAG=0123456789abcdef0123456789abcdef "$b" __doctor-grant </dev/null
rc=$?
echo "doctor probe: rc=$rc (want 125)"
[ "$rc" = 125 ] || ok=1
rm -rf "$tmp"
exit $ok
