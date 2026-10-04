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
# given, the path with `.`, `..` and empty parts removed, and the path with every symlink resolved:
#   (a) the signature's identifier, of every slice of a universal file;
#   (b) the CFBundleIdentifier of every enclosing bundle (any .../Contents/MacOS/... in the path,
#       in any case: APFS ignores case) and of an Info.plist beside the file (a flat bundle:
#       CFBundle takes it as the main bundle);
#   (c) the embedded __info_plist section of every slice (a section that is there but cannot be
#       parsed counts as the release ID);
#   (d) for a `#!` script, the same checks on every word of the #! line that is a path (so
#       `#!/usr/bin/env FILE` is judged too). What the script's body runs is not judged, nor what a
#       bare name after env (`#!/usr/bin/env NAME`) finds on PATH when the script runs.
# IDs are compared without case and whitespace (fail closed: whether macOS folds them is not
# measured). If any key is the release ID, the file and each such bundle must meet the release
# requirement in scripts/release.conf; otherwise the door refuses. What it cannot read counts as
# the release ID.
#
# Nothing in the caller's environment can answer for the door: it runs under `sh -p` (bash then
# imports no shell functions from the environment; started as `bash exec-guard.sh`, it re-runs
# itself so), and it judges in a subshell with PATH=/usr/bin:/bin and no DEVELOPER_DIR, SDKROOT or
# TOOLCHAINS (/usr/bin/lipo follows DEVELOPER_DIR). The one input it takes from the environment is
# SR_EXEC_RECORD: when it names a file, `exec` appends "would exec PATH" to it instead of running
# PATH (the test seam; it can only stop a run).
# Stated limits: call the door by its path. `bash exec-guard.sh` with an exported `exec` function
# or a BASH_ENV runs the caller's code before the door can re-run itself; no script can defend
# that. The file can change between the judgement and the exec.
case ${BASH_VERSION:-} in
  '') ;;
  *) case $- in *p*) ;; *) exec /bin/sh -p "$0" "$@" ;; esac ;;
esac
set -u

usage() { echo "usage: exec-guard.sh check|exec PATH [ARG...]" >&2; exit 2; }
[ $# -ge 2 ] || usage
mode=$1 target=$2
case $mode in check) [ $# -eq 2 ] || usage ;; exec) ;; *) usage ;; esac
case $target in /*) typed=$target ;; *) typed=$(pwd -P)/$target ;; esac

(
  PATH=/usr/bin:/bin; export PATH
  unset DEVELOPER_DIR SDKROOT TOOLCHAINS
  # lipo is an xcrun shim: it fails when the Xcode licence is not accepted or no developer dir is
  # set, and a Mach-O file it cannot read then counts as the release ID (below)
  LIPO=/usr/bin/lipo
  conf=$(cd "$(dirname "$0")/.." 2>/dev/null && pwd -P)/release.conf
  . "$conf" || exit 2
  rel=$(printf %s "$SR_RELEASE_ID" | tr -d '[:space:]' | tr '[:upper:]' '[:lower:]')
  # the temp dir is named and the traps set before it is made, so a signal at any point finds a
  # trap that knows it (only SIGKILL can leave it behind)
  tmp=/private/tmp/sr-exec-guard.$$.$(od -An -N4 -tx4 /dev/urandom | tr -d ' ')
  [ -d /private/tmp ] || tmp=/tmp/sr-exec-guard.$$.$(od -An -N4 -tx4 /dev/urandom | tr -d ' ')
  trap 'rm -rf "$tmp"' EXIT
  trap 'rm -rf "$tmp"; exit 1' HUP INT TERM
  mkdir -m 700 "$tmp" || exit 2
  nl='
'
  refuse() { echo "sheepr exec-guard: refused $typed: $*" >&2; exit 1; }
  is_rel() { [ "$(printf %s "$1" | tr -d '[:space:]' | tr '[:upper:]' '[:lower:]')" = "$rel" ]; }
  meets() { /usr/bin/codesign -v -R="$SR_RELEASE_REQUIREMENT" "$1" >/dev/null 2>&1; }

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
  # an absolute path with `.` and empty parts dropped and `..` taken lexically
  lexical() {
    out="" IFS_=$IFS; IFS=/
    set -f; set -- $1; set +f; IFS=$IFS_
    for c in "$@"; do
      case $c in ''|.) ;; ..) out=${out%/*} ;; *) out=$out/$c ;; esac
    done
    printf '%s\n' "${out:-/}"
  }
  plist_rel() { # Info.plist -> 0 if it says the release ID (or cannot be read)
    bid=$(plutil -extract CFBundleIdentifier raw -o - "$1" 2>/dev/null) || return 0
    is_rel "$bid"
  }

  # the bundles that enclose a path: every ancestor .../X/Contents/MacOS (any case). Prints
  # "yes" or "no" (does any say the release ID, or cannot be read), then each bundle.
  bundles() {
    d=$(dirname "$1") out="" relb=no
    # a flat bundle: an Info.plist beside the file
    if [ -e "$d/Info.plist" ] || [ -L "$d/Info.plist" ]; then plist_rel "$d/Info.plist" && relb=yes; out="$out$nl$d"; fi
    while [ "$d" != / ] && [ -n "$d" ] && [ "$d" != . ]; do
      par=$(dirname "$d")
      lb=$(basename "$d" | tr '[:upper:]' '[:lower:]'); lp=$(basename "$par" | tr '[:upper:]' '[:lower:]')
      if [ "$lb" = macos ] && [ "$lp" = contents ]; then
        if [ -e "$par/Info.plist" ] || [ -L "$par/Info.plist" ]; then plist_rel "$par/Info.plist" && relb=yes; fi
        out="$out$nl$(dirname "$par")"
      fi
      d=$par
    done
    printf '%s%s\n' "$relb" "$out"
  }

  judge() { # path depth -> exit 1 (refuse) or return 0
    jp=$1 jd=$2
    [ "$jd" -le 4 ] || refuse "too many #! interpreters"
    real=$(resolve "$jp") || refuse "cannot resolve $jp"
    [ -f "$real" ] || refuse "not a file: $jp"
    [ "$(uname -s)" = Darwin ] || return 0
    release=no why=""
    mark() { release=yes; why="${why:+$why; }$1"; }
    # a file the door cannot read (mode 0111: the kernel can still exec it) counts as release
    [ -r "$real" ] || mark "the door cannot read the file"
    # the slices of a universal file, each thinned into its own file (a thin file is its own)
    # Mach-O by its magic bytes (thin 32/64-bit either endian, fat, fat64)
    archs="" macho=no
    case $(od -An -N4 -tx1 "$real" 2>/dev/null | tr -d ' \n') in
      feedface|cefaedfe|feedfacf|cffaedfe|cafebabe|bebafeca|cafebabf|bfbafeca) macho=yes ;;
    esac
    if [ $macho = yes ]; then
      if info=$("$LIPO" -info "$real" 2>/dev/null); then
        if printf '%s\n' "$info" | grep -q '^Architectures in the fat file'; then
          archs=$("$LIPO" -archs "$real" 2>/dev/null) || mark "lipo cannot list the slices"
          [ -n "$archs" ] || mark "lipo lists no slices"
        fi
      else
        mark "lipo cannot read this Mach-O file (is the Xcode licence accepted? is a developer directory set?)"
      fi
    fi
    # (a)
    if [ -n "$archs" ]; then
      for a in $archs; do
        id=$(codesign -d -v -a "$a" "$real" 2>&1 | sed -n 's/^Identifier=//p')
        is_rel "$id" && mark "the $a slice's signature identifier is the release ID"
      done
    else
      id=$(codesign -d -v "$real" 2>&1 | sed -n 's/^Identifier=//p')
      is_rel "$id" && mark "its signature identifier is the release ID"
    fi
    # (b), for each spelling of the path
    all=""
    for p in "$jp" "$(lexical "$jp")" "$real"; do
      bl=$(bundles "$p")
      case $bl in yes*) mark "an Info.plist of an enclosing or beside bundle says the release ID (or cannot be read)" ;; esac
      all="$all$nl$(printf '%s\n' "$bl" | sed 1d)"
    done
    # (c), every slice of a Mach-O file (lipo reads only Mach-O; a script has no section)
    set --
    if [ $macho = no ]; then :
    elif [ -n "$archs" ]; then
      for a in $archs; do
        "$LIPO" -thin "$a" -output "$tmp/slice.$a" "$real" 2>/dev/null || { mark "lipo cannot thin the $a slice"; continue; }
        set -- "$@" "$tmp/slice.$a"
      done
    else
      set -- "$real"
    fi
    for s in "$@"; do
      out=$(launchctl plist __TEXT,__info_plist "$s" 2>"$tmp/err")
      if [ -n "$out" ]; then
        # every CFBundleIdentifier line (a nested dict may hold one too): any release ID counts
        e=$(printf '%s\n' "$out" | sed -n 's/^[[:space:]]*"CFBundleIdentifier" *= *"\(.*\)";[[:space:]]*$/\1/p')
        # a key the sed cannot parse (a value split over lines) must not hide behind one it can
        nk=$(printf '%s\n' "$out" | grep -c '"CFBundleIdentifier"'); nv=$(printf '%s' "$e" | grep -c .)
        if [ -z "$e" ] || [ "$nv" -lt "$nk" ]; then mark "an embedded Info.plist identifier the door cannot parse"
        else
          printf '%s\n' "$e" | { rel_any=no; while IFS= read -r v; do is_rel "$v" && rel_any=yes; done; [ $rel_any = yes ]; } && mark "an embedded Info.plist says the release ID"
        fi
      elif ! grep -q 'does not have a __TEXT,__info_plist' "$tmp/err"; then
        mark "an embedded Info.plist section launchctl cannot read"
      fi
    done
    if [ $release = yes ]; then
      meets "$real" || refuse "$why, and it lacks the release signature ($SR_RELEASE_ID requirement)"
      printf '%s\n' "$all" | while IFS= read -r b; do
        [ -n "$b" ] || continue
        meets "$b" || exit 1
      done || refuse "an enclosing bundle lacks the release signature"
    fi
    # (d)
    if [ "$(head -c 2 "$real" 2>/dev/null)" = '#!' ]; then
      line=$(head -n 1 "$real" | sed 's/^#!//')
      found=no
      set -f
      for w in $line; do
        # a subshell: judge's variables are global, and a refusal exits it
        case $w in /*) found=yes; ( judge "$w" $((jd + 1)) ) || exit 1 ;; esac
      done
      set +f
      [ $found = yes ] || refuse "a #! line with no absolute interpreter"
    fi
    return 0
  }
  judge "$typed" 0
) || exit 1

[ "$mode" = check ] && exit 0
shift 2
if [ -n "${SR_EXEC_RECORD:-}" ]; then echo "would exec $typed" >> "$SR_EXEC_RECORD"; exit 0; fi
exec "$typed" "$@"
