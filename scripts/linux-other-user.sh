#!/bin/sh
# The another-user leg of ./test-all, INSIDE a root container: build as root, then run every
# test binary as the unprivileged user `sd`, while a decoy owned by a third user (nobody) counts
# every catchable signal it gets. Exit 0 only if the suite passed as `sd` and the decoy got no
# signal and is neither gone nor stopped.
set -u
apk add -q procps bash util-linux-misc setpriv >/dev/null || exit 3
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
# the test executables only (the profile of a plain bin target has "test":false)
bins=$(cargo test --no-run --message-format=json 2>/dev/null | grep '"profile":{[^}]*"test":true' | grep -o '"executable":"[^"]*"' | cut -d'"' -f4)
[ -n "$bins" ] || { echo "no test executables found"; exit 3; }
adduser -D sd 2>/dev/null
mkdir -p /tmp/decoy && chmod 777 /tmp/decoy
/bin/setpriv --reuid=65534 --regid=65534 --clear-groups /t/debug/sr-fixture sigcount /tmp/decoy/rec &
i=0; while [ ! -s /tmp/decoy/rec ] && [ $i -lt 500 ]; do sleep 0.01; i=$((i+1)); done
decoy=$(cut -d' ' -f1 /tmp/decoy/rec)
echo "decoy pid $decoy (uid $(ps -o uid= -p "$decoy" | tr -d ' '))"
# control: the decoy counts a signal. A process of `sd` can reach a process of another user only
# with CONT, and only in its own session (kill(2)); that is the one path the suite could use, so
# the control is exactly that: a CONT sent by `sd`.
/bin/setpriv --reuid=sd --regid=sd --clear-groups kill -CONT "$decoy"
i=0; while [ "$(cat /tmp/decoy/rec.sig 2>/dev/null | wc -l)" -lt 1 ] && [ $i -lt 500 ]; do sleep 0.01; i=$((i+1)); done
before=$(cat /tmp/decoy/rec.sig 2>/dev/null | wc -l)
echo "decoy control: $before signal(s) counted (a CONT from sd)"
[ "$before" -eq 1 ] || { echo "the decoy does not count signals"; exit 3; }
# P4 review: a journal planted as root (owned by root, 0644, in a folder `sd` owns) and the same
# journal owned by `sd` (the control), both naming a decoy of `sd` with its correct identity. The
# leg tag is shared (the runner adopts SHEEPR_LEG_TAG), so the decoy carries the test's tag.
LEGTAG=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
boot=$(cat /proc/sys/kernel/random/boot_id)
pidns=$(stat -L -c '%d.%i' /proc/self/ns/pid)
mkdir -p /tmp/planted && chown sd:sd /tmp/planted
/bin/setpriv --reuid=sd --regid=sd --clear-groups env SHEEPR_TEST_TAG="$LEGTAG" /t/debug/sr-fixture sigcount /tmp/planted/decoy &
i=0; while [ ! -s /tmp/planted/decoy ] && [ $i -lt 500 ]; do sleep 0.01; i=$((i+1)); done
dpid=$(cut -d' ' -f1 /tmp/planted/decoy); did=$(cut -d' ' -f2 /tmp/planted/decoy)
[ -n "$dpid" ] && [ -n "$did" ] || { echo "the planted decoy did not start"; exit 3; }
sduid=$(id -u sd)
for which in foreign own; do
  st=/tmp/planted/$which
  mkdir -p "$st/jobs/$boot-$pidns" && : > "$st/.sheepr-test"
  chown -R sd:sd "$st" && chmod 700 "$st/jobs" "$st/jobs/$boot-$pidns"
  j="$st/jobs/$boot-$pidns/j-0badf00d.journal"
  printf '{"v":1,"kind":"header","job":"j-0badf00d","boot":"%s","pidns":"%s","owner":"default","uid":%s,"sup":{"pid":999999,"id":1},"argv":"sheepr run"}\n{"v":1,"pid":%s,"id":%s,"ppid":1,"pid_id":null,"puniq":null,"cmd":"decoy"}\n' "$boot" "$pidns" "$sduid" "$dpid" "$did" > "$j"
  if [ $which = foreign ]; then chown root:root "$j" && chmod 644 "$j"; else chown sd:sd "$j" && chmod 600 "$j"; fi
done
PLANTED="/tmp/planted/foreign|/tmp/planted/own|$dpid|$did|/tmp/planted/decoy"
rc=0
for b in $bins; do
  # a shell of `sd` stays as the test binary's parent (it does not exec it): a cell that walks
  # its own ancestors needs one of its own user above it
  # through the cargo runner, as cargo test would (PHASE2.md §0.4: the test environment)
  # the shared leg tag (and the planted journals) only for the sweep binary: every other binary
  # keeps a tag of its own, so the wall still keeps it off the planted decoy
  case $(basename "$b") in sweep-*) extra="SR_PLANTED=$PLANTED SHEEPR_LEG_TAG=$LEGTAG" ;; *) extra="SR_NOTHING=1" ;; esac
  timeout 900 /bin/setpriv --reuid=sd --regid=sd --clear-groups env HOME=/tmp TMPDIR=/tmp $extra /bin/sh -c '/w/scripts/test-env "$0"; exit $?' "$b" > /tmp/b.log 2>&1 || rc=1
  echo "$(basename "$b"): $(grep -E '^test result' /tmp/b.log | cut -c1-80)"
  grep -E '^thread|FAILED' /tmp/b.log | cut -c1-200
done
st=$(ps -o stat= -p "$decoy" | tr -d ' ')
sigs=$(cat /tmp/decoy/rec.sig 2>/dev/null | wc -l)
echo "decoy: state=${st:-gone} signals=$sigs (1 is the control)"
kill -KILL "$decoy" 2>/dev/null
# the planted decoy: the control sweep must have ended it; SIGKILL only if it is still that very
# process, by the rule of sheepr's own identity(): the fields after the LAST ')', a zombie is
# gone, else field 22 (the start time) must be the identity the journal named
same_decoy() {
  fields=$(sed 's/.*)//' "/proc/$dpid/stat" 2>/dev/null) || return 1
  set -- $fields
  [ $# -ge 20 ] && [ "$1" != Z ] && [ "${20}" = "$did" ]
}
if same_decoy; then
  echo "planted decoy: ALIVE (the control sweep did not end it)"; kill -KILL "$dpid"; rc=1
else
  echo "planted decoy: gone (the control sweep ended it)"
fi
sleep 0.2
left=$(ps -eo args | grep -cE '^(/bin/sleep 2[0-9]\.|\S*sr-fixture |\S*/sheepr run|sheepr (run|__root))')
echo "leftovers: $left"
[ "$rc" -eq 0 ] && [ -n "$st" ] && [ "${st#T}" = "$st" ] && [ "$sigs" -eq 1 ] && [ "$left" -eq 0 ]
