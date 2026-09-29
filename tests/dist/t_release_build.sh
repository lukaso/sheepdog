#!/bin/sh
# PHASE3.md S2: `release.sh build vTAG` (unsigned) for real, on a tagged throwaway copy of this
# repo: the checks, a fresh worktree of the tag, fresh CARGO_HOMEs per toolchain, the universal Mac
# build (minos 12.0 on both slices) bundled with the .dev ID and archived (archive.sh check), the
# two Linux binaries built offline in the pinned image (no PT_INTERP; they run in an empty image),
# every artifact's --version naming the tag's commit, every recorded rustc the pinned one,
# SHA256SUMS over the release files, MANIFEST.json. An existing output directory is refused. The
# worktree is gone afterwards.
set -u
. "$(dirname "$0")/lib.sh"
[ "$(uname -s)" = Darwin ] || { echo "SKIP (macOS only)"; exit 0; }
fx_dir; fx_repo
fx_release 0.1.0 1 v0.1.0-rc.1
short=$(g rev-parse --short=12 "v0.1.0-rc.1^{commit}")
out=$FX/out
(cd "$REPO" && env -u RUSTUP_TOOLCHAIN HOME="$FX/ghome" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" DOCKER_CONFIG="${DOCKER_CONFIG:-$HOME/.docker}" \
  timeout 3000 sh scripts/release.sh build --out "$out" v0.1.0-rc.1) > "$FX/log" 2>&1; rc=$?
[ $rc = 0 ] && pass "build: rc 0" || { fail "build: rc=$rc"; tail -20 "$FX/log"; finish; }
echo "expect commit $short; the build said: $(grep '^release: building' "$FX/log")"
D=$out/v0.1.0-rc.1-unsigned
for f in sheepdog-macos-universal.tar.gz sheepdog-linux-aarch64 sheepdog-linux-x86_64 SHA256SUMS MANIFEST.json; do
  [ -s "$D/$f" ] && pass "$f made" || fail "$f missing"
done
"$SD_ROOT/scripts/lib/archive.sh" check "$D/sheepdog-macos-universal.tar.gz" && pass "the archive passes archive.sh check" || fail "the archive fails its check"
mkdir "$FX/x" && tar -xzf "$D/sheepdog-macos-universal.tar.gz" -C "$FX/x"
app=$FX/x/Sheepdog.app
[ "$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$app/Contents/Info.plist")" = com.lukaso.sheepdog.dev ] && pass "the unsigned bundle has the .dev ID" || fail "the unsigned bundle's ID"
for a in arm64 x86_64; do
  m=$(vtool -arch $a -show-build "$app/Contents/MacOS/sheepdog" 2>/dev/null | awk '$1=="minos"{print $2; exit}')
  [ "$m" = 12.0 ] && pass "$a minos 12.0" || fail "$a minos '$m'"
done
mkdir -p "$FX/rh"
v=$("$SD_ROOT/scripts/lib/release-run.sh" "$FX/rh" "$app/Contents/MacOS/sheepdog" --version 2>&1)
case $v in *"$short"*) pass "the Mac binary names the tag's commit" ;; *) fail "the Mac binary says: $v" ;; esac
. "$SD_ROOT/scripts/release.conf"
for a in aarch64 x86_64; do
  b=$D/sheepdog-linux-$a
  case $a in aarch64) pf=linux/arm64 ;; *) pf=linux/amd64 ;; esac
  timeout 120 docker run --rm --pull=never --network none -v "$b":/b:ro "$SD_IMG_ALPINE" sh -c 'readelf -l /b | grep -q INTERP' \
    && fail "$a has a PT_INTERP (dynamic)" || pass "$a has no PT_INTERP"
  v=$(timeout 120 docker run --rm --pull=never --network none --platform $pf -v "$b":/sheepdog:ro "sd-scratch:empty-${pf#linux/}" /sheepdog --version 2>&1)
  case $v in *"$short"*) pass "$a runs in an empty image and names the tag's commit" ;; *) fail "$a in an empty image: $v" ;; esac
done
(cd "$D" && shasum -a 256 -c SHA256SUMS >/dev/null 2>&1) && pass "SHA256SUMS matches" || fail "SHA256SUMS does not match"
n=$(grep -c . "$D/SHA256SUMS"); [ "$n" -ge 3 ] && pass "SHA256SUMS has $n lines" || fail "SHA256SUMS has $n lines"
pin=$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$SD_ROOT/rust-toolchain.toml")
rs=$(sed -n 's/.*"rustc": *"\([^"]*\)".*/\1/p' "$D/MANIFEST.json" | sort -u)
[ -n "$rs" ] && [ "$(printf '%s\n' "$rs" | grep -vc "^rustc $pin ")" = 0 ] && pass "every recorded rustc is $pin" || fail "recorded rustc: $rs"
grep -q "\"commit\": *\"$(g rev-parse v0.1.0-rc.1^{commit})\"" "$D/MANIFEST.json" && pass "the manifest names the commit" || fail "the manifest's commit"
grep -q '"control": *false' "$D/MANIFEST.json" && pass "the manifest is not a control" || fail "the manifest's control flag"
[ "$(g worktree list | grep -c .)" = 1 ] && pass "no worktree left" || fail "a worktree left: $(g worktree list)"
(cd "$REPO" && env HOME="$FX/ghome" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 timeout 60 sh scripts/release.sh build --out "$out" v0.1.0-rc.1) >/dev/null 2>&1 \
  && fail "an existing output directory was reused" || pass "an existing output directory is refused"
finish
