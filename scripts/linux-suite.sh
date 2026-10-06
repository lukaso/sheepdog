#!/bin/sh
# One Linux leg of ./test-all, run INSIDE a container: copy the tree (read-only at /src), build,
# run the whole suite, then check that nothing the suite started is left. Exit 0 only if the
# suite passed and nothing is left.
#   SR_LEG_PROBE=pidfd|kill  also run the pidfd probe and require that door (ENOSYS leg: kill)
#   SR_REUSE_TEST            passed through (the privileged pid-reuse race cell)
set -u
if command -v apk >/dev/null; then
  apk add -q procps bash util-linux-misc >/dev/null || exit 3
else
  (command -v ps >/dev/null && command -v bash >/dev/null && command -v script >/dev/null) || { apt-get update -qq >/dev/null && apt-get install -y -qq procps bsdutils >/dev/null; } || exit 3
fi
# SR_PS_SHIM=1 (the emulated amd64 leg): under Rosetta, `ps` shows the translator and the
# executable in front of every argv ("/run/rosetta/rosetta /bin/sleep /bin/sleep M"); the
# tests read argv from `ps`, so a shim early on PATH strips that prefix
if [ -n "${SR_PS_SHIM:-}" ]; then
  real=$(command -v ps)
  printf '#!/bin/sh\n%s "$@" | sed -E "s#/run/rosetta/rosetta [^ ]+ ##"\n' "$real" > /usr/local/bin/ps && chmod +x /usr/local/bin/ps
fi
rm -rf /w && mkdir /w && cd /src && tar cf - --exclude=./target --exclude='./target-*' --exclude=./spike . | (cd /w && tar xmf -) && cd /w || exit 3
# build in a directory of the container, seeded from the mounted cache (deps keep their mtimes,
# so they stay fresh; the incremental data is left out): a build that writes to the bind mount
# failed at random ("libc required to be available in rlib format", a left-over incremental
# directory), and every leg recompiles sheepr anyway
mkdir -p /t && (cd /tgt && tar cf - --exclude=./debug/incremental .) | (cd /t && tar xf -) || exit 3
export CARGO_TARGET_DIR=/t CARGO_INCREMENTAL=0
# cargo's output lock (scripts/lib/warnings.sh), passed in by test-all's leg_run: without it a
# cargo config could hide a warning from the check (a progress bar, colour codes)
. /src/scripts/lib/warnings.sh || exit 3
for kv in $WARN_CARGO_ENV; do
  [ "$(printenv "${kv%%=*}")" = "${kv#*=}" ] || { echo "cargo's output lock is missing: ${kv%%=*} (test-all's leg_run passes it)"; exit 3; }
done
# the build's whole output reaches the leg's log (under the lock a quiet build prints only its
# diagnostics): test-all's record() fails a leg on a compiler warning in it (issue #10), so a
# warning in Linux-only code is seen too
cargo build -q --tests > /tmp/build.log 2>&1; brc=$?
cut -c1-200 /tmp/build.log
[ $brc = 0 ] || exit 3
timeout 3000 cargo test --no-fail-fast -- --nocapture > /tmp/suite.log 2>&1
rc=$?
grep -E '^test result|^thread|FAILED|left:|right:' /tmp/suite.log | cut -c1-200
# a leftover by name, and any live process that still carries this leg's tag (test-all passes
# SHEEPR_LEG_TAG; the runner gives it to every test process), whatever its name. Counted
# again for up to 3 s: a process still exiting when the suite ends is not a leftover (under an
# emulator the last cells' kills take a while), one that is alive after 3 s is (the name
# patterns are 20-29 s sleeps and live supervisors). The ones left are printed.
PAT='^(/bin/sleep 2[0-9]\.|\S*sr-fixture |\S*/sheepr run|sheepr (run|__root))'
tagged_pids() {
  [ -n "${SHEEPR_LEG_TAG:-}" ] || return 0
  for d in /proc/[0-9]*; do
    { tr '\0' '\n' < "$d/environ" | grep -qx "SHEEPR_TEST_TAG=$SHEEPR_LEG_TAG"; } 2>/dev/null && echo "${d#/proc/}"
  done
}
i=0
while :; do
  left=$(ps -eo args | grep -cE "$PAT")
  tagged=$(tagged_pids | grep -c .)
  { [ "$left" -eq 0 ] && [ "$tagged" -eq 0 ]; } || [ $i -ge 30 ] && break
  sleep 0.1; i=$((i+1))
done
if [ "$left" -gt 0 ] || [ "$tagged" -gt 0 ]; then
  ps -eo pid=,args= | while read -r p a; do echo "$a" | grep -qE "$PAT" && ps -o pid,ppid,stat,etime,args -p "$p" | tail -n +2 | cut -c1-200; done
  for p in $(tagged_pids); do echo "tagged: $(ps -o pid,ppid,stat,etime,args -p "$p" | tail -n +2 | cut -c1-200)"; done
fi
echo "leftovers: $left, tagged: $tagged"
left=$((left+tagged))
# the pid-reuse race cell returns early without SR_REUSE_TEST: require that it ran
race_ok=0
if [ -n "${SR_REUSE_TEST:-}" ]; then
  grep -q 'race cell ran: 4 legs' /tmp/suite.log || { echo "the pid-reuse race cell did not run"; race_ok=1; }
fi
probe_ok=0
if [ -n "${SR_LEG_PROBE:-}" ]; then
  # which door delivers signals: a stray the kill must end
  rm -f /tmp/probe.log
  SHEEPR_TEST_SIGNAL_LOG=/tmp/probe.log timeout 30 /t/debug/sheepr run --quiet -- /bin/sh -c '/bin/sleep 29.5 & exit 0'
  p=$(grep -c '^pidfd ' /tmp/probe.log); k=$(grep -c '^kill ' /tmp/probe.log)
  echo "probe: pidfd=$p kill=$k (want $SR_LEG_PROBE)"
  case "$SR_LEG_PROBE" in
    pidfd) [ "$p" -gt 0 ] || probe_ok=1 ;;
    kill) [ "$p" -eq 0 ] && [ "$k" -gt 0 ] || probe_ok=1 ;;
  esac
fi
[ "$rc" -eq 0 ] && [ "$left" -eq 0 ] && [ "$probe_ok" -eq 0 ] && [ "$race_ok" -eq 0 ]
