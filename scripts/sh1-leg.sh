#!/bin/sh
# The sh1 leg of ./test-all (PLAN.md cell 29(e), PHASE2.md D8), INSIDE a container whose PID 1 is
# this shell (docker run without --init): a live child of PID 1 and an orphan that PID 1 adopted
# look the same (both have ppid 1), so `strays` marks them `pid1-child` and `strays --kill` skips
# them unless named. Control: named with --pid, the child is killed. Exit 0 only if both hold.
set -u
T=/tgt/debug
[ "$$" = 1 ] || { echo "this shell is not PID 1 ($$)"; exit 3; }
# the test environment: every process here carries the tag, so the wall lets kill act
export SHEEPDOG_TEST_TAG=sh1leg
W=sdstrshone
D=/tmp/$W
mkdir -p $D
$T/sd-fixture sigcount $D/child &
$T/sd-fixture stray $D orphan
i=0; while [ ! -s $D/child ] && [ $i -lt 500 ]; do sleep 0.02; i=$((i+1)); done
child=$(cut -d' ' -f1 $D/child); cid=$(cut -d' ' -f2 $D/child)
orphan=$(cut -d' ' -f1 $D/orphan)
$T/sheepdog strays --json --cmd $W > $D/list
echo "listed: $(wc -l < $D/list) row(s)"; cat $D/list | cut -c1-200
grep -q "\"pid\":$child,.*\"pid1_child\":true" $D/list || { echo "FAIL: the child of PID 1 is not listed as pid1-child"; exit 1; }
# strays never lists its own process (here a child of PID 1, so it would match)
if grep -q '"cmd":"[^"]*sheepdog strays' $D/list; then echo "FAIL: strays listed itself"; exit 1; fi
$T/sheepdog strays --kill --yes --cmd $W; echo "strays --kill: exit $?"
sleep 1
rc=0
if [ -d /proc/$child ] && [ ! -s $D/child.sig ]; then echo "child of PID 1: alive, no signal"; else echo "FAIL: the child of PID 1 was signalled or killed"; rc=1; fi
if [ -d /proc/$orphan ]; then echo "adopted orphan: alive (skipped as well)"; else echo "FAIL: the adopted orphan was killed without being named"; rc=1; fi
# control: named, the child is killed
$T/sheepdog strays --kill --yes --pid "$child:$cid" --cmd $W; echo "named: exit $?"
i=0; while [ -d /proc/$child ] && [ "$(cut -d' ' -f3 /proc/$child/stat 2>/dev/null)" != Z ] && [ $i -lt 250 ]; do sleep 0.02; i=$((i+1)); done
if [ -d /proc/$child ] && [ "$(cut -d' ' -f3 /proc/$child/stat)" != Z ]; then echo "FAIL (control): named with --pid, the child survived"; rc=1; else echo "control: named, the child is gone"; fi
exit $rc
