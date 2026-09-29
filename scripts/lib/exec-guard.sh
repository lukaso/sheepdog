#!/bin/sh -p
# The exec door (PHASE3.md §1.1). Every phase-3 cell and release script runs a built executable
# only through this door:
#
#   exec-guard.sh check PATH          exit 0 if PATH may run, 1 if refused (the reason on stderr)
#   exec-guard.sh exec PATH [ARG...]  check, then exec PATH (as given) with the arguments
#
# Why: running a build that carries the release bundle ID but not the release signature switches
# off the user's privacy grant for the real release (measured in phase 0, PLAN.md §4.4). The ID
# travels with every copy of the file, so the door reads it from the file itself, for the path as
# given and for the path with every symlink resolved:
#   (a) the signature's identifier;
#   (b) the CFBundleIdentifier of every enclosing bundle (any .../Contents/MacOS/... in the path,
#       in any case: APFS ignores case, so .../contents/macos/... runs the same file);
#   (c) the embedded __info_plist section of every slice of a universal file;
#   (d) for a `#!` script, the same checks on its interpreter.
# IDs are compared without case and whitespace (fail closed: whether macOS folds them is not
# measured). If any key is the release ID, the file and each such bundle must meet the release
# requirement in scripts/release.conf; otherwise the door refuses. Anything it cannot read counts
# as the release ID.
#
# Nothing in the caller's environment can answer for the door: it runs under `sh -p` (no shell
# functions imported from the environment; if started as `sh exec-guard.sh` it re-runs itself so),
# and it judges in a subshell whose PATH is /usr/bin:/bin. The one input it takes from the
# environment is SD_EXEC_RECORD: when it names a file, `exec` appends "would exec PATH" to it
# instead of running PATH (the test seam; it can only stop a run).
case ${SD_GUARD_P:-} in
  1) unset SD_GUARD_P ;;
  *) SD_GUARD_P=1 exec /bin/sh -p "$0" "$@" ;;
esac
set -u

usage() { echo "usage: exec-guard.sh check|exec PATH [ARG...]" >&2; exit 2; }
[ $# -ge 2 ] || usage
mode=$1 target=$2
case $mode in check) [ $# -eq 2 ] || usage ;; exec) ;; *) usage ;; esac
case $target in /*) typed=$target ;; *) typed=$(pwd -P)/$target ;; esac

(
  PATH=/usr/bin:/bin; export PATH
  conf=$(cd "$(dirname "$0")/.." 2>/dev/null && pwd -P)/release.conf
  . "$conf" || exit 2
  rel=$(printf %s "$SD_RELEASE_ID" | tr -d '[:space:]' | tr '[:upper:]' '[:lower:]')
  tmp=$(mktemp -d /private/tmp/sd-exec-guard.XXXXXX 2>/dev/null || mktemp -d) || exit 2
  trap 'rm -rf "$tmp"' EXIT
  refuse() { echo "sheepdog exec-guard: refused $typed: $*" >&2; exit 1; }
  is_rel() { [ "$(printf %s "$1" | tr -d '[:space:]' | tr '[:upper:]' '[:lower:]')" = "$rel" ]; }
  meets() { /usr/bin/codesign -v -R="$SD_RELEASE_REQUIREMENT" "$1" >/dev/null 2>&1; }

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

  # the bundles that enclose a path: every ancestor .../X/Contents/MacOS (any case). Prints
  # "release" first if any of them says the release ID (or cannot be read), then each bundle.
  bundles() {
    d=$(dirname "$1") out="" relb=no
    while [ "$d" != / ] && [ -n "$d" ] && [ "$d" != . ]; do
      par=$(dirname "$d")
      lb=$(basename "$d" | tr '[:upper:]' '[:lower:]'); lp=$(basename "$par" | tr '[:upper:]' '[:lower:]')
      if [ "$lb" = macos ] && [ "$lp" = contents ]; then
        b=$(dirname "$par")
        if [ -e "$par/Info.plist" ] || [ -L "$par/Info.plist" ]; then
          bid=$(plutil -extract CFBundleIdentifier raw -o - "$par/Info.plist" 2>/dev/null) || bid=$SD_RELEASE_ID
          is_rel "$bid" && relb=yes
        fi
        out="$out
$b"
      fi
      d=$par
    done
    echo "$relb$out"
  }

  judge() { # path depth -> exit 1 (refuse) or return 0
    [ "$2" -le 4 ] || refuse "too many #! interpreters"
    real=$(resolve "$1") || refuse "cannot resolve $1"
    [ -f "$real" ] || refuse "not a file: $1"
    [ "$(uname -s)" = Darwin ] || return 0
    release=no
    # (a)
    id=$(codesign -d -v "$real" 2>&1 | sed -n 's/^Identifier=//p')
    is_rel "$id" && release=yes
    # (b), for the path as given and as resolved
    all=""
    for p in "$1" "$real"; do
      bl=$(bundles "$p")
      case $bl in yes*) release=yes ;; esac
      all="$all
$(printf '%s\n' "$bl" | sed 1d)"
    done
    # (c), every slice
    archs=$(lipo -archs "$real" 2>/dev/null) || archs=""
    # a thin file (not fat) is its own one slice; `lipo -thin` works only on a fat one
    if [ -z "$archs" ] || ! lipo -info "$real" 2>/dev/null | grep -q '^Architectures in the fat file'; then slices=$real
    else
      slices=""
      for a in $archs; do
        lipo -thin "$a" -output "$tmp/slice.$a" "$real" 2>/dev/null || { release=yes; continue; }
        slices="$slices $tmp/slice.$a"
      done
    fi
    for s in $slices; do
      e=$(launchctl plist __TEXT,__info_plist "$s" 2>/dev/null | sed -n 's/.*"CFBundleIdentifier" *= *"\(.*\)";.*/\1/p')
      [ -n "$e" ] && is_rel "$e" && release=yes
    done
    if [ $release = yes ]; then
      meets "$real" || refuse "it carries the release ID ($SD_RELEASE_ID) without the release signature"
      printf '%s\n' "$all" | while IFS= read -r b; do
        [ -n "$b" ] || continue
        meets "$b" || exit 1
      done || refuse "an enclosing bundle lacks the release signature"
    fi
    # (d)
    if [ "$(head -c 2 "$real" 2>/dev/null)" = '#!' ]; then
      interp=$(head -n 1 "$real" | sed 's/^#![[:space:]]*//; s/[[:space:]].*$//')
      [ -n "$interp" ] || refuse "an empty #! line"
      judge "$interp" $(($2 + 1))
    fi
    return 0
  }
  judge "$typed" 0
) || exit 1

[ "$mode" = check ] && exit 0
shift 2
if [ -n "${SD_EXEC_RECORD:-}" ]; then echo "would exec $typed" >> "$SD_EXEC_RECORD"; exit 0; fi
exec "$typed" "$@"
