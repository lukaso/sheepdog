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
mk() { # mode control
  rm -rf "$D"; mkdir -p "$D" "$FX/pk"
  "$SD_ROOT/scripts/lib/archive.sh" make "$app" "$D/sheepdog-macos-universal.tar.gz" || exit 3
  for p in sheepdog sheepdog-darwin-universal sheepdog-linux-arm64 sheepdog-linux-x64; do
    rm -rf "$FX/pk/$p"; mkdir -p "$FX/pk/$p"
    case $p in sheepdog-darwin-universal) tar -xzf "$D/sheepdog-macos-universal.tar.gz" -C "$FX/pk/$p"; cp "$SD_ROOT/LICENSE-MIT" "$SD_ROOT/LICENSE-APACHE" "$FX/pk/$p/"
        fl='"Sheepdog.app", "LICENSE-MIT", "LICENSE-APACHE"' ;;
      *) mkdir -p "$FX/pk/$p/bin"; printf '#!/bin/sh\n' > "$FX/pk/$p/bin/sheepdog"; chmod 755 "$FX/pk/$p/bin/sheepdog"; fl='"bin"' ;; esac
    printf '{"name":"@lukaso/%s","version":"%s","files":[%s]}\n' "$p" "$nv" "$fl" > "$FX/pk/$p/package.json"
    (cd "$FX/pk/$p" && env npm_config_cache="$FX/npmc" HOME="$FX/ghome" npm pack --silent --pack-destination "$D" >/dev/null) || exit 3
  done
  { printf '{\n  "v": 1,\n  "tag": "v0.1.0",\n  "commit": "%s",\n  "mode": "%s",\n  "control": %s,\n  "files": [\n' "$(g rev-parse 'v0.1.0^{commit}')" "$1" "$2"
    sep=""; for f in "$D"/sheepdog-macos-universal.tar.gz "$D"/*.tgz; do printf '%s    {"name": "%s", "sha256": "%s"}' "$sep" "$(basename "$f")" "$(shasum -a 256 "$f" | cut -d' ' -f1)"; sep=",
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
# the darwin package's shape, read raw (macOS tar hides AppleDouble entries; npm drops the first
# path part): each variant with its manifest hash updated, so the hash check passes
E="$SD_ROOT/tests/lib/tgz-edit.py"; DT=lukaso-sheepdog-darwin-universal-$nv.tgz
resum() { h=$(shasum -a 256 "$D/$DT" | cut -d' ' -f1); sed "s/\(\"name\": \"$DT\", \"sha256\": \"\)[0-9a-f]*/\1$h/" "$D/MANIFEST.json" > "$D/m.n" && mv "$D/m.n" "$D/MANIFEST.json"; }
for v in "add|zzz/Sheepdog.app/Contents/MacOS/sheepdog|NOT THE RELEASE|outside package/" \
         "add|package/Sheepdog.app/Contents/MacOS/._sheepdog|x|AppleDouble" \
         "symlink|package/Sheepdog.app/Contents/MacOS/link|sheepdog|not a regular file"; do
  op=${v%%|*} rest=${v#*|}; nm=${rest%%|*} rest=${rest#*|}; arg=${rest%%|*} why=${rest#*|}
  mk signed false; python3 "$E" "$D/$DT" "$FX/e.tgz" "$op" "$nm" "$arg" && mv "$FX/e.tgz" "$D/$DT" && resum || fail "could not make the $op variant"
  nc; r=$?
  [ $r = 1 ] && grep -q "$why" "$FX/o" && ! grep -q codesign "$FX/o" && pass "the darwin package with $op $nm: refused ($why), before any real-tool check" || fail "$op $nm: rc=$r $(tail -1 "$FX/o")"
done
mk signed false; nc; r=$?
[ $r = 1 ] && grep -q 'codesign' "$FX/o" && pass "an unsigned bundle in the darwin package: refused, by codesign" || fail "unsigned bundle: rc=$r $(tail -1 "$FX/o")"
finish
