#!/bin/sh
# The sheepr release (PHASE3.md S2). Run from a clean checkout of the tag.
#
#   release.sh check vX.Y.Z[-rc.N]                    the checks only
#   release.sh build [--out DIR] vTAG                 the unsigned artifacts (the agent may run it)
#   release.sh build --sign [--no-notarize] [--out DIR] vTAG
#                                                     the signed, notarized release (the operator)
#   release.sh npm-check [--out DIR] vTAG             check the npm tarballs; first (publish needs it)
#   release.sh publish [--out DIR] vTAG               publish a built release on GitHub (the operator)
#   release.sh publish-npm [--out DIR] vTAG           publish the four npm packages (the operator)
#   release.sh verify [--out DIR] vTAG                the release's facts from its archive (read-only)
#
# The checks (check, build): the tag is vX.Y.Z or vX.Y.Z-rc.N; the tree is clean (no change, no
# untracked file); HEAD is the tag's commit; the tag's Cargo.toml and Cargo.lock say X.Y.Z; the tag's
# scripts/release.conf SR_BUILD_COUNTER is above the previous tag's (the highest tag below this
# one, an -rc sorting before its final).
#
# `build --sign`, `publish` and `publish-npm` refuse under the test environment (any SHEEPR_TEST_* variable) and
# without a terminal (stdin and /dev/tty), and read the typed tag from /dev/tty. That is a guard
# against mistakes only: a pty passes it (PHASE3.md §1.2). The gate against an unattended Apple
# submission is the notary keychain's own password, asked for by sign.sh's unlock (D2 step 3).
#
# Exit codes: 0 done; 1 a check failed or a step failed; 2 usage; 3 refused in a test environment;
# 4 refused without a terminal (or the typed tag did not match).
set -u
# one collation and one message language for every sort and comparison (the operator's locale sorts
# "SHA256SUMS" after "install.sh"; measured)
LC_ALL=C; export LC_ALL
DRYHOME=""   # set only by __publish-dry; never inherited (its EXIT trap removes it)
root=$(cd "$(dirname "$0")/.." && pwd -P) || exit 1
usage() { echo "usage: release.sh check|build|npm-check|publish|publish-npm|verify [--sign] [--no-notarize] [--out DIR] vTAG" >&2; exit 2; }
die() { echo "release: $*" >&2; exit 1; }

[ $# -ge 1 ] || usage
sub=$1; shift
sign=no nonot=no out=${HOME:-}/sheepr-release tag=""
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
  check|build|publish|publish-npm|npm-check|verify|__publish-dry|__publish-npm-dry|__npm-env) ;;
  *) usage ;;
esac
[ $sign = no ] || [ "$sub" = build ] || usage
[ $nonot = no ] || [ $sign = yes ] || usage

# the gate of the two operator-only entries
if { [ "$sub" = build ] && [ $sign = yes ]; } || [ "$sub" = publish ] || [ "$sub" = publish-npm ]; then
  # the system's env and grep, the grep with no environment: neither a tool on PATH nor a variable
  # it reads (GREP_OPTIONS) can hide the test environment; only grep's "no match" (1) lets it on,
  # so a grep that fails refuses too
  /usr/bin/env | /usr/bin/env -i /usr/bin/grep -q '^SHEEPR_TEST_'; g=$?
  if [ $g != 1 ]; then
    echo "release: refused: a test environment (SHEEPR_TEST_*) is set$( [ $g = 0 ] || echo ", or it cannot be checked (grep exit $g)")" >&2; exit 3
  fi
  if ! [ -t 0 ] || ! (: < /dev/tty) 2>/dev/null; then
    echo "release: refused: $sub$( [ $sign = yes ] && echo ' --sign') needs a terminal" >&2; exit 4
  fi
  printf 'release: %s %s. Type the tag to go on: ' "$sub" "$tag" > /dev/tty
  IFS= read -r answer < /dev/tty || exit 4
  [ "$answer" = "$tag" ] || { echo "release: not confirmed" >&2; exit 4; }
fi

# --- the checks ---------------------------------------------------------------------------------
tagre='^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-rc\.[1-9][0-9]*)?$'
valid() { printf '%s\n' "$1" | grep -Eq "$tagre" && [ "$(printf '%s' "$1" | wc -l | tr -d ' ')" = 0 ]; }
counter_at() { # tag -> its SR_BUILD_COUNTER (empty if none)
  git -C "$root" show "$1:scripts/release.conf" 2>/dev/null | sed -n 's/^SR_BUILD_COUNTER=\([0-9][0-9]*\)$/\1/p' | head -1
}
checks() {
  valid "$tag" || die "the tag '$tag' is not vX.Y.Z or vX.Y.Z-rc.N"
  commit=$(git -C "$root" rev-parse -q --verify "refs/tags/$tag^{commit}") || die "no tag $tag"
  [ -z "$(git -C "$root" status --porcelain)" ] || die "the tree is not clean (a change or an untracked file)"
  [ "$(git -C "$root" rev-parse HEAD)" = "$commit" ] || die "HEAD is not $tag"
  xyz=${tag#v}; xyz=${xyz%%-*}
  v=$(git -C "$root" show "$tag:Cargo.toml" | sed -n 's/^version = "\(.*\)"$/\1/p' | head -1)
  [ "$v" = "$xyz" ] || die "Cargo.toml says $v, the tag $xyz"
  lv=$(git -C "$root" show "$tag:Cargo.lock" | awk 'p && /^version = /{gsub(/"/,"",$3); print $3; exit} /^name = "sheepr"$/{p=1}')
  [ "$lv" = "$xyz" ] || die "Cargo.lock records sheepr $lv, not $xyz (run cargo build to update it, and commit it)"
  # a final release's CHANGELOG entry carries its date (an rc's may still say "not yet released")
  case $tag in *-rc.*) ;; *)
    xre=$(printf %s "$xyz" | sed 's/\./\\./g')
    cl=$(git -C "$root" show "$tag:CHANGELOG.md" 2>/dev/null)
    n=$(printf '%s\n' "$cl" | grep -Ec "^## $xre( |\$)")
    d=$(printf '%s\n' "$cl" | grep -Ec "^## $xre \(20[0-9]{2}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])\)\$")
    [ "$n" = 1 ] && [ "$d" = 1 ] || die "CHANGELOG.md needs exactly one '## $xyz (YYYY-MM-DD)' heading for $xyz, with a YYYY-MM-DD date (found $n heading(s) for $xyz, $d of them dated so; a trailing space or CR leaves one undated): fix it, commit, and tag again" ;;
  esac
  c=$(counter_at "$tag"); [ -n "$c" ] && [ "$c" -ge 1 ] || die "the tag has no SR_BUILD_COUNTER >= 1 in scripts/release.conf"
  prev=""
  for t in $(git -C "$root" -c versionsort.suffix=-rc tag -l 'v*' --sort=v:refname); do
    [ "$t" = "$tag" ] && break
    valid "$t" && prev=$t
  done
  if [ -n "$prev" ]; then
    pc=$(counter_at "$prev")
    [ -n "$pc" ] || die "the previous tag $prev has no readable SR_BUILD_COUNTER in scripts/release.conf (a tag from before a rename keeps it under another name): delete that tag, or tag a commit whose counter can be read"
    [ "$c" -gt "$pc" ] || die "the build counter $c is not above $prev's ($pc)"
  fi
  echo "release: $tag checks passed (commit $commit, build $c${prev:+, previous $prev})"
}

# --- tools, each with its own named environment (PHASE3.md §1.4) -------------------------------
# base: HOME PATH TMPDIR USER LOGNAME (+ DEVELOPER_DIR); cargo adds its own; docker adds its host
# and config. Nothing else of the caller's environment reaches a tool (never RUSTUP_TOOLCHAIN: it
# overrides rust-toolchain.toml).
tool() { # class command... (base: HOME PATH TMPDIR USER LOGNAME [DEVELOPER_DIR]; net: base and the
          # git/gh transport: SSH_AUTH_SOCK GIT_SSH_COMMAND GH_CONFIG_DIR, never GH_TOKEN, since gh
          # uses its own stored login)
  cls=$1; shift
  case $cls in
    base) /usr/bin/env -i HOME="${HOME:-}" PATH="$PATH" TMPDIR="${TMPDIR:-/tmp}" USER="${USER:-}" LOGNAME="${LOGNAME:-}" \
            ${DEVELOPER_DIR:+DEVELOPER_DIR="$DEVELOPER_DIR"} "$@" ;;
    net) /usr/bin/env -i HOME="${HOME:-}" PATH="$PATH" TMPDIR="${TMPDIR:-/tmp}" USER="${USER:-}" LOGNAME="${LOGNAME:-}" \
            ${SSH_AUTH_SOCK:+SSH_AUTH_SOCK="$SSH_AUTH_SOCK"} ${GIT_SSH_COMMAND:+GIT_SSH_COMMAND="$GIT_SSH_COMMAND"} \
            ${GH_CONFIG_DIR:+GH_CONFIG_DIR="$GH_CONFIG_DIR"} ${XDG_CONFIG_HOME:+XDG_CONFIG_HOME="$XDG_CONFIG_HOME"} \
            ${HTTPS_PROXY:+HTTPS_PROXY="$HTTPS_PROXY"} ${https_proxy:+https_proxy="$https_proxy"} \
            ${HTTP_PROXY:+HTTP_PROXY="$HTTP_PROXY"} ${http_proxy:+http_proxy="$http_proxy"} \
            ${ALL_PROXY:+ALL_PROXY="$ALL_PROXY"} ${all_proxy:+all_proxy="$all_proxy"} \
            ${NO_PROXY:+NO_PROXY="$NO_PROXY"} ${no_proxy:+no_proxy="$no_proxy"} "$@" ;;
    # npm: its login is the operator's own npmrc (~/.npmrc, or where NPM_CONFIG_USERCONFIG says),
    # never a token from the environment; the dry run's npm gets a temp HOME and no npmrc
    npm) npm_env "${HOME:-}" "${NPM_CONFIG_USERCONFIG:-${npm_config_userconfig:-}}" "$@" ;;
    npmdry) npm_env "$DRYHOME" "" "$@" ;;
    # the dry publish: a fresh temp HOME and nothing else of the caller's, so even a real gh reached
    # through a stand-in would have no login
    dry) /usr/bin/env -i HOME="$DRYHOME" PATH="$PATH" TMPDIR="${TMPDIR:-/tmp}" USER="${USER:-}" LOGNAME="${LOGNAME:-}" "$@" ;;
  esac
}
npm_env() { # HOME USERCONFIG command...: base, the proxies and a CA npm may need, never the git/gh
            # transport (one construction for the real run and the dry one)
  h=$1 u=$2; shift 2
  /usr/bin/env -i HOME="$h" PATH="$PATH" TMPDIR="${TMPDIR:-/tmp}" USER="${USER:-}" LOGNAME="${LOGNAME:-}" ${u:+NPM_CONFIG_USERCONFIG="$u"} \
    ${HTTPS_PROXY:+HTTPS_PROXY="$HTTPS_PROXY"} ${https_proxy:+https_proxy="$https_proxy"} \
    ${HTTP_PROXY:+HTTP_PROXY="$HTTP_PROXY"} ${http_proxy:+http_proxy="$http_proxy"} \
    ${NO_PROXY:+NO_PROXY="$NO_PROXY"} ${no_proxy:+no_proxy="$no_proxy"} \
    ${NODE_EXTRA_CA_CERTS:+NODE_EXTRA_CA_CERTS="$NODE_EXTRA_CA_CERTS"} "$@"
}
cargo_env() { # CARGO_HOME TARGET_DIR command... (the cargo class)
  ch=$1 td=$2; shift 2
  tool base /usr/bin/env CARGO_HOME="$ch" CARGO_TARGET_DIR="$td" MACOSX_DEPLOYMENT_TARGET=12.0 \
    SHEEPR_COMMIT_OVERRIDE="$short" ${RUSTUP_HOME:+RUSTUP_HOME="$RUSTUP_HOME"} "$@"
}
docker_env() { # the docker class
  tool base /usr/bin/env ${DOCKER_HOST:+DOCKER_HOST="$DOCKER_HOST"} ${DOCKER_CONFIG:+DOCKER_CONFIG="$DOCKER_CONFIG"} docker "$@"
}

step() { echo "release: step: $*"; }
npy() { /usr/bin/env -u DEVELOPER_DIR -u SDKROOT -u TOOLCHAINS /usr/bin/python3 -I "$root/scripts/lib/npm-same.py" "$@"; }
npm_pack() { # the four npm tarballs into $dest; sets files_npm
  files_npm=$(tool base sh "$S/lib/npm-pack.sh" "${tag#v}" "$dest/sheepr-macos-universal.tar.gz" "$dest/sheepr-linux-aarch64" \
    "$dest/sheepr-linux-x86_64" "$src/npm/sheepr/bin/sheepr" "$dest") || return 1
  files_npm=$(printf '%s' "$files_npm" | tr '\n' ' ')
}

# --- the build (PHASE3.md S2): unsigned, signed, or the signed control ----------------------------
build_release() { # mode: unsigned | signed | control
  mode=$1
  case $mode in unsigned) dest=$out/$tag-unsigned ;; signed) dest=$out/$tag ;; control) dest=$out/$tag-control ;; esac
  short=$(git -C "$root" rev-parse --short=12 "$commit")
  [ -e "$dest" ] && die "$dest exists (a release output directory is never reused)"
  scratch=$(mktemp -d /private/tmp/sr-release.XXXXXX 2>/dev/null || mktemp -d) || die "no scratch dir"
  trap 'git -C "$root" worktree remove --force "$scratch/src" >/dev/null 2>&1; rm -rf "$scratch"' EXIT
  trap 'exit 1' HUP INT TERM
  src=$scratch/src
  git -C "$root" worktree add -q --detach "$src" "$tag" || die "cannot make a worktree of $tag"
  [ -z "$(git -C "$src" status --porcelain --ignored)" ] || die "the worktree holds an untracked or ignored file"
  # from here on every helper and every setting comes from the tag's worktree, never the shared checkout
  S=$src/scripts
  . "$S/release.conf" || die "cannot read the tag's scripts/release.conf"
  pin=$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$src/rust-toolchain.toml")
  [ -n "$pin" ] || die "the tag's rust-toolchain.toml names no channel"
  echo "release: building $tag ($short, $mode) in $src"
  if [ "$mode" != unsigned ]; then
    step "the signing identity"
    /usr/bin/security find-identity -v -p codesigning 2>/dev/null | grep -q " $SR_SIGN_IDENTITY " \
      || die "the signing identity $SR_SIGN_IDENTITY is not in the keychain"
  fi

  # macOS compile: a fresh CARGO_HOME, fetched (Cargo checks each crate against Cargo.lock), then
  # offline; the compiler must be the pinned one
  step "the macOS compile"
  mkdir -p "$scratch/cargo-mac"
  (cd "$src" && cargo_env "$scratch/cargo-mac" "$scratch/target-mac" cargo fetch --locked -q) || die "cargo fetch (mac)"
  rustc_mac=$(cd "$src" && cargo_env "$scratch/cargo-mac" "$scratch/target-mac" rustc -V) || die "rustc -V (mac)"
  case $rustc_mac in "rustc $pin "*) ;; *) die "the Mac compiler is '$rustc_mac', not the pinned $pin" ;; esac
  # the Linux images, checked before anything is made (a failed precondition leaves no $dest)
  step "the Linux images"
  amd64=sr-amd64-base:$(printf %s "${SR_IMG_ALPINE##*sha256:}" | cut -c1-12)
  docker_env image inspect "$amd64" >/dev/null 2>&1 || amd64=sr-amd64-alpine:$(printf %s "${SR_IMG_ALPINE##*sha256:}" | cut -c1-12)
  docker_env image inspect "$amd64" >/dev/null 2>&1 || die "no local amd64 image of the pinned base (run ./test-all pull, or ./test-all amd64)"
  for i in arm64 amd64; do
    docker_env image inspect "sr-scratch:empty-$i" >/dev/null 2>&1 && continue
    tar -cf "$scratch/empty.tar" -T /dev/null && docker_env import --platform "linux/$i" "$scratch/empty.tar" "sr-scratch:empty-$i" >/dev/null \
      || die "cannot make the empty image sr-scratch:empty-$i"
  done
  mkdir -p "$dest" || die "cannot make $dest"
  for t in aarch64-apple-darwin x86_64-apple-darwin; do
    (cd "$src" && cargo_env "$scratch/cargo-mac" "$scratch/target-mac" cargo build -q --release --locked --offline --bin sheepr --target $t) \
      || die "cargo build $t"
  done

  # Linux, all of it before any signing: a failure here must not cost a notarization
  step "the Linux builds"
  mkdir -p "$scratch/cargo-linux"
  docker_env run --rm --pull=never -v "$src":/src:ro -v "$scratch/cargo-linux":/sdhome -e CARGO_HOME=/sdhome -w /src "$SR_IMG_ALPINE" \
    cargo fetch --locked -q || die "cargo fetch (linux)"
  for a in aarch64 x86_64; do
    case $a in aarch64) img=$SR_IMG_ALPINE pf=linux/arm64 ;; x86_64) img=$amd64 pf=linux/amd64 ;; esac
    mkdir -p "$scratch/target-$a"
    docker_env run --rm --pull=never --network none --platform "$pf" -v "$src":/src:ro -v "$scratch/cargo-linux":/sdhome:ro \
      -v "$scratch/target-$a":/tgt -e CARGO_HOME=/sdhome -e CARGO_TARGET_DIR=/tgt -e SHEEPR_COMMIT_OVERRIDE="$short" -w /src "$img" \
      sh -c 'cargo build -q --release --locked --offline --bin sheepr && rustc -V > /tgt/rustc && uname -m > /tgt/arch && if readelf -l /tgt/release/sheepr | grep -q INTERP; then echo "a PT_INTERP" >&2; exit 1; fi' \
      || die "the Linux $a build"
    [ "$(cat "$scratch/target-$a/arch")" = "$a" ] || die "the $a build ran as $(cat "$scratch/target-$a/arch")"
    case $(cat "$scratch/target-$a/rustc") in "rustc $pin "*) ;; *) die "the Linux $a compiler is '$(cat "$scratch/target-$a/rustc")', not the pinned $pin" ;; esac
    cp "$scratch/target-$a/release/sheepr" "$dest/sheepr-linux-$a" || die "copy $a"
    v=$(docker_env run --rm --pull=never --network none --platform "$pf" -v "$dest/sheepr-linux-$a":/sheepr:ro "sr-scratch:empty-${pf#linux/}" /sheepr --version) \
      || die "the $a binary does not run in an empty image (not static?)"
    case $v in *"$short"*) ;; *) die "the $a binary names another commit: $v" ;; esac
  done

  # macOS: the universal binary, then the bundle (unsigned), or signing (sign.sh)
  step "the macOS bundle"
  tool base lipo -create -output "$scratch/sheepr" "$scratch/target-mac/aarch64-apple-darwin/release/sheepr" \
    "$scratch/target-mac/x86_64-apple-darwin/release/sheepr" || die "lipo"
  for a in arm64 x86_64; do
    m=$(tool base vtool -arch $a -show-build "$scratch/sheepr" 2>/dev/null | awk '$1=="minos"{print $2; exit}')
    [ "$m" = 12.0 ] || die "the $a slice's minimum macOS is '$m', not 12.0"
  done
  xyz=${tag#v}; xyz=${xyz%%-*}
  if [ "$mode" = unsigned ]; then
    app=$(tool base sh "$S/bundle.sh" "$scratch/sheepr" "$scratch/bundle" "$xyz" "$c") || die "bundle.sh"
    mkdir -p "$scratch/rh"
    v=$(tool base sh "$S/lib/release-run.sh" "$scratch/rh" "$app/Contents/MacOS/sheepr" --version) || die "the Mac binary does not run"
    case $v in *"$short"*) ;; *) die "the Mac binary names another commit: $v" ;; esac
    tool base sh "$S/lib/archive.sh" make "$app" "$dest/sheepr-macos-universal.tar.gz" || die "archive"
    tool base sh "$S/lib/archive.sh" check "$dest/sheepr-macos-universal.tar.gz" || die "the archive fails its check"
  else
    step "signing"
    # sign.sh as a direct child (never in ( ), $( ) or a pipeline: it checks that its parent is
    # this process, by pid), with a nonce only this run knows, in a 0600 file of this user's
    umask 077; od -An -N16 -tx1 /dev/urandom | tr -d ' \n' > "$scratch/nonce"; umask 022
    nn=$(cat "$scratch/nonce")
    set -- --bin "$scratch/sheepr" --version "$xyz" --build "$c" --tag "$tag" --commit "$short" --dest "$dest" --real "$nn" --nonce-file "$scratch/nonce" --parent-pid "$$"
    [ "$mode" = control ] && set -- "$@" --no-notarize
    # in the foreground: a background child of a non-interactive sh ignores INT, so ctrl-C would
    # not reach it (measured); env execs sign.sh, so its parent is still this shell
    /usr/bin/env -i HOME="${HOME:-}" PATH="$PATH" TMPDIR="${TMPDIR:-/tmp}" USER="${USER:-}" LOGNAME="${LOGNAME:-}" \
      ${DEVELOPER_DIR:+DEVELOPER_DIR="$DEVELOPER_DIR"} "$S/lib/sign.sh" "$@" || die "signing failed"
  fi

  # install.sh (PHASE3.md S3), rendered with this release's version and the door
  step "install.sh, the checksums, the npm packages"
  files="sheepr-macos-universal.tar.gz sheepr-linux-aarch64 sheepr-linux-x86_64 install.sh"
  [ -f "$S/install.sh" ] || die "no scripts/install.sh at the tag"
  tool base sh "$S/lib/render-install.sh" "$S/install.sh" "${tag#v}" "$dest/install.sh" || die "render install.sh"
  (cd "$dest" && shasum -a 256 $files > SHA256SUMS) || die "SHA256SUMS"
  # the Homebrew cask for lukaso/tap (PHASE3.md S4): a file for the tap, never uploaded
  tool base sh "$S/lib/render-cask.sh" "${tag#v}" "$(shasum -a 256 "$dest/sheepr-macos-universal.tar.gz" | cut -d' ' -f1)" "$dest/sheepr.rb" \
    || die "render the cask"
  npm_pack || die "npm packing"
  {
    printf '{\n  "v": 1,\n  "tag": "%s",\n  "commit": "%s",\n  "mode": "%s",\n  "control": %s,\n  "files": [\n' "$tag" "$commit" "$mode" "$( [ "$mode" = control ] && echo true || echo false)"
    sep=""
    for f in $files $files_npm; do
      h=$(shasum -a 256 "$dest/$f" | cut -d' ' -f1)
      case $f in
        sheepr-macos-*) r=$rustc_mac ;;
        sheepr-linux-aarch64|sheepr-linux-x86_64) r=$(cat "$scratch/target-${f#sheepr-linux-}/rustc") ;;   # the binaries by name: the npm packages also start sheepr-linux-
        *) r="" ;;
      esac
      printf '%s    {"name": "%s", "sha256": "%s"%s}' "$sep" "$f" "$h" "${r:+, \"rustc\": \"$r\"}"
      sep=",
"
    done
    printf '\n  ]\n}\n'
  } > "$dest/MANIFEST.json"
  echo "release: $tag built ($mode) in $dest"
}

# --- publish: the thin executor behind the gate (PHASE3.md §1.2) ------------------------------
# The remote tag is read from github.com/lukaso/sheepr itself (the repository the release is
# made on), never from `origin`. GH and GITCMD are the gh and git to use: the real ones for
# publish; for __publish-dry (the cells), stand-ins under /private/tmp/sr-p3-fixtures.*, given by
# path, so the dry run can never reach the real gh.
UPSTREAM=https://github.com/lukaso/sheepr
confirm() { # prompt -> 0 on the tag typed back
  if [ "${DRY:-no}" = yes ]; then
    n=$(grep -c '^ask ' "${SR_ASK_RECORD:-/dev/null}" 2>/dev/null); n=${n:-0}
    a=$(sed -n "$((n + 1))p" "${SR_ASK_SCRIPT:-/dev/null}")
    echo "ask $1 -> $a" >> "${SR_ASK_RECORD:-/dev/null}"
  else
    printf '%s ' "$1" > /dev/tty; IFS= read -r a < /dev/tty || return 1
  fi
  [ "$a" = "$tag" ]
}
# npm-check's stamp (NPM-CHECKED in the output dir): the tag and the sha256 of the MANIFEST.json it
# passed on. npm is the last one-way step, so its check must pass before anything is public.
stamp_line() { printf '%s %s' "$tag" "$(shasum -a 256 "$1/MANIFEST.json" | cut -d' ' -f1)"; }
stamp_ok() { # dir -> 0 if npm-check passed on this tag and this manifest
  [ -f "$1/NPM-CHECKED" ] && [ -f "$1/MANIFEST.json" ] && [ "$(cat "$1/NPM-CHECKED")" = "$(stamp_line "$1")" ]
}
man_hash() { # dir file -> the file's sha256 in MANIFEST.json (empty if it is not listed)
  sed -n "s/.*\"name\": \"$2\", \"sha256\": \"\([0-9a-f]\{64\}\)\".*/\1/p" "$1/MANIFEST.json" | head -1
}
files_ok() { # dir -> 0 if every file MANIFEST.json lists is there and is its hash (npm-check's files)
  fs=$(sed -n 's/.*"name": "\([^"]*\)", "sha256": "[0-9a-f]\{64\}".*/\1/p' "$1/MANIFEST.json")
  [ -n "$fs" ] || return 1
  for f in $fs; do
    [ -f "$1/$f" ] && [ "$(shasum -a 256 "$1/$f" | cut -d' ' -f1)" = "$(man_hash "$1" "$f")" ] || { echo "release: $f is missing or not its manifest hash" >&2; return 1; }
  done
}
publish_exec() { # dir
  d=$1
  pt=$(mktemp -d /private/tmp/sr-publish.XXXXXX) || die "no temp dir"
  trap 'rm -rf "$pt" ${DRYHOME:+"$DRYHOME"}' EXIT; trap 'rm -rf "$pt" ${DRYHOME:+"$DRYHOME"}; exit 1' HUP INT TERM
  stamp_ok "$d" || die "npm-check has not passed on $d for this manifest; run release.sh npm-check $tag first"
  files_ok "$d" || die "a file npm-check checked has changed since (above); run release.sh npm-check $tag again"
  # SHA256SUMS is not in the manifest: the build wrote it from those four files, in this order, so
  # its bytes follow from the manifest's hashes; nothing else is published, at any time
  for x in sheepr-macos-universal.tar.gz sheepr-linux-aarch64 sheepr-linux-x86_64 install.sh; do
    printf '%s  %s\n' "$(man_hash "$d" "$x")" "$x"
  done > "$pt/sums"
  cmp -s "$pt/sums" "$d/SHA256SUMS" || die "SHA256SUMS is not the one the build wrote (from MANIFEST.json's hashes)"
  sums_h=$(shasum -a 256 "$pt/sums" | cut -d' ' -f1)
  # npm's owners of the four names, before any git or gh call: GitHub must not go public when npm
  # would then refuse (the first v0.1.0 did exactly that)
  npm_list; npm_owners "$NPMC" "$NPM" "$pt"
  tool "$NETC" "$GITCMD" ls-remote "$UPSTREAM" "refs/tags/$tag*" > "$pt/remote" || die "git ls-remote $UPSTREAM"
  tool "$NETC" "$GH" api "repos/lukaso/sheepr/releases" --paginate --jq '.[].tag_name' > "$pt/releases" || die "gh: cannot list the releases"
  (cd "$root" && sh scripts/release-plan.sh --out "$d" --tag "$tag" --remote "$pt/remote" --releases "$pt/releases") > "$pt/plan" || die "the planner refused"
  sh "$root/scripts/release-plan.sh" --validate "$pt/plan" || die "the plan does not validate"
  echo "release: the plan:"; sed 's/^/  /' "$pt/plan"
  set -- $(sed -n 's/^POST [^ ]* //p' "$pt/plan")
  id=$(tool "$NETC" "$GH" api -X POST repos/lukaso/sheepr/releases "$@" --jq .id) || die "gh: cannot make the draft"
  case $id in ''|*[!0-9]*) die "gh: no release id ($id)" ;; esac
  echo "release: draft $id made"
  for f in $(sed -n 's/^UPLOAD //p' "$pt/plan"); do
    tool "$NETC" "$GH" api -X POST -H 'Content-Type: application/octet-stream' \
      "https://uploads.github.com/repos/lukaso/sheepr/releases/$id/assets?name=$f" --input "$d/$f" --jq .id >/dev/null \
      || die "gh: upload of $f failed (the draft $id is not public; delete it on GitHub)"
  done
  tool "$NETC" "$GH" api "repos/lukaso/sheepr/releases/$id" --jq '.assets[] | "\(.id) \(.name)"' > "$pt/assets" || die "gh: cannot read the draft"
  [ "$(awk '{print $2}' "$pt/assets" | sort | tr '\n' ' ')" = "SHA256SUMS install.sh sheepr-linux-aarch64 sheepr-linux-x86_64 sheepr-macos-universal.tar.gz " ] \
    || die "the draft's assets are not exactly the five: $(awk '{print $2}' "$pt/assets" | tr '\n' ' ')"
  while read -r aid name; do
    tool "$NETC" "$GH" api -H 'Accept: application/octet-stream' "repos/lukaso/sheepr/releases/assets/$aid" > "$pt/dl" || die "gh: cannot download $name"
    # the build's bytes: the manifest's hash (SHA256SUMS: the bytes the build wrote, from it)
    if [ "$name" = SHA256SUMS ]; then
      [ "$(shasum -a 256 "$pt/dl" | cut -d' ' -f1)" = "$sums_h" ] || die "the uploaded SHA256SUMS is not the one the build wrote; the draft $id stays a draft"
      continue
    fi
    want=$(man_hash "$d" "$name")
    [ -n "$want" ] || die "$name has no manifest hash; the draft $id stays a draft"
    [ "$(shasum -a 256 "$pt/dl" | cut -d' ' -f1)" = "$want" ] || die "the uploaded $name is not the build's (its manifest hash); the draft $id stays a draft"
  done < "$pt/assets"
  tool "$NETC" "$GITCMD" ls-remote "$UPSTREAM" "refs/tags/$tag*" > "$pt/remote2" || die "git ls-remote $UPSTREAM"
  cmp -s "$pt/remote" "$pt/remote2" || die "the remote tag changed during the upload; the draft $id stays a draft"
  confirm "release: the draft $id holds the five files, each matching. Type the tag again to make it public:" || die "not confirmed; the draft $id stays a draft"
  if ! tool "$NETC" "$GH" api -X PATCH "repos/lukaso/sheepr/releases/$id" -F draft=false >/dev/null; then
    # the request failed, but GitHub may have made the release public anyway: read it again
    st=$(tool "$NETC" "$GH" api "repos/lukaso/sheepr/releases/$id" --jq .draft) || st=""
    case $st in
      false) echo "release: the publish request failed, but $tag is public (the release reads draft: false)"; return 0 ;;
      true) die "gh: cannot publish the draft $id; it is still a draft. Publish it on GitHub (Releases, the draft $tag, Publish release), or delete the draft and run publish again" ;;
      *) die "gh: cannot publish the draft $id, and cannot read whether it is public: check it on GitHub" ;;
    esac
  fi
  echo "release: $tag published"
}
publish() {
  verify "$out/$tag"
  GH=gh GITCMD=git NPM=npm NPMC=npm DRY=no NETC=net publish_exec "$out/$tag"
}
standins() { # VAR...: each names a stand-in for a dry run, or refuse
  # a stand-in is a script (not a symlink, not a binary), whose real directory is a fixture dir;
  # and every call runs with a fresh temp HOME and nothing else of the caller's (the `dry` class)
  for v in "$@"; do
    eval "p=\${$v:-}"
    # one string is checked and run: no control character (a trailing newline would be stripped
    # when the path is rebuilt, and another file would run)
    case $p in *[[:cntrl:]]*) die "$v holds a control character" ;; esac
    [ -n "$p" ] && [ -f "$p" ] && [ ! -L "$p" ] && [ -x "$p" ] || die "$v is not an executable file (and not a symlink)"
    pd=$(cd -P "$(dirname "$p")" 2>/dev/null && pwd -P) || die "$v: cannot resolve its directory"
    case $pd/ in /private/tmp/sr-p3-fixtures.*/) ;; *) die "$v must be a stand-in in a /private/tmp/sr-p3-fixtures.* directory, not in $pd" ;; esac
    [ "$(head -c 2 "$p")" = '#!' ] || die "$v must be a script stand-in"
    eval "$v=\$pd/\$(basename \"\$p\")"
  done
}
publish_dry() { # the cells' entry: stand-ins by path only, never the real gh; verify is not run
  standins SR_PUBLISH_DRY_GH SR_PUBLISH_DRY_GIT SR_PUBLISH_DRY_NPM
  DRYHOME=$(mktemp -d /private/tmp/sr-dryhome.XXXXXX) || die "no temp HOME"
  echo "release: __publish-dry (stand-ins; verify not run; a temp HOME)"
  GH=$SR_PUBLISH_DRY_GH GITCMD=$SR_PUBLISH_DRY_GIT NPM=$SR_PUBLISH_DRY_NPM NPMC=npmdry DRY=yes NETC=dry publish_exec "$out/$tag"
  rm -rf "$DRYHOME"
}

# --- publish-npm: the four packages (PHASE3.md §5), after publish --------------------------------
# Platform packages first, the main one last (its optional dependencies name them). Only the files
# in <out>/vTAG, each hashed against its manifest entry just before its upload. A version already on
# npm is skipped when its integrity is this file's (a resumed run) and refused otherwise. An rc goes
# under the `next` dist-tag: npm makes a version without a tag `latest`. npm uses the operator's own
# login (`npm login`), never a token from the environment.
NPMREG=https://registry.npmjs.org/   # pinned on the command line: an npmrc's registry or scope registry never applies
# npm_visible NAME VERSION INTEGRITY: wait until npm serves this version with this file. npm may
# answer a publish of a new package with "Your package is being processed" and show the version
# only minutes later (measured 2026-10-04: two of the four placeholders, about two minutes). The
# main package's optional dependencies name the platform packages, so each upload waits until its
# file is there before the next one: NPM_WAIT_TRIES views, NPM_WAIT_STEP seconds apart (set by the
# entry, not the environment). Another file there is refused at once.
npm_visible() {
  i=0
  while :; do
    vout=$(tool "$NETC" "$NPM" view --json --prefer-online --registry="$NPMREG" "$1@$2" dist.integrity 2> "$pc/err"); r=$?
    got=$(printf '%s' "$vout" | tr -d ' "\n\r')
    if [ $r = 0 ] && [ -n "$got" ]; then
      [ "$got" = "$3" ] && return 0
      die "$1@$2 is on npm with another file ($got) than the one just published; nothing after it is published"
    fi
    i=$((i + 1))
    [ $i -lt "$NPM_WAIT_TRIES" ] || die "$1@$2 is not visible on npm after $NPM_WAIT_TRIES views, $NPM_WAIT_STEP s apart (npm may still be processing it): wait, then run publish-npm again (it skips a version already there with this file); nothing after it is published"
    sleep "$NPM_WAIT_STEP"
  done
}
# npm_list: release.conf's SR_NPM_PKGS must be the four packages, the main one last (npm-same.py
# list: the check npm-check makes). publish and publish-npm read the list again, and npm-check's
# stamp does not bind it, so each checks it before any npm call.
npm_list() {
  . "$root/scripts/release.conf" || die "cannot read scripts/release.conf"
  npy list "${SR_NPM_PKGS:-}" || die "scripts/release.conf's SR_NPM_PKGS is refused (above); nothing was published"
}
# npm_owners CLASS NPM TMPDIR: npm's owners of each name in SR_NPM_PKGS must include SR_NPM_USER
# (release.conf). Measured (npm 11.6.0): `owner ls NAME` needs no login and prints `user <email>`
# lines, or exits 1 with E404 for a name not on npm. A name not on npm yet is refused: npm may
# reject it as too similar to another, or someone may take it first; each name is reserved first
# (a 0.0.0 placeholder). publish runs this before any GitHub call, publish-npm before any upload.
npm_owners() { # after npm_list, which read release.conf: the list checked is the list used (one read)
  [ -n "${SR_NPM_USER:-}" ] && [ -n "${SR_NPM_PKGS:-}" ] || die "scripts/release.conf names no SR_NPM_USER or SR_NPM_PKGS (npm_list runs first)"
  for p in $SR_NPM_PKGS; do
    own=$(tool "$1" "$2" owner ls "$p" --registry="$NPMREG" 2> "$3/err"); r=$?
    if [ $r != 0 ]; then
      grep -q 'E404' "$3/err" && die "$p is not on npm yet: reserve the name first (publish a 0.0.0 placeholder as $SR_NPM_USER), then run this again; nothing was published"
      die "cannot read the owners of $p on npm (npm owner ls: exit $r, $(head -c 200 "$3/err" | tr '\n' ' ')); nothing was published"
    fi
    printf '%s\n' "$own" | awk -v me="$SR_NPM_USER" '$1 == me {f = 1} END {exit !f}' \
      || die "$p on npm belongs to $(printf '%s\n' "$own" | awk '{print $1}' | tr '\n' ' ' | sed 's/ $//'), not to $SR_NPM_USER; nothing was published"
  done
}
publish_npm_exec() { # dir
  d=$1 m=$1/MANIFEST.json nv=${tag#v}
  pc=$(mktemp -d /private/tmp/sr-npmpub.XXXXXX) || die "no temp dir"   # the private copies npm gets
  trap 'rm -rf "$pc" ${DRYHOME:+"$DRYHOME"}' EXIT; trap 'rm -rf "$pc" ${DRYHOME:+"$DRYHOME"}; exit 1' HUP INT TERM
  chmod 700 "$pc" || die "cannot make $pc private"
  stamp_ok "$d" || die "npm-check has not passed on $d for this manifest; run release.sh npm-check $tag first"
  mf() { sed -n "s/^ *\"$1\": *\"\{0,1\}\([^\",]*\)\"\{0,1\},\{0,1\}$/\1/p" "$m" | head -1; }
  [ "$(mf tag)" = "$tag" ] && [ "$(mf mode)" = signed ] && [ "$(mf control)" = false ] \
    || die "the manifest is not $tag's signed build, or it is a control build"
  lc=$(git -C "$root" rev-parse -q --verify "refs/tags/$tag^{commit}") || die "no local tag $tag"
  [ "$(mf commit)" = "$lc" ] || die "the manifest's commit $(mf commit) is not $tag's ($lc)"
  # every file npm-check checked, before the first npm call (each is checked again just before its upload)
  files_ok "$d" || die "a file npm-check checked has changed since (above); nothing is published. Run release.sh npm-check $tag again"
  # who may publish, before any view or upload: npm must say the user is release.conf's (measured,
  # npm 11.6.0: `whoami` prints the user, or exits 1 with ENEEDAUTH), and that user must own every name
  npm_list
  me=$(tool "$NETC" "$NPM" whoami --registry="$NPMREG" 2> "$pc/err") \
    || die "npm whoami failed (not logged in to npm?): run npm login as ${SR_NPM_USER:-the owner}, then publish-npm again; nothing was published ($(head -c 200 "$pc/err" | tr '\n' ' '))"
  [ "$me" = "${SR_NPM_USER:-}" ] || die "logged in to npm as $(printf '%s' "$me" | head -c 100 | tr '\n' ' '), but the packages belong to ${SR_NPM_USER:-nobody named in release.conf}: run npm login as that user, then publish-npm again; nothing was published"
  npm_owners "$NETC" "$NPM" "$pc"
  case $tag in *-rc.*) set -- --tag next ;; *) set -- ;; esac
  for p in $SR_NPM_PKGS; do
    f=$p-$nv.tgz
    # npm gets a private copy, hashed against the manifest: the bytes checked are the bytes sent
    cp "$d/$f" "$pc/$f" 2>/dev/null || die "no $f in $d; nothing after it is published"
    h=$(shasum -a 256 "$pc/$f" | cut -d' ' -f1)
    [ -n "$h" ] && [ "$h" = "$(man_hash "$d" "$f")" ] \
      || die "$f does not match its manifest hash (changed after npm-check?); nothing after it is published"
    want=$(npy integrity "$pc/$f") || die "cannot hash $f"
    # on npm already? measured (npm 11.6.0): a version there is exit 0 and its integrity (quoted
    # under --json); one that is not, exit 1 with E404; anything else cannot say
    vout=$(tool "$NETC" "$NPM" view --json --prefer-online --registry="$NPMREG" "$p@$nv" dist.integrity 2> "$pc/err"); r=$?
    got=$(printf '%s' "$vout" | tr -d ' "\n\r')
    if [ $r = 0 ] && [ -n "$got" ]; then
      [ "$got" = "$want" ] || die "$p@$nv is already on npm with another file ($got); nothing after it is published"
      echo "release: $p@$nv is already on npm (the same file): skipped"
      continue
    elif ! { [ $r != 0 ] && printf '%s\n' "$vout" | cat - "$pc/err" | grep -q 'E404'; }; then
      die "cannot tell whether $p@$nv is on npm (npm view: exit $r, $(head -c 300 "$pc/err" | tr '\n' ' ')); nothing after it is published"
    fi
    tool "$NETC" "$NPM" publish --access public --registry="$NPMREG" "$@" "$pc/$f" \
      || die "npm publish of $f failed; run publish-npm again to go on (a package already on npm is skipped; if npm says this version exists, it is still being processed: wait a few minutes first)"
    npm_visible "$p" "$nv" "$want"
    echo "release: $p@$nv published, and npm shows this file"
  done
  echo "release: the four npm packages of $tag are on npm. Next: npm logout, and the token check (PHASE3.md §5)"
}
publish_npm() { NPM=npm NETC=npm NPM_WAIT_TRIES=90 NPM_WAIT_STEP=10 publish_npm_exec "$out/$tag"; }   # at most 15 minutes per package
publish_npm_dry() { # the cells' entry: a stand-in npm by path only, never the real one
  standins SR_PUBLISH_DRY_NPM
  DRYHOME=$(mktemp -d /private/tmp/sr-dryhome.XXXXXX) || die "no temp HOME"
  echo "release: __publish-npm-dry (a stand-in npm; a temp HOME)"
  NPM=$SR_PUBLISH_DRY_NPM NETC=npmdry NPM_WAIT_TRIES=5 NPM_WAIT_STEP=0 publish_npm_exec "$out/$tag"
  rm -rf "$DRYHOME"
}

# --- npm-check: before any `npm publish` (PHASE3.md §5 step 5) ----------------------------------
npm_check() { # dir
  . "$root/scripts/release.conf" || die "cannot read scripts/release.conf"
  . "$root/scripts/lib/realtools.sh" || die "cannot read realtools.sh"
  d=$1 m=$1/MANIFEST.json nv=${tag#v}
  [ -f "$m" ] || die "no $m"
  # a stamp from an earlier pass never outlives a check that does not pass
  rm -f "$d/NPM-CHECKED" || die "cannot remove $d/NPM-CHECKED"
  mode=$(sed -n 's/^ *"mode": *"\([^"]*\)",*$/\1/p' "$m"); ctl=$(sed -n 's/^ *"control": *\([a-z]*\),*$/\1/p' "$m")
  mtag=$(sed -n 's/^ *"tag": *"\([^"]*\)",*$/\1/p' "$m")
  [ "$mtag" = "$tag" ] || die "the manifest is for $mtag, not $tag"
  [ "$mode" = signed ] || die "the manifest's mode is '$mode', not signed"
  [ "$ctl" = false ] || die "the manifest is a control build"
  [ -n "${SR_NPM_PKGS:-}" ] || die "scripts/release.conf names no SR_NPM_PKGS"
  for f in sheepr-macos-universal.tar.gz sheepr-linux-aarch64 sheepr-linux-x86_64 $(for p in $SR_NPM_PKGS; do printf '%s-%s.tgz\n' "$p" "$nv"; done); do
    [ -f "$d/$f" ] || die "missing $f (a file of the build, or a package of release.conf's SR_NPM_PKGS)"
    h=$(shasum -a 256 "$d/$f" | cut -d' ' -f1)
    grep -q "\"name\": \"$f\", \"sha256\": \"$h\"" "$m" || die "$f does not match its manifest hash"
  done
  nt=/private/tmp/sr-npmcheck.$$.$(od -An -N4 -tx4 /dev/urandom | tr -d ' ')
  trap 'rm -rf "$nt"' EXIT; trap 'rm -rf "$nt"; exit 1' HUP INT TERM
  mkdir -m 700 "$nt" "$nt/p" "$nt/ref" || die "no temp dir"
  # the four packages, read raw, before anything unpacks them (npm-same.py says why), against the
  # tag's license texts, README and launcher
  for x in LICENSE-MIT LICENSE-APACHE README.md npm/sheepr/bin/sheepr; do
    git -C "$root" show "$tag:$x" > "$nt/ref/$(basename "$x")" 2>/dev/null || die "cannot read $x at $tag"
  done
  mv "$nt/ref/sheepr" "$nt/ref/launcher"
  dt=$d/sheepr-darwin-universal-$nv.tgz
  npy packages "$d" "$nv" "$nt/ref" "$SR_NPM_PKGS" || die "an npm package is refused (above)"
  /usr/bin/tar -xzf "$dt" -C "$nt/p" || die "cannot unpack the darwin package"   # the bundle the real tools judge
  b=$nt/p/package/Sheepr.app
  /usr/bin/plutil -extract SheeprControlBuild raw -o - "$b/Contents/Info.plist" >/dev/null 2>&1 && die "the darwin package holds a control build (SheeprControlBuild in its Info.plist)"
  rt_meets "$b" && rt_meets "$b/Contents/MacOS/sheepr" || die "codesign: the darwin package's bundle does not meet the release requirement"
  rt_staple_ok "$b" || die "stapler: the darwin package's bundle has no valid staple ticket"
  rt_spctl_ok "$b" || die "spctl: Gatekeeper rejects the darwin package's bundle"
  # the release archive: its own check (only files and directories, the bundle signed and
  # stapled), then the same files as the package, read raw: path, mode and content (a CDHash is
  # read from the embedded signature, not recomputed, so equal CDHashes prove nothing)
  "$root/scripts/lib/archive.sh" check "$d/sheepr-macos-universal.tar.gz" --signed || die "the release archive fails its check (above)"
  npy same "$dt" "$d/sheepr-macos-universal.tar.gz" || die "the darwin package's bundle is not the release archive's bundle (above)"
  rm -rf "$nt"; trap - EXIT
  stamp_line "$d" > "$d/NPM-CHECKED" || die "cannot write $d/NPM-CHECKED"
  echo "release: the npm tarballs of $tag check out (NPM-CHECKED written); publish comes next, then publish-npm"
}

# --- verify: the three facts, from the archive, by the real tools (PHASE3.md §1.2) --------------
verify() { # dir -> exit 1 naming the tool that refused
  . "$root/scripts/release.conf" || die "cannot read scripts/release.conf"
  . "$root/scripts/lib/realtools.sh" || die "cannot read realtools.sh"
  d=$1 arc=$1/sheepr-macos-universal.tar.gz
  [ -f "$arc" ] || die "no $arc"
  vt=/private/tmp/sr-verify.$$.$(od -An -N4 -tx4 /dev/urandom | tr -d ' ')
  trap 'rm -rf "$vt"' EXIT; trap 'rm -rf "$vt"; exit 1' HUP INT TERM
  mkdir -m 700 "$vt" && /usr/bin/tar -xzf "$arc" -C "$vt" || die "cannot unpack $arc"   # the bundle the real tools judge
  a=$vt/Sheepr.app
  # a control build's bundle carries the control marker (bundle.sh --control): never a release
  /usr/bin/plutil -extract SheeprControlBuild raw -o - "$a/Contents/Info.plist" >/dev/null 2>&1 && die "the archive holds a control build (SheeprControlBuild in its Info.plist)"
  rt_meets "$a" && rt_meets "$a/Contents/MacOS/sheepr" || die "codesign: the bundle does not meet the release requirement"
  rt_staple_ok "$a" || die "stapler: no valid staple ticket"
  rt_spctl_ok "$a" || die "spctl: Gatekeeper rejects the bundle"
  rm -rf "$vt"; trap - EXIT
  echo "release: $d: signed, notarized and stapled (checked from the archive)"
}

case $sub in
  check) checks ;;
  verify) verify "$out/$tag" ;;
  build)
    checks
    if [ $sign = no ]; then build_release unsigned
    elif [ $nonot = yes ]; then build_release control
    else build_release signed; fi ;;
  publish) publish ;;
  __publish-dry) publish_dry ;;
  publish-npm) publish_npm ;;
  __publish-npm-dry) publish_npm_dry ;;
  # the cells' view of the npm class: the environment the real run's npm gets (it runs only env)
  __npm-env) tool npm /usr/bin/env ;;
  npm-check) npm_check "$out/$tag" ;;
esac
