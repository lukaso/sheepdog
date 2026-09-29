#!/bin/sh
# The first-use eval's check (PLAN.md §10.9): PASS if the escapee ./hang.sh recorded is gone.
d=$(cd "$(dirname "$0")" && pwd -P)
[ -s "$d/escapee" ] || { echo "check: no ./escapee record: hang.sh did not run, or its escapee did not start"; exit 2; }
p=$(sed -n 1p "$d/escapee"); s=$(sed -n 2p "$d/escapee")
case $p in ''|*[!0-9]*) s="" ;; esac
[ -n "$s" ] || { echo "check: the ./escapee record is incomplete (no pid or no start time)"; exit 2; }
if [ -r "/proc/$p/stat" ]; then now=$(sed 's/.*) //' "/proc/$p/stat" | cut -d' ' -f20); else now=$(LC_ALL=C ps -o lstart= -p "$p" 2>/dev/null); fi
if [ -n "$now" ] && [ "$now" = "$s" ]; then echo "check: FAIL: the escapee (pid $p) is still alive"; exit 1; fi
echo "check: PASS: the escapee is gone"
