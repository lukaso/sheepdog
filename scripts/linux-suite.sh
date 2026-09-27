#!/bin/sh
# One Linux leg of ./test-all, run INSIDE a container: copy the tree (read-only at /src), build,
# run the whole suite, then check that nothing the suite started is left. Exit 0 only if the
# suite passed and nothing is left.
#   SD_LEG_PROBE=pidfd|kill  also run the pidfd probe and require that door (ENOSYS leg: kill)
#   SD_REUSE_TEST            passed through (the privileged pid-reuse race cell)
set -u
if command -v apk >/dev/null; then
  apk add -q procps bash util-linux-misc >/dev/null || exit 3
else
  (command -v ps >/dev/null && command -v bash >/dev/null && command -v script >/dev/null) || { apt-get update -qq >/dev/null && apt-get install -y -qq procps bsdutils >/dev/null; } || exit 3
fi
# SD_PS_SHIM=1 (the emulated amd64 leg): under Rosetta, `ps` shows the translator and the
# executable in front of every argv ("/run/rosetta/rosetta /bin/sleep /bin/sleep M"); the
# tests read argv from `ps`, so a shim early on PATH strips that prefix
if [ -n "${SD_PS_SHIM:-}" ]; then
  real=$(command -v ps)
  printf '#!/bin/sh\n%s "$@" | sed -E "s#/run/rosetta/rosetta [^ ]+ ##"\n' "$real" > /usr/local/bin/ps && chmod +x /usr/local/bin/ps
fi
rm -rf /w && mkdir /w && cd /src && tar cf - --exclude=./target --exclude='./target-*' --exclude=./spike . | (cd /w && tar xf -) && cd /w || exit 3
export CARGO_TARGET_DIR=/tgt
cargo build -q --tests 2>&1 | grep -E '^error' -A6 && exit 3
timeout 3000 cargo test --no-fail-fast > /tmp/suite.log 2>&1
rc=$?
grep -E '^test result|^thread|FAILED|left:|right:' /tmp/suite.log | cut -c1-200
left=$(ps -eo args | grep -cE '^(/bin/sleep 2[0-9]\.|\S*sd-fixture |\S*/sheepdog run)')
echo "leftovers: $left"
probe_ok=0
if [ -n "${SD_LEG_PROBE:-}" ]; then
  # which door delivers signals: a stray the kill must end
  rm -f /tmp/probe.log
  SHEEPDOG_TEST_SIGNAL_LOG=/tmp/probe.log timeout 30 /tgt/debug/sheepdog run --quiet -- /bin/sh -c '/bin/sleep 29.5 & exit 0'
  p=$(grep -c '^pidfd ' /tmp/probe.log); k=$(grep -c '^kill ' /tmp/probe.log)
  echo "probe: pidfd=$p kill=$k (want $SD_LEG_PROBE)"
  case "$SD_LEG_PROBE" in
    pidfd) [ "$p" -gt 0 ] || probe_ok=1 ;;
    kill) [ "$p" -eq 0 ] && [ "$k" -gt 0 ] || probe_ok=1 ;;
  esac
fi
[ "$rc" -eq 0 ] && [ "$left" -eq 0 ] && [ "$probe_ok" -eq 0 ]
