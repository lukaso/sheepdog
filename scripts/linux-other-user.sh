#!/bin/sh
# The another-user leg of ./test-all, INSIDE a root container: build as root, then run every
# test binary as the unprivileged user `sd`, while a decoy owned by a third user (nobody) counts
# every catchable signal it gets. Exit 0 only if the suite passed as `sd` and the decoy got no
# signal and is neither gone nor stopped.
set -u
apk add -q procps bash util-linux-misc setpriv >/dev/null || exit 3
rm -rf /w && mkdir /w && cd /src && tar cf - --exclude=./target --exclude='./target-*' --exclude=./spike . | (cd /w && tar xf -) && cd /w || exit 3
export CARGO_TARGET_DIR=/tgt
cargo build -q --tests 2>&1 | grep -E '^error' -A6 && exit 3
# the test executables only (the profile of a plain bin target has "test":false)
bins=$(cargo test --no-run --message-format=json 2>/dev/null | grep '"profile":{[^}]*"test":true' | grep -o '"executable":"[^"]*"' | cut -d'"' -f4)
[ -n "$bins" ] || { echo "no test executables found"; exit 3; }
adduser -D sd 2>/dev/null
mkdir -p /tmp/decoy && chmod 777 /tmp/decoy
/bin/setpriv --reuid=65534 --regid=65534 --clear-groups /tgt/debug/sd-fixture sigcount /tmp/decoy/rec &
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
rc=0
for b in $bins; do
  # a shell of `sd` stays as the test binary's parent (it does not exec it): a cell that walks
  # its own ancestors needs one of its own user above it
  # through the cargo runner, as cargo test would (PHASE2.md §0.4: the test environment)
  timeout 900 /bin/setpriv --reuid=sd --regid=sd --clear-groups env HOME=/tmp TMPDIR=/tmp /bin/sh -c '/w/scripts/test-env "$0"; exit $?' "$b" > /tmp/b.log 2>&1 || rc=1
  echo "$(basename "$b"): $(grep -E '^test result' /tmp/b.log | cut -c1-80)"
  grep -E '^thread|FAILED' /tmp/b.log | cut -c1-200
done
st=$(ps -o stat= -p "$decoy" | tr -d ' ')
sigs=$(cat /tmp/decoy/rec.sig 2>/dev/null | wc -l)
echo "decoy: state=${st:-gone} signals=$sigs (1 is the control)"
kill -KILL "$decoy" 2>/dev/null
sleep 0.2
left=$(ps -eo args | grep -cE '^(/bin/sleep 2[0-9]\.|\S*sd-fixture |\S*/sheepdog run|sheepdog (run|__root))')
echo "leftovers: $left"
[ "$rc" -eq 0 ] && [ -n "$st" ] && [ "${st#T}" = "$st" ] && [ "$sigs" -eq 1 ] && [ "$left" -eq 0 ]
