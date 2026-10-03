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
# the bundle verify judges is the archive's own: with a lying `tar` first on PATH that unpacks a
# plain bundle, an archive holding a control build is still refused as one (the system's tar)
mkdir -p "$FX/mk" "$FX/liar-tar" && cp -R "$app" "$FX/mk/" && /usr/bin/plutil -insert SheepdogControlBuild -bool true "$FX/mk/Sheepdog.app/Contents/Info.plist" || exit 3
cp "$D/sheepdog-macos-universal.tar.gz" "$FX/plain.tar.gz" && "$SD_ROOT/scripts/lib/archive.sh" make "$FX/mk/Sheepdog.app" "$D/sheepdog-macos-universal.tar.gz" || exit 3
printf '#!/bin/sh\nd=""; while [ $# -gt 0 ]; do [ "$1" = -C ] && d=$2; shift; done\nexec /usr/bin/tar -xzf "%s" -C "$d"\n' "$FX/plain.tar.gz" > "$FX/liar-tar/tar"; chmod 755 "$FX/liar-tar/tar"
v env PATH="$FX/liar-tar:$PATH"; rc=$?
[ $rc = 1 ] && grep -q 'a control build' "$FX/o" && pass "a lying tar on PATH: the archive's control build is still refused as one" || fail "lying tar: rc=$rc $(tail -1 "$FX/o")"
cp "$FX/plain.tar.gz" "$D/sheepdog-macos-universal.tar.gz"
# the staple check itself, with a lying `env` first on PATH (it would say yes to anything): the
# real tool still answers (no staple ticket on a plain directory)
mkdir -p "$FX/liar-env" "$FX/plain"; printf '#!/bin/sh\nexit 0\n' > "$FX/liar-env/env"; chmod 755 "$FX/liar-env/env"
[ "$(PATH="$FX/liar-env:$PATH" sh -c 'command -v env')" = "$FX/liar-env/env" ] || fail "premise: the lying env is not first on PATH"
r=$(PATH="$FX/liar-env:$PATH" sh -c '. "$1/scripts/release.conf"; . "$1/scripts/lib/realtools.sh"; rt_staple_ok "$2"; echo $?' sh "$SD_ROOT" "$FX/plain")
[ "$r" != 0 ] && [ -n "$r" ] && pass "rt_staple_ok with a lying env on PATH: still refuses (rc $r)" || fail "rt_staple_ok with a lying env: rc '$r'"
# a control build's bundle (the control marker) is never a release: refused before any real tool
mkdir -p "$FX/mk" && cp -R "$app" "$FX/mk/" && /usr/bin/plutil -insert SheepdogControlBuild -bool true "$FX/mk/Sheepdog.app/Contents/Info.plist" || exit 3
"$SD_ROOT/scripts/lib/archive.sh" make "$FX/mk/Sheepdog.app" "$D/sheepdog-macos-universal.tar.gz.m" || exit 3
mv "$D/sheepdog-macos-universal.tar.gz" "$D/u.tgz" && mv "$D/sheepdog-macos-universal.tar.gz.m" "$D/sheepdog-macos-universal.tar.gz"
v env; rc=$?
[ $rc = 1 ] && grep -q 'a control build' "$FX/o" && ! grep -q codesign "$FX/o" && pass "a control build's archive (the marker): refused before any real-tool check" || fail "marked: rc=$rc $(tail -1 "$FX/o")"
mv "$D/u.tgz" "$D/sheepdog-macos-universal.tar.gz"
finish
