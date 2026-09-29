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
# untracked file); HEAD is the tag's commit; the tag's Cargo.toml and Cargo.lock say X.Y.Z; the tag's
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
  lv=$(git -C "$root" show "$tag:Cargo.lock" | awk 'p && /^version = /{gsub(/"/,"",$3); print $3; exit} /^name = "sheepdog"$/{p=1}')
  [ "$lv" = "$xyz" ] || die "Cargo.lock records sheepdog $lv, not $xyz (run cargo build to update it, and commit it)"
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

# --- tools, each with its own named environment (PHASE3.md §1.4) -------------------------------
# base: HOME PATH TMPDIR USER LOGNAME (+ DEVELOPER_DIR); cargo adds its own; docker adds its host
# and config. Nothing else of the caller's environment reaches a tool (never RUSTUP_TOOLCHAIN: it
# overrides rust-toolchain.toml).
tool() { # class command...
  cls=$1; shift
  set -- env -i HOME="${HOME:-}" PATH="$PATH" TMPDIR="${TMPDIR:-/tmp}" USER="${USER:-}" LOGNAME="${LOGNAME:-}" \
    ${DEVELOPER_DIR:+DEVELOPER_DIR="$DEVELOPER_DIR"} "$@"
  case $cls in
    cargo) set -- "$@" ;;
    docker) set -- "$@" ;;
    base) ;;
  esac
  "$@"
}
cargo_env() { # CARGO_HOME TARGET_DIR command... (the cargo class)
  ch=$1 td=$2; shift 2
  tool base env CARGO_HOME="$ch" CARGO_TARGET_DIR="$td" MACOSX_DEPLOYMENT_TARGET=12.0 \
    SHEEPDOG_COMMIT_OVERRIDE="$short" ${RUSTUP_HOME:+RUSTUP_HOME="$RUSTUP_HOME"} "$@"
}
docker_env() { # the docker class
  tool base env ${DOCKER_HOST:+DOCKER_HOST="$DOCKER_HOST"} ${DOCKER_CONFIG:+DOCKER_CONFIG="$DOCKER_CONFIG"} docker "$@"
}

# --- the unsigned build (PHASE3.md S2) --------------------------------------------------------
build_unsigned() {
  . "$root/scripts/release.conf" || die "cannot read scripts/release.conf"
  short=$(git -C "$root" rev-parse --short=12 "$commit")
  dest=$out/$tag-unsigned
  [ -e "$dest" ] && die "$dest exists (a release output directory is never reused)"
  mkdir -p "$dest" || die "cannot make $dest"
  scratch=$(mktemp -d /private/tmp/sd-release.XXXXXX 2>/dev/null || mktemp -d) || die "no scratch dir"
  trap 'git -C "$root" worktree remove --force "$scratch/src" >/dev/null 2>&1; rm -rf "$scratch"' EXIT
  trap 'exit 1' HUP INT TERM
  src=$scratch/src
  git -C "$root" worktree add -q --detach "$src" "$tag" || die "cannot make a worktree of $tag"
  [ -z "$(git -C "$src" status --porcelain --ignored)" ] || die "the worktree holds an untracked or ignored file"
  echo "release: building $tag ($short) in $src"

  # macOS: a fresh CARGO_HOME, fetched (Cargo checks each crate against Cargo.lock), then offline
  mkdir -p "$scratch/cargo-mac"
  (cd "$src" && cargo_env "$scratch/cargo-mac" "$scratch/target-mac" cargo fetch --locked -q) || die "cargo fetch (mac)"
  for t in aarch64-apple-darwin x86_64-apple-darwin; do
    (cd "$src" && cargo_env "$scratch/cargo-mac" "$scratch/target-mac" cargo build -q --release --locked --offline --bin sheepdog --target $t) \
      || die "cargo build $t"
  done
  rustc_mac=$(cd "$src" && cargo_env "$scratch/cargo-mac" "$scratch/target-mac" rustc -V) || die "rustc -V (mac)"
  lipo -create -output "$scratch/sheepdog" "$scratch/target-mac/aarch64-apple-darwin/release/sheepdog" \
    "$scratch/target-mac/x86_64-apple-darwin/release/sheepdog" || die "lipo"
  for a in arm64 x86_64; do
    m=$(vtool -arch $a -show-build "$scratch/sheepdog" 2>/dev/null | awk '$1=="minos"{print $2; exit}')
    [ "$m" = 12.0 ] || die "the $a slice's minimum macOS is '$m', not 12.0"
  done
  xyz=${tag#v}; xyz=${xyz%%-*}
  app=$("$root/scripts/bundle.sh" "$scratch/sheepdog" "$scratch/bundle" "$xyz" "$c") || die "bundle.sh"
  mkdir -p "$scratch/rh"
  v=$("$root/scripts/lib/release-run.sh" "$scratch/rh" "$app/Contents/MacOS/sheepdog" --version) || die "the Mac binary does not run"
  case $v in *"$short"*) ;; *) die "the Mac binary names another commit: $v" ;; esac
  "$root/scripts/lib/archive.sh" make "$app" "$dest/sheepdog-macos-universal.tar.gz" || die "archive"
  "$root/scripts/lib/archive.sh" check "$dest/sheepdog-macos-universal.tar.gz" || die "the archive fails its check"

  # Linux: the pinned image's own CARGO_HOME, fetched in the image, then built offline
  amd64=sd-amd64-base:$(printf %s "${SD_IMG_ALPINE##*sha256:}" | cut -c1-12)
  docker_env image inspect "$amd64" >/dev/null 2>&1 || amd64=sd-amd64-alpine:$(printf %s "${SD_IMG_ALPINE##*sha256:}" | cut -c1-12)
  docker_env image inspect "$amd64" >/dev/null 2>&1 || die "no local amd64 image of the pinned base (run ./test-all pull, or ./test-all amd64)"
  for i in arm64 amd64; do
    docker_env image inspect "sd-scratch:empty-$i" >/dev/null 2>&1 && continue
    tar -cf "$scratch/empty.tar" -T /dev/null && docker_env import --platform "linux/$i" "$scratch/empty.tar" "sd-scratch:empty-$i" >/dev/null \
      || die "cannot make the empty image sd-scratch:empty-$i"
  done
  mkdir -p "$scratch/cargo-linux"
  docker_env run --rm --pull=never -v "$src":/src:ro -v "$scratch/cargo-linux":/sdhome -e CARGO_HOME=/sdhome -w /src "$SD_IMG_ALPINE" \
    cargo fetch --locked -q || die "cargo fetch (linux)"
  for a in aarch64 x86_64; do
    case $a in aarch64) img=$SD_IMG_ALPINE pf=linux/arm64 ;; x86_64) img=$amd64 pf=linux/amd64 ;; esac
    mkdir -p "$scratch/target-$a"
    docker_env run --rm --pull=never --network none --platform "$pf" -v "$src":/src:ro -v "$scratch/cargo-linux":/sdhome:ro \
      -v "$scratch/target-$a":/tgt -e CARGO_HOME=/sdhome -e CARGO_TARGET_DIR=/tgt -e SHEEPDOG_COMMIT_OVERRIDE="$short" -w /src "$img" \
      sh -c 'cargo build -q --release --locked --offline --bin sheepdog && rustc -V > /tgt/rustc && uname -m > /tgt/arch && if readelf -l /tgt/release/sheepdog | grep -q INTERP; then echo "a PT_INTERP" >&2; exit 1; fi' \
      || die "the Linux $a build"
    [ "$(cat "$scratch/target-$a/arch")" = "$a" ] || die "the $a build ran as $(cat "$scratch/target-$a/arch")"
    cp "$scratch/target-$a/release/sheepdog" "$dest/sheepdog-linux-$a" || die "copy $a"
    v=$(docker_env run --rm --pull=never --network none --platform "$pf" -v "$dest/sheepdog-linux-$a":/sheepdog:ro "sd-scratch:empty-${pf#linux/}" /sheepdog --version) \
      || die "the $a binary does not run in an empty image (not static?)"
    case $v in *"$short"*) ;; *) die "the $a binary names another commit: $v" ;; esac
  done

  # install.sh (PHASE3.md S3), with this release's version written in
  files="sheepdog-macos-universal.tar.gz sheepdog-linux-aarch64 sheepdog-linux-x86_64"
  if [ -f "$src/scripts/install.sh" ]; then
    sed "s/^SHEEPDOG_VERSION=.*/SHEEPDOG_VERSION=${tag#v}/" "$src/scripts/install.sh" > "$dest/install.sh" && chmod 755 "$dest/install.sh"
    files="$files install.sh"
  fi
  (cd "$dest" && shasum -a 256 $files > SHA256SUMS) || die "SHA256SUMS"
  {
    printf '{\n  "v": 1,\n  "tag": "%s",\n  "commit": "%s",\n  "control": false,\n  "files": [\n' "$tag" "$commit"
    sep=""
    for f in $files; do
      h=$(shasum -a 256 "$dest/$f" | cut -d' ' -f1)
      case $f in
        sheepdog-macos-*) r=$rustc_mac ;;
        sheepdog-linux-*) r=$(cat "$scratch/target-${f#sheepdog-linux-}/rustc") ;;
        *) r="" ;;
      esac
      printf '%s    {"name": "%s", "sha256": "%s"%s}' "$sep" "$f" "$h" "${r:+, \"rustc\": \"$r\"}"
      sep=",
"
    done
    printf '\n  ]\n}\n'
  } > "$dest/MANIFEST.json"
  echo "release: $tag built (unsigned) in $dest"
}

case $sub in
  check) checks ;;
  build)
    checks
    [ $sign = yes ] && die "build --sign is not built yet (PHASE3.md S2c)"
    build_unsigned ;;
  publish) die "publish is not built yet (PHASE3.md S2d)" ;;
  npm-check) die "npm-check is not built yet (PHASE3.md S2e)" ;;
esac
