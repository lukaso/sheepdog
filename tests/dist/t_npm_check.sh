#!/bin/sh
# PHASE3.md §5 step 5: `release.sh npm-check vTAG` before any `npm publish`: the manifest is this
# tag's signed, non-control build; the release archive and the four .tgz files are there and each
# matches its manifest hash; the darwin package's bundle passes the real-tool checks, and the
# release archive's bundle meets the requirement and is byte-identical to it. Refused (exit 1)
# here: an unsigned build, a control build, a tarball or a release archive whose hash is not the
# manifest's, a missing tarball, and an unsigned bundle in the darwin package (by codesign).
# (The rc leg adds, on real builds: rc.1 accepted; a tampered or another Developer ID archive
# refused; the control refused.)
set -u
. "$(dirname "$0")/lib.sh"
[ "$(uname -s)" = Darwin ] || { echo "SKIP (macOS only)"; exit 0; }
fx_dir; fx_repo
fx_release 0.1.0 1 v0.1.0
nv=0.1.0 D=$FX/out/v0.1.0
printf 'int main(){return 0;}\n' > "$FX/m.c"; cc -o "$FX/bin" "$FX/m.c" || exit 3
app=$("$SD_ROOT/scripts/bundle.sh" "$FX/bin" "$FX/b" 0.1.0 1) || exit 3
# the packages come from scripts/lib/npm-pack.sh (what the build runs), around a .dev bundle and
# two stand-in Linux binaries that are also in the output dir, as a build leaves them
mk() { # mode control
  rm -rf "$D"; mkdir -p "$D"
  "$SD_ROOT/scripts/lib/archive.sh" make "$app" "$D/sheepdog-macos-universal.tar.gz" || exit 3
  printf '#!/bin/sh\necho aarch64\n' > "$D/sheepdog-linux-aarch64"; printf '#!/bin/sh\necho x86_64\n' > "$D/sheepdog-linux-x86_64"
  chmod 755 "$D/sheepdog-linux-aarch64" "$D/sheepdog-linux-x86_64"
  sh "$SD_ROOT/scripts/lib/npm-pack.sh" "$nv" "$D/sheepdog-macos-universal.tar.gz" "$D/sheepdog-linux-aarch64" "$D/sheepdog-linux-x86_64" "$SD_ROOT/npm/sheepdog/bin/sheepdog" "$D" >/dev/null || exit 3
  { printf '{\n  "v": 1,\n  "tag": "v0.1.0",\n  "commit": "%s",\n  "mode": "%s",\n  "control": %s,\n  "files": [\n' "$(g rev-parse 'v0.1.0^{commit}')" "$1" "$2"
    sep=""; for f in "$D"/sheepdog-macos-universal.tar.gz "$D"/sheepdog-linux-aarch64 "$D"/sheepdog-linux-x86_64 "$D"/*.tgz; do printf '%s    {"name": "%s", "sha256": "%s"}' "$sep" "$(basename "$f")" "$(shasum -a 256 "$f" | cut -d' ' -f1)"; sep=",
"; done; printf '\n  ]\n}\n'; } > "$D/MANIFEST.json"
}
nc() { (cd "$REPO" && env HOME="$FX/ghome" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 sh scripts/release.sh npm-check --out "$FX/out" v0.1.0) > "$FX/o" 2>&1; }
mk unsigned false; nc; r=$?; [ $r = 1 ] && grep -q "mode" "$FX/o" && pass "an unsigned build: refused (its mode)" || fail "unsigned: rc=$r $(tail -1 "$FX/o")"
mk signed true; nc; r=$?; [ $r = 1 ] && grep -q "control" "$FX/o" && pass "a control build: refused (the control flag)" || fail "control: rc=$r $(tail -1 "$FX/o")"
mk signed false; echo x >> "$D/lukaso-sheepdog-linux-x64-$nv.tgz"; nc; r=$?
[ $r = 1 ] && grep -q 'hash' "$FX/o" && pass "a tarball not matching its manifest hash: refused" || fail "hash: rc=$r $(tail -1 "$FX/o")"
mk signed false; echo x >> "$D/sheepdog-macos-universal.tar.gz"; nc; r=$?
[ $r = 1 ] && grep -q 'sheepdog-macos-universal.tar.gz does not match its manifest hash' "$FX/o" && pass "a release archive not matching its manifest hash: refused" || fail "archive hash: rc=$r $(tail -1 "$FX/o")"
mk signed false; rm "$D/lukaso-sheepdog-$nv.tgz"; nc; r=$?; [ $r = 1 ] && grep -q "missing" "$FX/o" && pass "a missing tarball: refused (named missing)" || fail "missing: rc=$r $(tail -1 "$FX/o")"
# each package read raw, as npm reads it (plain ustar entries only; every installed byte accounted
# for): each variant with its manifest hash updated, so the hash check passes, and each refused
# before any real-tool check
E="$SD_ROOT/tests/lib/tgz-edit.py"
DT=lukaso-sheepdog-darwin-universal-$nv.tgz MT=lukaso-sheepdog-$nv.tgz XT=lukaso-sheepdog-linux-x64-$nv.tgz
resum() { h=$(shasum -a 256 "$D/$1" | cut -d' ' -f1); sed "s/\(\"name\": \"$1\", \"sha256\": \"\)[0-9a-f]*/\1$h/" "$D/MANIFEST.json" > "$D/m.n" && mv "$D/m.n" "$D/MANIFEST.json"; }
P=package/Sheepdog.app/Contents
for v in "DT|add|zzz/Sheepdog.app/Contents/MacOS/sheepdog|NOT THE RELEASE|outside package/" \
         "DT|add|$P/MacOS/._sheepdog|x|AppleDouble" \
         "DT|symlink|$P/MacOS/link|sheepdog|type '2'" \
         "DT|after-null|$P/Resources/marker.txt|x|after the end" \
         "DT|raw-append|$P/Resources/px|x,ustar00,-|type 'x'" \
         "DT|raw-append|$P/Resources/pg|g,ustar00,-|type 'g'" \
         "DT|raw-append|$P/Resources/ln|L,ustar00,-|type 'L'" \
         "DT|raw-append|$P/Resources/sp|S,ustar00,-|type 'S'" \
         "DT|raw-append|$P/Resources/gm|0,gnu,-|not a plain ustar header" \
         "DT|raw-append|Resources/pf|0,ustar00,package/Sheepdog.app/Contents|a ustar prefix" \
         "DT|raw-append|$P/Resources/a b|0,ustar00,-|a name outside" \
         "DT|badsum|$P/Resources/bs|-|bad header checksum" \
         "DT|raw-append|$P/MacOS/SHEEPDOG|0,ustar00,-|appears twice (case ignored)" \
         "DT|replace|package/LICENSE-MIT|not the license|LICENSE-MIT differs from the repo" \
         "DT|json-set|package/package.json|scripts={\"postinstall\": \"echo hi\"}|package.json: key scripts" \
         "MT|replace|package/bin/sheepdog|#!/bin/sh|bin/sheepdog differs from the repo's launcher" \
         "MT|json-set|package/package.json|optionalDependencies={\"@lukaso/sheepdog-darwin-universal\": \"*\"}|optionalDependencies" \
         "XT|replace|package/bin/sheepdog|#!/bin/sh|bin/sheepdog differs from sheepdog-linux-x86_64"; do
  k=${v%%|*} rest=${v#*|}; op=${rest%%|*} rest=${rest#*|}; nm=${rest%%|*} rest=${rest#*|}; arg=${rest%%|*} why=${rest#*|}
  eval "t=\$$k"
  mk signed false; python3 "$E" "$D/$t" "$FX/e.tgz" "$op" "$nm" "$arg" && mv "$FX/e.tgz" "$D/$t" && resum "$t" || fail "could not make the $op variant of $t"
  nc; r=$?
  [ $r = 1 ] && grep -q "$why" "$FX/o" && ! grep -q codesign "$FX/o" && pass "$t with $op $nm: refused ($why), before any real-tool check" || fail "$t $op $nm: rc=$r $(tail -1 "$FX/o")"
done
mk signed false; nc; r=$?
[ $r = 1 ] && grep -q 'codesign' "$FX/o" && pass "an unsigned bundle in the darwin package: refused, by codesign" || fail "unsigned bundle: rc=$r $(tail -1 "$FX/o")"
finish
