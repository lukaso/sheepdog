#!/bin/sh
# Run a release or rc sheepdog in a cell (PHASE3.md S1; PHASE2.md §0.5 carried over). A release
# build has no test tag latch and no state wall, so the cell is walled here instead:
#
#   release-run.sh HOME-DIR BINARY [SUBCOMMAND ARGS...]
#
# - the environment is cleared (no SHEEPDOG_TEST_*: a release build exits 125 on one; no
#   SHEEPDOG_OUTER: no listener of another test's run is reachable), with PATH=/usr/bin:/bin and
#   HOME, XDG_STATE_HOME, SHEEPDOG_STATE and TMPDIR inside HOME-DIR, which must be under a temp
#   root and not be, hold or lie inside the real home (below);
# - only run, doctor, ps, strays (without --kill), --version and help are allowed: kill, sweep
#   and strays --kill aim at processes the cell did not start;
# - --inherit-terminal-permissions (before `--`) is refused: its responsible process is the
#   terminal app, whose other processes a defect could reach (PHASE2.md's T is not here);
# - `run` gets --no-sweep;
# - BINARY runs through the exec door (SD_EXEC_RECORD is passed to it, so refusals can be tested).
set -u
# the tools by name from the system dirs only (Alpine has no /usr/bin/uname or /usr/bin/sed)
PATH=/usr/bin:/bin; export PATH
usage() { echo "usage: release-run.sh HOME-DIR BINARY [SUBCOMMAND ARGS...]" >&2; exit 2; }
no() { echo "release-run: refused: $*" >&2; exit 2; }
[ $# -ge 2 ] || usage
home=$1 bin=$2; shift 2
[ -d "$home" ] || no "HOME-DIR $home is not a directory"
h=$(cd -P "$home" && pwd -P) || no "cannot resolve $home"
# HOME-DIR rules. The real home comes from the account database ($HOME can be anything), and it is
# compared by device and inode, not by spelling (/USERS/..., /System/Volumes/Data/... and a symlink
# all name the same folder). HOME-DIR may not be the real home or inside it (exit 4), may not hold
# it (exit 5: a job there could reach it), and must be under a temp root (exit 3).
# SD_RELEASE_RUN_DRY=1 stops after these checks and before any write (the cells use it).
u=$(id -un)
r=$(dscl . -read "/Users/$u" NFSHomeDirectory 2>/dev/null | sed -n 's/^NFSHomeDirectory: *//p')
[ -n "$r" ] || r=$(getent passwd "$u" 2>/dev/null | cut -d: -f6)
[ -n "$r" ] && [ -d "$r" ] || no "cannot read the real home of $u"
ino() { # path -> device:inode
  case $(uname -s) in Darwin) stat -f %d:%i "$1" ;; *) stat -c %d:%i "$1" ;; esac
}
under() { # dir ancestor-inode -> 0 if dir is that folder or inside it
  d=$1
  while :; do
    [ "$(ino "$d")" = "$2" ] && return 0
    [ "$d" = / ] && return 1
    d=$(dirname "$d")
  done
}
ri=$(ino "$r") || no "cannot stat the real home"
hi=$(ino "$h") || no "cannot stat HOME-DIR"
under "$h" "$ri" && { echo "release-run: refused: HOME-DIR is the real home or inside it" >&2; exit 4; }
under "$r" "$hi" && { echo "release-run: refused: HOME-DIR holds the real home" >&2; exit 5; }
case $(uname -s) in Darwin) roots="/private/tmp/ /private/var/folders/" ;; *) roots="/tmp/" ;; esac
ok=no
for t in $roots; do case $h/ in "$t"*) ok=yes ;; esac; done
[ $ok = yes ] || { echo "release-run: refused: HOME-DIR is not under a temp root ($roots)" >&2; exit 3; }
[ "${SD_RELEASE_RUN_DRY:-}" = 1 ] && { echo "release-run: checks passed"; exit 0; }

sub=${1:-}
extra=
case $sub in
  run) extra=--no-sweep ;;
  doctor|ps|strays|--version|-V|help|--help|-h) ;;
  *) no "subcommand '$sub' (only run, doctor, ps, strays, --version and help)" ;;
esac
# `--` ends sheepdog's own flags only for `run`; strays takes it as a value (`--cmd --`), so for
# every other subcommand the whole argv is checked
for a in "$@"; do
  [ "$a" = -- ] && [ "$sub" = run ] && break
  case $a in
    --inherit-terminal-permissions) no "--inherit-terminal-permissions" ;;
    --kill) [ "$sub" = strays ] && no "strays --kill" ;;
  esac
done
[ $# -gt 0 ] && shift
mkdir -p "$h/xdg" "$h/state" "$h/tmp" || exit 1
set -- ${sub:+"$sub"} ${extra:+"$extra"} "$@"
exec /usr/bin/env -i PATH=/usr/bin:/bin HOME="$h" XDG_STATE_HOME="$h/xdg" SHEEPDOG_STATE="$h/state" TMPDIR="$h/tmp" \
  ${SD_EXEC_RECORD:+SD_EXEC_RECORD="$SD_EXEC_RECORD"} \
  "$(dirname "$0")/exec-guard.sh" exec "$bin" "$@"
