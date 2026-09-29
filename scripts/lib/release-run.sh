#!/bin/sh
# Run a release or rc sheepdog in a cell (PHASE3.md S1; PHASE2.md §0.5 carried over). A release
# build has no test tag latch and no state wall, so the cell is walled here instead:
#
#   release-run.sh HOME-DIR BINARY [SUBCOMMAND ARGS...]
#
# - the environment is cleared (no SHEEPDOG_TEST_*: a release build exits 125 on one; no
#   SHEEPDOG_OUTER: no listener of another test's run is reachable), with PATH=/usr/bin:/bin and
#   HOME, XDG_STATE_HOME, SHEEPDOG_STATE and TMPDIR inside HOME-DIR (never the real home);
# - only run, doctor, ps, strays (without --kill), --version and help are allowed: kill, sweep
#   and strays --kill aim at processes the cell did not start;
# - --inherit-terminal-permissions (before `--`) is refused: its responsible process is the
#   terminal app, whose other processes a defect could reach (PHASE2.md's T is not here);
# - `run` gets --no-sweep;
# - BINARY runs through the exec door (SD_EXEC_RECORD is passed to it, so refusals can be tested).
set -u
usage() { echo "usage: release-run.sh HOME-DIR BINARY [SUBCOMMAND ARGS...]" >&2; exit 2; }
no() { echo "release-run: refused: $*" >&2; exit 2; }
[ $# -ge 2 ] || usage
home=$1 bin=$2; shift 2
[ -d "$home" ] || no "HOME-DIR $home is not a directory"
h=$(cd -P "$home" && pwd -P) || no "cannot resolve $home"
r=$(cd -P "${HOME:-/nonexistent}" 2>/dev/null && pwd -P) || r=
[ -n "$r" ] && [ "$h" = "$r" ] && no "HOME-DIR is the real home"

sub=${1:-}
extra=
case $sub in
  run) extra=--no-sweep ;;
  doctor|ps|strays|--version|-V|help|--help|-h) ;;
  *) no "subcommand '$sub' (only run, doctor, ps, strays, --version and help)" ;;
esac
for a in "$@"; do
  [ "$a" = -- ] && break
  case $a in
    --inherit-terminal-permissions) no "--inherit-terminal-permissions" ;;
    --kill) [ "$sub" = strays ] && no "strays --kill" ;;
  esac
done
[ $# -gt 0 ] && shift
mkdir -p "$h/xdg" "$h/state" "$h/tmp" || exit 1
set -- ${sub:+"$sub"} ${extra:+"$extra"} "$@"
exec env -i PATH=/usr/bin:/bin HOME="$h" XDG_STATE_HOME="$h/xdg" SHEEPDOG_STATE="$h/state" TMPDIR="$h/tmp" \
  ${SD_EXEC_RECORD:+SD_EXEC_RECORD="$SD_EXEC_RECORD"} \
  "$(dirname "$0")/exec-guard.sh" exec "$bin" "$@"
