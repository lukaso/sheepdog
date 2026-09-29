#!/bin/sh
# The sheepdog release (PHASE3.md S2). Run from a clean checkout of the tag.
#
#   release.sh check vX.Y.Z[-rc.N]                    the checks only
#   release.sh build [--out DIR] vTAG                 the unsigned artifacts (the agent may run it)
#   release.sh build --sign [--no-notarize] [--out DIR] vTAG
#                                                     the signed, notarized release (the operator)
#   release.sh publish [--out DIR] vTAG               publish a built release (the operator)
#   release.sh npm-check [--out DIR] vTAG             check the npm tarballs before `npm publish`
#
# The checks (check, build): the tag is vX.Y.Z or vX.Y.Z-rc.N; the tree is clean (no change, no
# untracked file); HEAD is the tag's commit; the tag's Cargo.toml version is X.Y.Z; the tag's
# scripts/release.conf SD_BUILD_COUNTER is above the previous tag's (the highest tag below this
# one, an -rc sorting before its final).
#
# `build --sign` and `publish` refuse under the test environment (any SHEEPDOG_TEST_* variable) and
# without a terminal (stdin and /dev/tty), and read the typed tag from /dev/tty. That is a guard
# against mistakes only: a pty passes it (PHASE3.md §1.2). The gate against an unattended Apple
# submission is the keychain dialog (D2 step 3).
#
# Exit codes: 0 done; 1 a check failed or a step failed; 2 usage; 3 refused in a test environment;
# 4 refused without a terminal (or the typed tag did not match).
set -u
root=$(cd "$(dirname "$0")/.." && pwd -P) || exit 1
usage() { echo "usage: release.sh check|build|publish|npm-check [--sign] [--no-notarize] [--out DIR] vTAG" >&2; exit 2; }
die() { echo "release: $*" >&2; exit 1; }

[ $# -ge 1 ] || usage
sub=$1; shift
sign=no nonot=no out=${HOME:-}/sheepdog-release tag=""
while [ $# -gt 0 ]; do
  case $1 in
    --sign) sign=yes ;;
    --no-notarize) nonot=yes ;;
    --out) [ $# -ge 2 ] || usage; out=$2; shift ;;
    -*) usage ;;
    *) [ -z "$tag" ] || usage; tag=$1 ;;
  esac
  shift
done
[ -n "$tag" ] || usage
case $sub in
  check|build|publish|npm-check) ;;
  *) usage ;;
esac
[ $sign = no ] || [ "$sub" = build ] || usage
[ $nonot = no ] || [ $sign = yes ] || usage

# the gate of the two operator-only entries
if { [ "$sub" = build ] && [ $sign = yes ]; } || [ "$sub" = publish ]; then
  if env | grep -q '^SHEEPDOG_TEST_'; then
    echo "release: refused: a test environment (SHEEPDOG_TEST_*) is set" >&2; exit 3
  fi
  if ! [ -t 0 ] || ! (: < /dev/tty) 2>/dev/null; then
    echo "release: refused: $sub${sign:+ --sign} needs a terminal" >&2; exit 4
  fi
  printf 'release: %s %s. Type the tag to go on: ' "$sub" "$tag" > /dev/tty
  IFS= read -r answer < /dev/tty || exit 4
  [ "$answer" = "$tag" ] || { echo "release: not confirmed" >&2; exit 4; }
fi

# --- the checks ---------------------------------------------------------------------------------
tagre='^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-rc\.[1-9][0-9]*)?$'
valid() { printf '%s\n' "$1" | grep -Eq "$tagre" && [ "$(printf '%s' "$1" | wc -l | tr -d ' ')" = 0 ]; }
counter_at() { # tag -> its SD_BUILD_COUNTER (empty if none)
  git -C "$root" show "$1:scripts/release.conf" 2>/dev/null | sed -n 's/^SD_BUILD_COUNTER=\([0-9][0-9]*\)$/\1/p' | head -1
}
checks() {
  valid "$tag" || die "the tag '$tag' is not vX.Y.Z or vX.Y.Z-rc.N"
  commit=$(git -C "$root" rev-parse -q --verify "refs/tags/$tag^{commit}") || die "no tag $tag"
  [ -z "$(git -C "$root" status --porcelain)" ] || die "the tree is not clean (a change or an untracked file)"
  [ "$(git -C "$root" rev-parse HEAD)" = "$commit" ] || die "HEAD is not $tag"
  xyz=${tag#v}; xyz=${xyz%%-*}
  v=$(git -C "$root" show "$tag:Cargo.toml" | sed -n 's/^version = "\(.*\)"$/\1/p' | head -1)
  [ "$v" = "$xyz" ] || die "Cargo.toml says $v, the tag $xyz"
  c=$(counter_at "$tag"); [ -n "$c" ] && [ "$c" -ge 1 ] || die "the tag has no SD_BUILD_COUNTER >= 1 in scripts/release.conf"
  prev=""
  for t in $(git -C "$root" -c versionsort.suffix=-rc tag -l 'v*' --sort=v:refname); do
    [ "$t" = "$tag" ] && break
    valid "$t" && prev=$t
  done
  if [ -n "$prev" ]; then
    pc=$(counter_at "$prev")
    [ -z "$pc" ] || [ "$c" -gt "$pc" ] || die "the build counter $c is not above $prev's ($pc)"
  fi
  echo "release: $tag checks passed (commit $commit, build $c${prev:+, previous $prev})"
}

case $sub in
  check) checks ;;
  build) checks; die "build is not built yet (PHASE3.md S2b)" ;;
  publish) die "publish is not built yet (PHASE3.md S2d)" ;;
  npm-check) die "npm-check is not built yet (PHASE3.md S2e)" ;;
esac
