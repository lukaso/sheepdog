#!/bin/sh
# The exec door (PHASE3.md §1.1). Every phase-3 cell and release script runs a built executable
# only through this door:
#
#   exec-guard.sh check PATH          exit 0 if PATH may run, 1 if refused (the reason on stderr)
#   exec-guard.sh exec PATH [ARG...]  check, then exec PATH with the arguments
#
# Why: running a build that carries the release bundle ID but not the release signature switches
# off the user's privacy grant for the real release (measured in phase 0, PLAN.md §4.4). The ID
# travels with every copy of the file, so the door reads it from the file itself: the signature's
# identifier, the CFBundleIdentifier of an enclosing bundle, and an embedded __info_plist section.
# If any of them is the release ID, the file and its bundle must meet the release requirement in
# scripts/release.conf; otherwise the door refuses. A file with none of the three may run.
#
# The tools are called by absolute path, so nothing on PATH can answer for them. Nothing here can
# be overridden by the environment, except SD_EXEC_RECORD: when it names a file, `exec` appends
# "would exec PATH" to it instead of running PATH (the test seam; it can only stop a run).
set -u

conf_dir=$(cd "$(dirname "$0")/.." && pwd -P) || exit 2
. "$conf_dir/release.conf" || exit 2

refuse() { echo "sheepdog exec-guard: refused $real: $*" >&2; exit 1; }

# the real path, symlinks followed (no `readlink -f` or `realpath`: not on every macOS)
resolve() {
  p=$1 n=0
  while [ -L "$p" ]; do
    n=$((n + 1)); [ $n -le 40 ] || return 1
    l=$(readlink "$p") || return 1
    case $l in /*) p=$l ;; *) p=$(dirname "$p")/$l ;; esac
  done
  d=$(cd -P "$(dirname "$p")" 2>/dev/null && pwd -P) || return 1
  printf '%s/%s\n' "$d" "$(basename "$p")"
}

meets() { # path: 0 if it meets the release requirement
  /usr/bin/codesign -v -R="$SD_RELEASE_REQUIREMENT" "$1" >/dev/null 2>&1
}

check() {
  real=$(resolve "$1") || { real=$1; refuse "cannot resolve the path"; }
  [ -f "$real" ] || refuse "not a file"
  [ "$(uname -s)" = Darwin ] || return 0
  release=no
  # (a) the signature's identifier
  id=$(/usr/bin/codesign -d -v "$real" 2>&1 | sed -n 's/^Identifier=//p')
  [ "$id" = "$SD_RELEASE_ID" ] && release=yes
  # (b) an enclosing bundle's CFBundleIdentifier
  bundle=
  case $real in */Contents/MacOS/*) bundle=${real%%/Contents/MacOS/*} ;; esac
  if [ -n "$bundle" ]; then
    plist=$bundle/Contents/Info.plist
    if [ -e "$plist" ]; then
      # fail closed: an Info.plist the door cannot read counts as the release ID
      bid=$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$plist" 2>/dev/null) || bid=$SD_RELEASE_ID
      [ "$bid" = "$SD_RELEASE_ID" ] && release=yes
    fi
  fi
  # (c) an embedded Info.plist
  if /bin/launchctl plist __TEXT,__info_plist "$real" 2>/dev/null | grep -F "\"$SD_RELEASE_ID\";" >/dev/null; then
    release=yes
  fi
  [ $release = no ] && return 0
  meets "$real" || refuse "it carries the release ID ($SD_RELEASE_ID) without the release signature"
  if [ -n "$bundle" ]; then meets "$bundle" || refuse "its bundle lacks the release signature"; fi
  return 0
}

case ${1:-} in
  check) [ $# -eq 2 ] || { echo "usage: exec-guard.sh check PATH" >&2; exit 2; }
    check "$2" ;;
  exec) [ $# -ge 2 ] || { echo "usage: exec-guard.sh exec PATH [ARG...]" >&2; exit 2; }
    shift; target=$1; shift
    check "$target"
    if [ -n "${SD_EXEC_RECORD:-}" ]; then echo "would exec $real" >> "$SD_EXEC_RECORD"; exit 0; fi
    exec "$real" "$@" ;;
  *) echo "usage: exec-guard.sh check|exec PATH [ARG...]" >&2; exit 2 ;;
esac
