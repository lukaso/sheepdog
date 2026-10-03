#!/bin/sh
# PHASE3.md §3 `rc` leg, the release checks, against the operator's rc output (SD_RC_DIR) and its
# control (SD_RC_CONTROL_DIR), read only:
#   - verify (the executor's real-tool step): the rc accepted; the control refused by its control
#     marker (rc.1's, which predates the marker: by stapler), and Gatekeeper (spctl) rejects the
#     control's bundle too; a copy of the rc without its staple ticket refused by stapler even with
#     a DEVELOPER_DIR whose xcrun says yes to everything;
#   - npm-check (each run on a copy): the rc accepted, and its stamp written (the tag and the
#     manifest's sha256: publish refuses without it); a copy whose release archive does not match the manifest
#     refused by its hash; a tampered archive (the manifest updated) refused by the comparison;
#     the control's archive in the release's place refused by the archive check (no staple
#     ticket); the control refused by its mode; a copy of the control with the mode and control
#     flag cleared refused by its marker (rc.1's: by stapler); the rc without its staple ticket in
#     both tarballs (manifest updated) refused by stapler, so that check has a row on any rc;
#   - the control is not the release: it carries the control marker, and its CDHash differs on
#     both slices (rc.1's control does neither: it was built from a tag before the marker);
#   - the planner accepts the rc's manifest (with this checkout's tag as the remote tag) and its
#     plan passes --validate;
#   - the door: the rc's executable allowed; a copy with a changed Info.plist byte, and a copy
#     with an added file, refused for the missing release signature; the rc bundle inside an
#     unsigned outer bundle refused by the enclosing-bundle check (the executable and its own
#     bundle meet the requirement; the outer bundle does not).
set -u
. "$(dirname "$0")/../dist/lib.sh"
[ "$(uname -s)" = Darwin ] || { echo "FAIL: the rc leg needs macOS"; exit 1; }
fx_dir
RC=$SD_RC_DIR CT=$SD_RC_CONTROL_DIR
tag=$(basename "$RC")
[ "$(basename "$CT")" = "$tag-control" ] || fail "the control dir is not $tag-control: $CT"
rel() { (cd "$SD_ROOT" && env HOME="$FX/h" sh scripts/release.sh "$@" < /dev/null); }
# copy DIR to $FX/<name>/$tag; setsum NAME FILE: write FILE's sha256 into that copy's manifest
cpy() { # a source with a symlink is refused (a later write into the copy would go through it)
  [ -z "$(find "$1" -type l)" ] || { fail "cpy: $1 holds a symlink"; return 1; }
  mkdir -p "$FX/$2/$tag" && cp -R "$1/." "$FX/$2/$tag/"; }
mkdir -p "$FX/sl" && ln -s /nonexistent "$FX/sl/l" && (FAILS=0; cpy "$FX/sl" slc >/dev/null; [ $FAILS = 1 ]) && [ ! -e "$FX/slc" ] \
  && pass "cpy refuses a source that holds a symlink" || fail "cpy copied a source with a symlink"
setsum() {
  m=$FX/$1/$tag/MANIFEST.json h=$(shasum -a 256 "$FX/$1/$tag/$2" | cut -d' ' -f1)
  sed "s/\(\"name\": \"$2\", \"sha256\": \"\)[0-9a-f]*/\1$h/" "$m" > "$m.n" && mv "$m.n" "$m"
  grep -q "\"$2\", \"sha256\": \"$h\"" "$m" || fail "setsum $1 $2 did not write the hash"
}
mkdir -p "$FX/h"
. "$SD_ROOT/scripts/release.conf"; . "$SD_ROOT/scripts/lib/realtools.sh"

# verify
rel verify --out "$(dirname "$RC")" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 0 ] && pass "verify: the rc is signed, notarized and stapled" || fail "verify rc: $r $(tail -1 "$FX/o")"
cpy "$CT" c
mkdir -p "$FX/cb" && tar -xzf "$CT/sheepdog-macos-universal.tar.gz" -C "$FX/cb" || fail "cannot unpack the control"
# a marked control (rc.2 on) is refused by its marker before any real tool; rc.1's, which predates
# the marker, by stapler (the marker row below says which this control is)
marked=no; /usr/bin/plutil -extract SheepdogControlBuild raw -o - "$FX/cb/Sheepdog.app/Contents/Info.plist" >/dev/null 2>&1 && marked=yes
rel verify --out "$FX/c" "$tag" > "$FX/o" 2>&1; r=$?
if [ $marked = yes ]; then
  [ $r = 1 ] && grep -q 'the archive holds a control build' "$FX/o" && ! grep -q -e codesign -e stapler "$FX/o" && pass "verify: the (marked) control refused by its marker" || fail "verify control: $r $(tail -1 "$FX/o")"
else
  [ $r = 1 ] && grep -q 'stapler' "$FX/o" && pass "verify: the (unmarked) control refused by stapler" || fail "verify control: $r $(tail -1 "$FX/o")"
fi
# the control must differ from the release: its marker, and so its CDHashes (a reproducible build
# of the same commit shares them, and Gatekeeper then finds the release's notarization online)
mkdir -p "$FX/rb" && tar -xzf "$RC/sheepdog-macos-universal.tar.gz" -C "$FX/rb" || fail "cannot unpack the rc"
[ "$(/usr/bin/plutil -extract SheepdogControlBuild raw -o - "$FX/cb/Sheepdog.app/Contents/Info.plist" 2>/dev/null)" = true ] \
  && pass "the control carries the control marker" || fail "the control has no control marker (built from a tag before the marker: rc.1's is; use the next rc's control)"
cdh() { /usr/bin/codesign -d -vvv --arch "$2" "$1" 2>&1 | sed -n 's/^CDHash=//p'; }
d=0; for a in arm64 x86_64; do x=$(cdh "$FX/rb/Sheepdog.app" $a) y=$(cdh "$FX/cb/Sheepdog.app" $a)
  [ -n "$x" ] && [ -n "$y" ] && [ "$x" != "$y" ] && d=$((d + 1)); done
[ $d = 2 ] && pass "the control's CDHash differs from the rc's on both slices" || fail "the control's CDHash differs from the rc's on $d of 2 slices (both must be read and differ)"
# the spctl row counts only for a bundle that is there and meets the requirement (a missing one
# is rejected too)
if [ -d "$FX/cb/Sheepdog.app" ] && rt_meets "$FX/cb/Sheepdog.app"; then
  pass "the control's bundle meets the release requirement (Developer ID signed)"
  rt_spctl_ok "$FX/cb/Sheepdog.app" && fail "Gatekeeper accepts the control's bundle" || pass "Gatekeeper (spctl) rejects the control's bundle"
else fail "the control's bundle is missing or does not meet the requirement"; fi
# no staple ticket, and a DEVELOPER_DIR whose xcrun answers 0 to everything: still refused
cpy "$RC" ns; mkdir -p "$FX/nsb" && tar -xzf "$FX/ns/$tag/sheepdog-macos-universal.tar.gz" -C "$FX/nsb" && rm -f "$FX/nsb/Sheepdog.app/Contents/CodeResources" \
  && rt_meets "$FX/nsb/Sheepdog.app" && "$SD_ROOT/scripts/lib/archive.sh" make "$FX/nsb/Sheepdog.app" "$FX/ns/$tag/sheepdog-macos-universal.tar.gz" \
  || fail "the unstapled copy could not be made (or no longer meets the requirement)"
mkdir -p "$FX/dev/usr/bin"; printf '#!/bin/sh\nexit 0\n' > "$FX/dev/usr/bin/xcrun"; chmod 755 "$FX/dev/usr/bin/xcrun"
DEVELOPER_DIR="$FX/dev" /usr/bin/xcrun stapler validate "$FX/nsb/Sheepdog.app" >/dev/null 2>&1 || fail "the lying DEVELOPER_DIR does not lie (the row would prove nothing)"
(cd "$SD_ROOT" && env HOME="$FX/h" DEVELOPER_DIR="$FX/dev" sh scripts/release.sh verify --out "$FX/ns" "$tag" < /dev/null) > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q 'stapler' "$FX/o" && pass "verify: no staple ticket refused by stapler, a lying DEVELOPER_DIR ignored" || fail "verify, lying DEVELOPER_DIR: $r $(tail -1 "$FX/o")"

# npm-check (on a copy: a pass writes its stamp, and the rc output stays read only)
cpy "$RC" ok; rm -f "$FX/ok/$tag/NPM-CHECKED"
rel npm-check --out "$FX/ok" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 0 ] && pass "npm-check: the rc accepted" || fail "npm-check rc: $r $(tail -1 "$FX/o")"
[ "$(cat "$FX/ok/$tag/NPM-CHECKED" 2>/dev/null)" = "$tag $(shasum -a 256 "$FX/ok/$tag/MANIFEST.json" | cut -d' ' -f1)" ] \
  && pass "npm-check's pass wrote its stamp: the tag and the manifest's sha256" || fail "npm-check's stamp: '$(cat "$FX/ok/$tag/NPM-CHECKED" 2>/dev/null)'"
# (a) the release archive does not match its manifest entry
cpy "$RC" xa; mkdir -p "$FX/tb" && tar -xzf "$RC/sheepdog-macos-universal.tar.gz" -C "$FX/tb" || fail "cannot unpack the rc"
perl -pi -e 's/<string>APPL<\/string>/<string>APPl<\/string>/' "$FX/tb/Sheepdog.app/Contents/Info.plist"
grep -q APPl "$FX/tb/Sheepdog.app/Contents/Info.plist" || fail "the Info.plist byte was not changed"
"$SD_ROOT/scripts/lib/archive.sh" make "$FX/tb/Sheepdog.app" "$FX/xa/$tag/sheepdog-macos-universal.tar.gz" || fail "archive"
rel npm-check --out "$FX/xa" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q 'sheepdog-macos-universal.tar.gz does not match its manifest hash' "$FX/o" && pass "npm-check: a release archive that does not match the manifest refused by its hash" || fail "npm-check archive hash: $r $(tail -1 "$FX/o")"
# (b) the same tampered archive with the manifest updated: its bundle fails the requirement
setsum xa sheepdog-macos-universal.tar.gz
rel npm-check --out "$FX/xa" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q "Sheepdog.app/Contents/Info.plist differs in content" "$FX/o" && pass "npm-check: a tampered release archive (manifest updated) refused (its Info.plist differs)" || fail "npm-check tampered archive: $r $(tail -1 "$FX/o")"
# (c) the control's archive (Developer ID signed, meets the requirement) with the manifest updated:
#     not the npm package's bundle, byte for byte
cpy "$RC" xc; cp "$CT/sheepdog-macos-universal.tar.gz" "$FX/xc/$tag/sheepdog-macos-universal.tar.gz" && setsum xc sheepdog-macos-universal.tar.gz || fail "the control's archive could not be put in the copy"
rel npm-check --out "$FX/xc" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q "the files are not exactly the bundle's" "$FX/o" && ! grep "the files are not exactly" "$FX/o" | grep -q 'Sheepdog.app/Contents/CodeResources' \
  && pass "npm-check: the control's archive in the release's place refused (no staple ticket: the archive check)" || fail "npm-check control archive: $r $(tail -1 "$FX/o")"
# (d) the release archive holds only a symlink to a bundle elsewhere (manifest updated)
cpy "$RC" xs && python3 "$SD_ROOT/tests/lib/tgz-edit.py" "$RC/sheepdog-macos-universal.tar.gz" "$FX/xs/$tag/sheepdog-macos-universal.tar.gz" only-symlink Sheepdog.app ../p/package/Sheepdog.app \
  && setsum xs sheepdog-macos-universal.tar.gz || fail "the symlink archive could not be made"
rel npm-check --out "$FX/xs" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q 'neither files nor directories' "$FX/o" && pass "npm-check: a release archive holding a symlink refused by the archive check" || fail "npm-check symlink archive: $r $(tail -1 "$FX/o")"
# (e) the npm package's executable at 0644 (manifest updated): npm installs it so, and the launcher
#     cannot run it
DT=lukaso-sheepdog-darwin-universal-${tag#v}.tgz
cpy "$RC" xm && python3 "$SD_ROOT/tests/lib/tgz-edit.py" "$RC/$DT" "$FX/xm/$tag/$DT" mode package/Sheepdog.app/Contents/MacOS/sheepdog 644 \
  && setsum xm "$DT" || fail "the 0644 variant could not be made"
rel npm-check --out "$FX/xm" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q "Sheepdog.app/Contents/MacOS/sheepdog's mode is 644, not 755" "$FX/o" && pass "npm-check: the npm package's executable at 0644 refused (its exact mode)" || fail "npm-check 0644: $r $(tail -1 "$FX/o")"
# (g) a non-executable file's mode differs in the release archive only (Info.plist 0600): only the
#     comparison with the archive sees it
cpy "$RC" xg && python3 "$SD_ROOT/tests/lib/tgz-edit.py" "$RC/sheepdog-macos-universal.tar.gz" "$FX/xg/$tag/sheepdog-macos-universal.tar.gz" mode Sheepdog.app/Contents/Info.plist 600 \
  && setsum xg sheepdog-macos-universal.tar.gz || fail "the archive-0600 variant could not be made"
rel npm-check --out "$FX/xg" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q 'Info.plist has mode 644, the archive.s 600' "$FX/o" && pass "npm-check: a file whose mode differs in the release archive refused" || fail "npm-check archive 0600: $r $(tail -1 "$FX/o")"
# (h) the executable at 0644 in the release archive only
cpy "$RC" xh && python3 "$SD_ROOT/tests/lib/tgz-edit.py" "$RC/sheepdog-macos-universal.tar.gz" "$FX/xh/$tag/sheepdog-macos-universal.tar.gz" mode Sheepdog.app/Contents/MacOS/sheepdog 644 \
  && setsum xh sheepdog-macos-universal.tar.gz || fail "the archive-0644 variant could not be made"
rel npm-check --out "$FX/xh" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q 'Contents/MacOS/sheepdog has mode 755, the archive.s 644' "$FX/o" && pass "npm-check: the executable at 0644 in the release archive refused" || fail "npm-check archive 0644: $r $(tail -1 "$FX/o")"
# (j) the executable setuid (4755) in both tarballs (manifest updated): equal, and refused
cpy "$RC" xj && python3 "$SD_ROOT/tests/lib/tgz-edit.py" "$RC/$DT" "$FX/xj/$tag/$DT" mode package/Sheepdog.app/Contents/MacOS/sheepdog 4755 \
  && python3 "$SD_ROOT/tests/lib/tgz-edit.py" "$RC/sheepdog-macos-universal.tar.gz" "$FX/xj/$tag/sheepdog-macos-universal.tar.gz" mode Sheepdog.app/Contents/MacOS/sheepdog 4755 \
  && setsum xj "$DT" && setsum xj sheepdog-macos-universal.tar.gz || fail "the setuid variant could not be made"
rel npm-check --out "$FX/xj" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q "Contents/MacOS/sheepdog's mode 4755 has setuid" "$FX/o" && pass "npm-check: the executable setuid in both tarballs refused" || fail "npm-check setuid: $r $(tail -1 "$FX/o")"
# (k) the executable setuid in the release archive only (manifest updated): the archive side refuses
cpy "$RC" xk && python3 "$SD_ROOT/tests/lib/tgz-edit.py" "$RC/sheepdog-macos-universal.tar.gz" "$FX/xk/$tag/sheepdog-macos-universal.tar.gz" mode Sheepdog.app/Contents/MacOS/sheepdog 4755 \
  && setsum xk sheepdog-macos-universal.tar.gz || fail "the archive-setuid variant could not be made"
rel npm-check --out "$FX/xk" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q "the release archive: Sheepdog.app/Contents/MacOS/sheepdog's mode 4755 has setuid" "$FX/o" && pass "npm-check: the executable setuid in the release archive refused" || fail "npm-check archive setuid: $r $(tail -1 "$FX/o")"
# (l) a directory of the release archive setuid (Contents/MacOS/ at 4755; manifest updated)
cpy "$RC" xl && python3 "$SD_ROOT/tests/lib/tgz-edit.py" "$RC/sheepdog-macos-universal.tar.gz" "$FX/xl/$tag/sheepdog-macos-universal.tar.gz" mode Sheepdog.app/Contents/MacOS/ 4755 \
  && setsum xl sheepdog-macos-universal.tar.gz || fail "the directory-setuid variant could not be made"
rel npm-check --out "$FX/xl" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q "the release archive: Sheepdog.app/Contents/MacOS/'s mode 4755 has setuid" "$FX/o" && pass "npm-check: a setuid directory in the release archive refused" || fail "npm-check directory setuid: $r $(tail -1 "$FX/o")"
# (f) the same as the rc, as npm reads it, but with an AppleDouble entry (manifest updated)
cpy "$RC" xd && python3 "$SD_ROOT/tests/lib/tgz-edit.py" "$RC/$DT" "$FX/xd/$tag/$DT" add package/Sheepdog.app/Contents/MacOS/._sheepdog x \
  && setsum xd "$DT" || fail "the AppleDouble variant could not be made"
rel npm-check --out "$FX/xd" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q 'AppleDouble' "$FX/o" && pass "npm-check: an AppleDouble entry in the npm package refused" || fail "npm-check AppleDouble: $r $(tail -1 "$FX/o")"
# (i) an entry after one null block, in the package and in the release archive (manifest updated):
#     npm's tar reads on past a single null block, tarfile and macOS tar stop there
cpy "$RC" xn && python3 "$SD_ROOT/tests/lib/tgz-edit.py" "$RC/$DT" "$FX/xn/$tag/$DT" after-null package/Sheepdog.app/Contents/Resources/marker.txt x \
  && setsum xn "$DT" || fail "the null-block package could not be made"
rel npm-check --out "$FX/xn" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q 'after the end' "$FX/o" && pass "npm-check: an entry after one null block in the package refused" || fail "npm-check null block (package): $r $(tail -1 "$FX/o")"
cpy "$RC" xo && python3 "$SD_ROOT/tests/lib/tgz-edit.py" "$RC/sheepdog-macos-universal.tar.gz" "$FX/xo/$tag/sheepdog-macos-universal.tar.gz" after-null Sheepdog.app/Contents/Resources/marker.txt x \
  && setsum xo sheepdog-macos-universal.tar.gz || fail "the null-block archive could not be made"
rel npm-check --out "$FX/xo" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q 'after the end' "$FX/o" && pass "npm-check: an entry after one null block in the release archive refused" || fail "npm-check null block (archive): $r $(tail -1 "$FX/o")"
rel npm-check --out "$FX/c" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q "mode is 'control'" "$FX/o" && pass "npm-check: the control refused by its manifest's mode" || fail "npm-check control: $r $(tail -1 "$FX/o")"
sed -e 's/"mode": "control"/"mode": "signed"/' -e 's/"control": true/"control": false/' "$FX/c/$tag/MANIFEST.json" > "$FX/m" && mv "$FX/m" "$FX/c/$tag/MANIFEST.json"
grep -q '"control": false' "$FX/c/$tag/MANIFEST.json" && grep -q '"mode": "signed"' "$FX/c/$tag/MANIFEST.json" || fail "the control manifest was not cleared"
rel npm-check --out "$FX/c" "$tag" > "$FX/o" 2>&1; r=$?
if [ $marked = yes ]; then
  [ $r = 1 ] && grep -q 'the darwin package holds a control build' "$FX/o" && ! grep -q -e codesign -e stapler "$FX/o" && pass "npm-check: the (marked) control with its flag cleared refused by its marker" || fail "npm-check cleared control: $r $(tail -1 "$FX/o")"
else
  [ $r = 1 ] && grep -q 'stapler' "$FX/o" && pass "npm-check: the (unmarked) control with its flag cleared refused by stapler" || fail "npm-check cleared control: $r $(tail -1 "$FX/o")"
fi
# npm-check's stapler refusal on real Developer ID bytes, on any rc: the rc without its staple
# ticket in both tarballs (manifest updated)
cpy "$RC" xu && python3 "$SD_ROOT/tests/lib/tgz-edit.py" "$RC/$DT" "$FX/xu/$tag/$DT" remove package/Sheepdog.app/Contents/CodeResources - \
  && python3 "$SD_ROOT/tests/lib/tgz-edit.py" "$RC/sheepdog-macos-universal.tar.gz" "$FX/xu/$tag/sheepdog-macos-universal.tar.gz" remove Sheepdog.app/Contents/CodeResources - \
  && setsum xu "$DT" && setsum xu sheepdog-macos-universal.tar.gz || fail "the unstapled rc could not be made"
rel npm-check --out "$FX/xu" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q "stapler: the darwin package's bundle has no valid staple ticket" "$FX/o" && pass "npm-check: the rc without its staple ticket refused by stapler" || fail "npm-check unstapled rc: $r $(tail -1 "$FX/o")"

# the planner
t=$(cd "$SD_ROOT" && git rev-parse "$tag") && c=$(cd "$SD_ROOT" && git rev-parse "$tag^{commit}") || fail "no local tag $tag"
printf '%s\trefs/tags/%s\n%s\trefs/tags/%s^{}\n' "$t" "$tag" "$c" "$tag" > "$FX/remote"; : > "$FX/releases"
(cd "$SD_ROOT" && sh scripts/release-plan.sh --out "$RC" --tag "$tag" --remote "$FX/remote" --releases "$FX/releases") > "$FX/plan" 2> "$FX/o"; r=$?
[ $r = 0 ] && sh "$SD_ROOT/scripts/release-plan.sh" --validate "$FX/plan" && pass "the planner accepts the rc's manifest; the plan validates" || fail "planner: $r $(tail -1 "$FX/o")"

# the door
mkdir -p "$FX/d" && tar -xzf "$RC/sheepdog-macos-universal.tar.gz" -C "$FX/d" || fail "cannot unpack the rc"
G="$SD_ROOT/scripts/lib/exec-guard.sh"
sh "$G" check "$FX/d/Sheepdog.app/Contents/MacOS/sheepdog" 2> "$FX/o" && pass "the door allows the rc's executable" || fail "the door refuses the rc: $(head -c 200 "$FX/o")"
cp -R "$FX/d" "$FX/d1" && perl -pi -e 's/<string>APPL<\/string>/<string>APPl<\/string>/' "$FX/d1/Sheepdog.app/Contents/Info.plist"
grep -q APPl "$FX/d1/Sheepdog.app/Contents/Info.plist" || fail "the Info.plist byte was not changed"
sh "$G" check "$FX/d1/Sheepdog.app/Contents/MacOS/sheepdog" 2> "$FX/o" && fail "the door allows a changed Info.plist"
grep -q ', and it lacks the release signature (' "$FX/o" && ! grep -q 'enclosing bundle lacks' "$FX/o" && pass "the door refuses the rc with a changed Info.plist byte (no release signature)" || fail "changed Info.plist: $(head -c 200 "$FX/o")"
cp -R "$FX/d" "$FX/d2" && mkdir -p "$FX/d2/Sheepdog.app/Contents/Resources" && echo extra > "$FX/d2/Sheepdog.app/Contents/Resources/extra.txt"
[ -f "$FX/d2/Sheepdog.app/Contents/Resources/extra.txt" ] && [ -f "$FX/d2/Sheepdog.app/Contents/MacOS/sheepdog" ] || fail "the added-file copy was not made"
sh "$G" check "$FX/d2/Sheepdog.app/Contents/MacOS/sheepdog" 2> "$FX/o" && fail "the door allows an added file"
grep -q ', and it lacks the release signature (' "$FX/o" && ! grep -q 'enclosing bundle lacks' "$FX/o" && pass "the door refuses the rc with an added file (no release signature)" || fail "added file: $(head -c 200 "$FX/o")"
# the rc bundle, intact, inside an unsigned outer bundle
mkdir -p "$FX/d3/Out.app/Contents/MacOS" && cp -R "$FX/d/Sheepdog.app" "$FX/d3/Out.app/Contents/MacOS/" && fx_plist "$FX/d3/Out.app/Contents/Info.plist" com.example.outer
in3=$FX/d3/Out.app/Contents/MacOS/Sheepdog.app
rt_meets "$in3" && rt_meets "$in3/Contents/MacOS/sheepdog" || fail "the nested copy of the rc does not meet the requirement (the row would prove nothing)"
sh "$G" check "$in3/Contents/MacOS/sheepdog" 2> "$FX/o" && fail "the door allows the rc inside an unsigned outer bundle"
grep -q 'an enclosing bundle lacks the release signature' "$FX/o" && pass "the door refuses the rc inside an unsigned outer bundle (the enclosing-bundle check)" || fail "nested: $(head -c 200 "$FX/o")"
finish
