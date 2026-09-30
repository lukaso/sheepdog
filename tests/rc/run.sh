#!/bin/sh
# The rc leg (PHASE3.md §3): cells that run against the operator's rc output, read only.
#   SD_RC_DIR=<out>/vX.Y.Z-rc.N SD_RC_CONTROL_DIR=<out>/vX.Y.Z-rc.N-control sh tests/rc/run.sh
# Both directories are required (a missing one is a failure, never a skip). The cells copy what
# they change into /private/tmp/sd-p3-fixtures.*; they never write into either directory, never
# touch a keychain, and run a release-ID executable only through the exec door.
set -u
# absolute before the cd (a relative directory would name another place)
for v in SD_RC_DIR SD_RC_CONTROL_DIR; do
  eval "d=\${$v:-}"
  [ -n "$d" ] && [ -d "$d" ] && eval "$v=\$(cd \"\$d\" && pwd -P)" && export "$v"
done
cd "$(dirname "$0")" || exit 3
[ -d "${SD_RC_DIR:-}" ] || { echo "FAIL: SD_RC_DIR is not a directory (${SD_RC_DIR:-unset})"; exit 1; }
[ -d "${SD_RC_CONTROL_DIR:-}" ] || { echo "FAIL: SD_RC_CONTROL_DIR is not a directory (${SD_RC_CONTROL_DIR:-unset})"; exit 1; }
n=0 bad=""
for t in t_*.sh; do
  [ -f "$t" ] || continue
  n=$((n + 1)); echo "== $t"
  timeout -k 10 1800 sh "$t" < /dev/null; r=$?
  [ $r = 0 ] || bad="$bad $t"
done
[ $n -gt 0 ] || { echo "FAIL: no rc cells ran"; exit 1; }
# part 1 only: S3's Mac install cells and S5's cell 18 with the stapled bundle are not built yet
[ -z "$bad" ] && { echo "PASS rc part 1 ($n cells; not built: S3's Mac install, S5's cell 18)"; exit 0; }
echo "RED rc:$bad"; exit 1
