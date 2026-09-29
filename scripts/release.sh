#!/bin/sh
# The sheepdog release (PHASE3.md S2). Run from a clean checkout of the tag.
#
#   release.sh check vX.Y.Z[-rc.N]                    the checks only
#   release.sh build [--out DIR] vTAG                 the unsigned artifacts (the agent may run it)
#   release.sh build --sign [--no-notarize] [--out DIR] vTAG
#                                                     the signed, notarized release (the operator)
#   release.sh publish [--out DIR] vTAG               publish a built release (the operator)
#   release.sh npm-check [--out DIR] vTAG             check the npm tarballs before `npm publish`
#   release.sh verify [--out DIR] vTAG                the release's facts from its archive (read-only)
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
# one collation and one message language for every sort and comparison (the operator's locale sorts
# "SHA256SUMS" after "install.sh"; measured)
LC_ALL=C; export LC_ALL
root=$(cd "$(dirname "$0")/.." && pwd -P) || exit 1
usage() { echo "usage: release.sh check|build|publish|npm-check|verify [--sign] [--no-notarize] [--out DIR] vTAG" >&2; exit 2; }
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
  check|build|publish|npm-check|verify|__publish-dry) ;;
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
    echo "release: refused: $sub$( [ $sign = yes ] && echo ' --sign') needs a terminal" >&2; exit 4
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
tool() { # class command... (base: HOME PATH TMPDIR USER LOGNAME [DEVELOPER_DIR]; net: base and the
          # git/gh transport: SSH_AUTH_SOCK GIT_SSH_COMMAND GH_CONFIG_DIR, never GH_TOKEN, since gh
          # uses its own stored login)
  cls=$1; shift
  case $cls in
    base) env -i HOME="${HOME:-}" PATH="$PATH" TMPDIR="${TMPDIR:-/tmp}" USER="${USER:-}" LOGNAME="${LOGNAME:-}" \
            ${DEVELOPER_DIR:+DEVELOPER_DIR="$DEVELOPER_DIR"} "$@" ;;
    net) env -i HOME="${HOME:-}" PATH="$PATH" TMPDIR="${TMPDIR:-/tmp}" USER="${USER:-}" LOGNAME="${LOGNAME:-}" \
            ${SSH_AUTH_SOCK:+SSH_AUTH_SOCK="$SSH_AUTH_SOCK"} ${GIT_SSH_COMMAND:+GIT_SSH_COMMAND="$GIT_SSH_COMMAND"} \
            ${GH_CONFIG_DIR:+GH_CONFIG_DIR="$GH_CONFIG_DIR"} ${XDG_CONFIG_HOME:+XDG_CONFIG_HOME="$XDG_CONFIG_HOME"} \
            ${HTTPS_PROXY:+HTTPS_PROXY="$HTTPS_PROXY"} ${https_proxy:+https_proxy="$https_proxy"} \
            ${HTTP_PROXY:+HTTP_PROXY="$HTTP_PROXY"} ${http_proxy:+http_proxy="$http_proxy"} \
            ${ALL_PROXY:+ALL_PROXY="$ALL_PROXY"} ${all_proxy:+all_proxy="$all_proxy"} \
            ${NO_PROXY:+NO_PROXY="$NO_PROXY"} ${no_proxy:+no_proxy="$no_proxy"} "$@" ;;
    # the dry publish: a fresh temp HOME and nothing else of the caller's, so even a real gh reached
    # through a stand-in would have no login
    dry) env -i HOME="$DRYHOME" PATH="$PATH" TMPDIR="${TMPDIR:-/tmp}" USER="${USER:-}" LOGNAME="${LOGNAME:-}" "$@" ;;
  esac
}
cargo_env() { # CARGO_HOME TARGET_DIR command... (the cargo class)
  ch=$1 td=$2; shift 2
  tool base env CARGO_HOME="$ch" CARGO_TARGET_DIR="$td" MACOSX_DEPLOYMENT_TARGET=12.0 \
    SHEEPDOG_COMMIT_OVERRIDE="$short" ${RUSTUP_HOME:+RUSTUP_HOME="$RUSTUP_HOME"} "$@"
}
docker_env() { # the docker class
  tool base env ${DOCKER_HOST:+DOCKER_HOST="$DOCKER_HOST"} ${DOCKER_CONFIG:+DOCKER_CONFIG="$DOCKER_CONFIG"} docker "$@"
}

step() { echo "release: step: $*"; }
npm_pack() { # the four npm tarballs into $dest; sets files_npm
  files_npm=$(tool base sh "$S/lib/npm-pack.sh" "${tag#v}" "$dest/sheepdog-macos-universal.tar.gz" "$dest/sheepdog-linux-aarch64" \
    "$dest/sheepdog-linux-x86_64" "$src/npm/sheepdog/bin/sheepdog" "$dest") || return 1
  files_npm=$(printf '%s' "$files_npm" | tr '\n' ' ')
}

# --- the build (PHASE3.md S2): unsigned, signed, or the signed control ----------------------------
build_release() { # mode: unsigned | signed | control
  mode=$1
  case $mode in unsigned) dest=$out/$tag-unsigned ;; signed) dest=$out/$tag ;; control) dest=$out/$tag-control ;; esac
  short=$(git -C "$root" rev-parse --short=12 "$commit")
  [ -e "$dest" ] && die "$dest exists (a release output directory is never reused)"
  scratch=$(mktemp -d /private/tmp/sd-release.XXXXXX 2>/dev/null || mktemp -d) || die "no scratch dir"
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
    /usr/bin/security find-identity -v -p codesigning 2>/dev/null | grep -q " $SD_SIGN_IDENTITY " \
      || die "the signing identity $SD_SIGN_IDENTITY is not in the keychain"
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
  amd64=sd-amd64-base:$(printf %s "${SD_IMG_ALPINE##*sha256:}" | cut -c1-12)
  docker_env image inspect "$amd64" >/dev/null 2>&1 || amd64=sd-amd64-alpine:$(printf %s "${SD_IMG_ALPINE##*sha256:}" | cut -c1-12)
  docker_env image inspect "$amd64" >/dev/null 2>&1 || die "no local amd64 image of the pinned base (run ./test-all pull, or ./test-all amd64)"
  for i in arm64 amd64; do
    docker_env image inspect "sd-scratch:empty-$i" >/dev/null 2>&1 && continue
    tar -cf "$scratch/empty.tar" -T /dev/null && docker_env import --platform "linux/$i" "$scratch/empty.tar" "sd-scratch:empty-$i" >/dev/null \
      || die "cannot make the empty image sd-scratch:empty-$i"
  done
  mkdir -p "$dest" || die "cannot make $dest"
  for t in aarch64-apple-darwin x86_64-apple-darwin; do
    (cd "$src" && cargo_env "$scratch/cargo-mac" "$scratch/target-mac" cargo build -q --release --locked --offline --bin sheepdog --target $t) \
      || die "cargo build $t"
  done

  # Linux, all of it before any signing: a failure here must not cost a notarization
  step "the Linux builds"
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
    case $(cat "$scratch/target-$a/rustc") in "rustc $pin "*) ;; *) die "the Linux $a compiler is '$(cat "$scratch/target-$a/rustc")', not the pinned $pin" ;; esac
    cp "$scratch/target-$a/release/sheepdog" "$dest/sheepdog-linux-$a" || die "copy $a"
    v=$(docker_env run --rm --pull=never --network none --platform "$pf" -v "$dest/sheepdog-linux-$a":/sheepdog:ro "sd-scratch:empty-${pf#linux/}" /sheepdog --version) \
      || die "the $a binary does not run in an empty image (not static?)"
    case $v in *"$short"*) ;; *) die "the $a binary names another commit: $v" ;; esac
  done

  # macOS: the universal binary, then the bundle (unsigned), or signing (sign.sh)
  step "the macOS bundle"
  tool base lipo -create -output "$scratch/sheepdog" "$scratch/target-mac/aarch64-apple-darwin/release/sheepdog" \
    "$scratch/target-mac/x86_64-apple-darwin/release/sheepdog" || die "lipo"
  for a in arm64 x86_64; do
    m=$(tool base vtool -arch $a -show-build "$scratch/sheepdog" 2>/dev/null | awk '$1=="minos"{print $2; exit}')
    [ "$m" = 12.0 ] || die "the $a slice's minimum macOS is '$m', not 12.0"
  done
  xyz=${tag#v}; xyz=${xyz%%-*}
  if [ "$mode" = unsigned ]; then
    app=$(tool base sh "$S/bundle.sh" "$scratch/sheepdog" "$scratch/bundle" "$xyz" "$c") || die "bundle.sh"
    mkdir -p "$scratch/rh"
    v=$(tool base sh "$S/lib/release-run.sh" "$scratch/rh" "$app/Contents/MacOS/sheepdog" --version) || die "the Mac binary does not run"
    case $v in *"$short"*) ;; *) die "the Mac binary names another commit: $v" ;; esac
    tool base sh "$S/lib/archive.sh" make "$app" "$dest/sheepdog-macos-universal.tar.gz" || die "archive"
    tool base sh "$S/lib/archive.sh" check "$dest/sheepdog-macos-universal.tar.gz" || die "the archive fails its check"
  else
    step "signing"
    # sign.sh as a direct child (never in ( ), $( ) or a pipeline: it checks that its parent is
    # this process, by pid), with a nonce only this run knows, in a 0600 file of this user's
    umask 077; od -An -N16 -tx1 /dev/urandom | tr -d ' \n' > "$scratch/nonce"; umask 022
    nn=$(cat "$scratch/nonce")
    set -- --bin "$scratch/sheepdog" --version "$xyz" --build "$c" --tag "$tag" --commit "$short" --dest "$dest" --real "$nn" --nonce-file "$scratch/nonce" --parent-pid "$$"
    [ "$mode" = control ] && set -- "$@" --no-notarize
    # in the foreground: a background child of a non-interactive sh ignores INT, so ctrl-C would
    # not reach it (measured); env execs sign.sh, so its parent is still this shell
    env -i HOME="${HOME:-}" PATH="$PATH" TMPDIR="${TMPDIR:-/tmp}" USER="${USER:-}" LOGNAME="${LOGNAME:-}" \
      ${DEVELOPER_DIR:+DEVELOPER_DIR="$DEVELOPER_DIR"} "$S/lib/sign.sh" "$@" || die "signing failed"
  fi

  # install.sh (PHASE3.md S3), rendered with this release's version and the door
  step "install.sh, the checksums, the npm packages"
  files="sheepdog-macos-universal.tar.gz sheepdog-linux-aarch64 sheepdog-linux-x86_64"
  if [ -f "$S/install.sh" ]; then
    tool base sh "$S/lib/render-install.sh" "$S/install.sh" "${tag#v}" "$dest/install.sh" || die "render install.sh"
    files="$files install.sh"
  fi
  (cd "$dest" && shasum -a 256 $files > SHA256SUMS) || die "SHA256SUMS"
  npm_pack || die "npm packing"
  {
    printf '{\n  "v": 1,\n  "tag": "%s",\n  "commit": "%s",\n  "mode": "%s",\n  "control": %s,\n  "files": [\n' "$tag" "$commit" "$mode" "$( [ "$mode" = control ] && echo true || echo false)"
    sep=""
    for f in $files $files_npm; do
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
  echo "release: $tag built ($mode) in $dest"
}

# --- publish: the thin executor behind the gate (PHASE3.md §1.2) ------------------------------
# The remote tag is read from github.com/lukaso/sheepdog itself (the repository the release is
# made on), never from `origin`. GH and GITCMD are the gh and git to use: the real ones for
# publish; for __publish-dry (the cells), stand-ins under /private/tmp/sd-p3-fixtures.*, given by
# path, so the dry run can never reach the real gh.
UPSTREAM=https://github.com/lukaso/sheepdog
confirm() { # prompt -> 0 on the tag typed back
  if [ "${DRY:-no}" = yes ]; then
    n=$(grep -c '^ask ' "${SD_ASK_RECORD:-/dev/null}" 2>/dev/null); n=${n:-0}
    a=$(sed -n "$((n + 1))p" "${SD_ASK_SCRIPT:-/dev/null}")
    echo "ask $1 -> $a" >> "${SD_ASK_RECORD:-/dev/null}"
  else
    printf '%s ' "$1" > /dev/tty; IFS= read -r a < /dev/tty || return 1
  fi
  [ "$a" = "$tag" ]
}
publish_exec() { # dir
  d=$1
  pt=$(mktemp -d /private/tmp/sd-publish.XXXXXX) || die "no temp dir"
  trap 'rm -rf "$pt" ${DRYHOME:+"$DRYHOME"}' EXIT; trap 'rm -rf "$pt" ${DRYHOME:+"$DRYHOME"}; exit 1' HUP INT TERM
  tool "$NETC" "$GITCMD" ls-remote "$UPSTREAM" "refs/tags/$tag*" > "$pt/remote" || die "git ls-remote $UPSTREAM"
  tool "$NETC" "$GH" api "repos/lukaso/sheepdog/releases" --paginate --jq '.[].tag_name' > "$pt/releases" || die "gh: cannot list the releases"
  (cd "$root" && sh scripts/release-plan.sh --out "$d" --tag "$tag" --remote "$pt/remote" --releases "$pt/releases") > "$pt/plan" || die "the planner refused"
  sh "$root/scripts/release-plan.sh" --validate "$pt/plan" || die "the plan does not validate"
  echo "release: the plan:"; sed 's/^/  /' "$pt/plan"
  set -- $(sed -n 's/^POST [^ ]* //p' "$pt/plan")
  id=$(tool "$NETC" "$GH" api -X POST repos/lukaso/sheepdog/releases "$@" --jq .id) || die "gh: cannot make the draft"
  case $id in ''|*[!0-9]*) die "gh: no release id ($id)" ;; esac
  echo "release: draft $id made"
  for f in $(sed -n 's/^UPLOAD //p' "$pt/plan"); do
    tool "$NETC" "$GH" api -X POST -H 'Content-Type: application/octet-stream' \
      "https://uploads.github.com/repos/lukaso/sheepdog/releases/$id/assets?name=$f" --input "$d/$f" --jq .id >/dev/null \
      || die "gh: upload of $f failed (the draft $id is not public; delete it on GitHub)"
  done
  tool "$NETC" "$GH" api "repos/lukaso/sheepdog/releases/$id" --jq '.assets[] | "\(.id) \(.name)"' > "$pt/assets" || die "gh: cannot read the draft"
  [ "$(awk '{print $2}' "$pt/assets" | sort | tr '\n' ' ')" = "SHA256SUMS install.sh sheepdog-linux-aarch64 sheepdog-linux-x86_64 sheepdog-macos-universal.tar.gz " ] \
    || die "the draft's assets are not exactly the five: $(awk '{print $2}' "$pt/assets" | tr '\n' ' ')"
  while read -r aid name; do
    tool "$NETC" "$GH" api -H 'Accept: application/octet-stream' "repos/lukaso/sheepdog/releases/assets/$aid" > "$pt/dl" || die "gh: cannot download $name"
    [ "$(shasum -a 256 "$pt/dl" | cut -d' ' -f1)" = "$(shasum -a 256 "$d/$name" | cut -d' ' -f1)" ] || die "the uploaded $name differs from the local one; the draft $id stays a draft"
  done < "$pt/assets"
  tool "$NETC" "$GITCMD" ls-remote "$UPSTREAM" "refs/tags/$tag*" > "$pt/remote2" || die "git ls-remote $UPSTREAM"
  cmp -s "$pt/remote" "$pt/remote2" || die "the remote tag changed during the upload; the draft $id stays a draft"
  confirm "release: the draft $id holds the five files, each matching. Type the tag again to make it public:" || die "not confirmed; the draft $id stays a draft"
  tool "$NETC" "$GH" api -X PATCH "repos/lukaso/sheepdog/releases/$id" -F draft=false >/dev/null || die "gh: cannot publish the draft $id"
  echo "release: $tag published"
}
publish() {
  verify "$out/$tag"
  GH=gh GITCMD=git DRY=no NETC=net publish_exec "$out/$tag"
}
publish_dry() { # the cells' entry: stand-ins by path only, never the real gh; verify is not run
  # a stand-in is a script (not a symlink, not a binary), whose real directory is a fixture dir;
  # and every call runs with a fresh temp HOME and nothing else of the caller's (the `dry` class)
  for v in SD_PUBLISH_DRY_GH SD_PUBLISH_DRY_GIT; do
    eval "p=\${$v:-}"
    [ -n "$p" ] && [ -f "$p" ] && [ ! -L "$p" ] && [ -x "$p" ] || die "$v is not an executable file (and not a symlink)"
    pd=$(cd -P "$(dirname "$p")" 2>/dev/null && pwd -P) || die "$v: cannot resolve its directory"
    case $pd/ in /private/tmp/sd-p3-fixtures.*/) ;; *) die "$v must be a stand-in in a /private/tmp/sd-p3-fixtures.* directory, not in $pd" ;; esac
    [ "$(head -c 2 "$p")" = '#!' ] || die "$v must be a script stand-in"
    eval "$v=\$pd/\$(basename \"\$p\")"
  done
  DRYHOME=$(mktemp -d /private/tmp/sd-dryhome.XXXXXX) || die "no temp HOME"
  echo "release: __publish-dry (stand-ins; verify not run; a temp HOME)"
  GH=$SD_PUBLISH_DRY_GH GITCMD=$SD_PUBLISH_DRY_GIT DRY=yes NETC=dry publish_exec "$out/$tag"
  rm -rf "$DRYHOME"
}

# --- npm-check: before any `npm publish` (PHASE3.md §5 step 5) ----------------------------------
npm_check() { # dir
  . "$root/scripts/release.conf" || die "cannot read scripts/release.conf"
  . "$root/scripts/lib/realtools.sh" || die "cannot read realtools.sh"
  d=$1 m=$1/MANIFEST.json nv=${tag#v}
  [ -f "$m" ] || die "no $m"
  mode=$(sed -n 's/^ *"mode": *"\([^"]*\)",*$/\1/p' "$m"); ctl=$(sed -n 's/^ *"control": *\([a-z]*\),*$/\1/p' "$m")
  mtag=$(sed -n 's/^ *"tag": *"\([^"]*\)",*$/\1/p' "$m")
  [ "$mtag" = "$tag" ] || die "the manifest is for $mtag, not $tag"
  [ "$mode" = signed ] || die "the manifest's mode is '$mode', not signed"
  [ "$ctl" = false ] || die "the manifest is a control build"
  for p in sheepdog sheepdog-darwin-universal sheepdog-linux-arm64 sheepdog-linux-x64; do
    f=lukaso-$p-$nv.tgz
    [ -f "$d/$f" ] || die "missing $f"
    h=$(shasum -a 256 "$d/$f" | cut -d' ' -f1)
    grep -q "\"name\": \"$f\", \"sha256\": \"$h\"" "$m" || die "$f does not match its manifest hash"
  done
  nt=/private/tmp/sd-npmcheck.$$.$(od -An -N4 -tx4 /dev/urandom | tr -d ' ')
  trap 'rm -rf "$nt"' EXIT; trap 'rm -rf "$nt"; exit 1' HUP INT TERM
  mkdir -m 700 "$nt" "$nt/p" "$nt/a" || die "no temp dir"
  tar -xzf "$d/lukaso-sheepdog-darwin-universal-$nv.tgz" -C "$nt/p" || die "cannot unpack the darwin package"
  tar -xzf "$d/sheepdog-macos-universal.tar.gz" -C "$nt/a" || die "cannot unpack the release archive"
  b=$nt/p/package/Sheepdog.app
  rt_meets "$b" && rt_meets "$b/Contents/MacOS/sheepdog" || die "codesign: the darwin package's bundle does not meet the release requirement"
  rt_staple_ok "$b" || die "stapler: the darwin package's bundle has no valid staple ticket"
  rt_spctl_ok "$b" || die "spctl: Gatekeeper rejects the darwin package's bundle"
  c1=$(rt_cdhash "$b"); c2=$(rt_cdhash "$nt/a/Sheepdog.app")
  [ -n "$c1" ] && [ "$c1" = "$c2" ] || die "the darwin package's bundle ($c1) is not the release archive's ($c2)"
  rm -rf "$nt"; trap - EXIT
  echo "release: the npm tarballs of $tag check out; publish them, platform packages first"
}

# --- verify: the three facts, from the archive, by the real tools (PHASE3.md §1.2) --------------
verify() { # dir -> exit 1 naming the tool that refused
  . "$root/scripts/release.conf" || die "cannot read scripts/release.conf"
  . "$root/scripts/lib/realtools.sh" || die "cannot read realtools.sh"
  d=$1 arc=$1/sheepdog-macos-universal.tar.gz
  [ -f "$arc" ] || die "no $arc"
  vt=/private/tmp/sd-verify.$$.$(od -An -N4 -tx4 /dev/urandom | tr -d ' ')
  trap 'rm -rf "$vt"' EXIT; trap 'rm -rf "$vt"; exit 1' HUP INT TERM
  mkdir -m 700 "$vt" && tar -xzf "$arc" -C "$vt" || die "cannot unpack $arc"
  a=$vt/Sheepdog.app
  rt_meets "$a" && rt_meets "$a/Contents/MacOS/sheepdog" || die "codesign: the bundle does not meet the release requirement"
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
  npm-check) npm_check "$out/$tag" ;;
esac
