#!/bin/sh
# PHASE3.md §1.2: `release.sh verify vTAG` works out the three facts of a release from its archive,
# never from the manifest: the bundle meets the release requirement, the staple ticket validates,
# and spctl accepts it — with the real tools by absolute path (scripts/lib/realtools.sh), so a
# shim on PATH cannot answer. Here: an unsigned build's archive is refused (by codesign), also
# with a lying codesign, xcrun and spctl first on PATH. (The rc leg adds: rc.1 accepted; the
# --no-notarize control refused by stapler and spctl.)
set -u
. "$(dirname "$0")/lib.sh"
[ "$(uname -s)" = Darwin ] || { echo "SKIP (macOS only)"; exit 0; }
fx_dir; fx_repo
fx_release 0.1.0 1 v0.1.0
# an output directory as the unsigned build makes it, but named as the signed one
D=$FX/out/v0.1.0; mkdir -p "$D"
printf 'int main(){return 0;}\n' > "$FX/m.c"; cc -o "$FX/bin" "$FX/m.c" || exit 3
app=$("$SD_ROOT/scripts/bundle.sh" "$FX/bin" "$FX/b" 0.1.0 1) || exit 3
"$SD_ROOT/scripts/lib/archive.sh" make "$app" "$D/sheepdog-macos-universal.tar.gz" || exit 3
v() { (cd "$REPO" && env HOME="$FX/ghome" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 "$@" sh scripts/release.sh verify --out "$FX/out" v0.1.0) > "$FX/o" 2>&1; }
v env; rc=$?
[ $rc = 1 ] && grep -q 'codesign' "$FX/o" && pass "an unsigned archive: refused, by codesign" || fail "unsigned: rc=$rc $(tail -1 "$FX/o")"
mkdir -p "$FX/liar"
for t in codesign xcrun spctl; do printf '#!/bin/sh\necho "valid on disk"\nexit 0\n' > "$FX/liar/$t"; chmod +x "$FX/liar/$t"; done
v env PATH="$FX/liar:$PATH"; rc=$?
[ $rc = 1 ] && grep -q 'codesign' "$FX/o" && pass "with lying codesign, xcrun and spctl on PATH: still refused, by codesign" || fail "liars: rc=$rc $(tail -1 "$FX/o")"
v env DEVELOPER_DIR="$FX/liar-dev"; rc=$?
[ $rc = 1 ] && pass "with a DEVELOPER_DIR set: still refused" || fail "DEVELOPER_DIR: rc=$rc"
finish
