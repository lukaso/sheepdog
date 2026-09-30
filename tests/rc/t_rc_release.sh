#!/bin/sh
# PHASE3.md §3 `rc` leg, the release checks, against the operator's rc output (SD_RC_DIR) and its
# control (SD_RC_CONTROL_DIR), read only:
#   - verify (the executor's real-tool step): the rc accepted; the control refused by stapler,
#     and Gatekeeper (spctl) rejects the control's bundle too; a copy of the rc without its staple
#     ticket refused by stapler even with a DEVELOPER_DIR whose xcrun says yes to everything;
#   - npm-check: the rc accepted; a copy whose release archive does not match the manifest
#     refused by its hash; a tampered archive (the manifest updated) refused by codesign; the
#     control's archive (Developer ID signed, the manifest updated) refused as not byte-identical
#     to the npm package's bundle; the control refused by its mode; a copy of the control with
#     the mode and control flag cleared refused by stapler;
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
rel verify --out "$FX/c" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q 'stapler' "$FX/o" && pass "verify: the control refused by stapler" || fail "verify control: $r $(tail -1 "$FX/o")"
mkdir -p "$FX/cb" && tar -xzf "$CT/sheepdog-macos-universal.tar.gz" -C "$FX/cb" || fail "cannot unpack the control"
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

# npm-check
rel npm-check --out "$(dirname "$RC")" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 0 ] && pass "npm-check: the rc accepted" || fail "npm-check rc: $r $(tail -1 "$FX/o")"
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
[ $r = 1 ] && grep -q "is not the release archive's bundle" "$FX/o" && pass "npm-check: another Developer ID bundle in the release archive refused (not the same files)" || fail "npm-check control archive: $r $(tail -1 "$FX/o")"
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
[ $r = 1 ] && grep -q 'Sheepdog.app/Contents/MacOS/sheepdog has mode 644, the archive.s 755' "$FX/o" && pass "npm-check: the npm package's executable at 0644 refused (its mode differs from the archive's)" || fail "npm-check 0644: $r $(tail -1 "$FX/o")"
# (g) a non-executable file's mode differs (Info.plist 0600 in the package): only the mode comparison
cpy "$RC" xg && python3 "$SD_ROOT/tests/lib/tgz-edit.py" "$RC/$DT" "$FX/xg/$tag/$DT" mode package/Sheepdog.app/Contents/Info.plist 600 \
  && setsum xg "$DT" || fail "the 0600 variant could not be made"
rel npm-check --out "$FX/xg" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q 'Info.plist has mode 600, the archive.s 644' "$FX/o" && pass "npm-check: a file whose mode differs from the archive's refused" || fail "npm-check 0600: $r $(tail -1 "$FX/o")"
# (h) the executable at 0644 in both tarballs (manifest updated): equal, and still refused
cpy "$RC" xh && python3 "$SD_ROOT/tests/lib/tgz-edit.py" "$RC/$DT" "$FX/xh/$tag/$DT" mode package/Sheepdog.app/Contents/MacOS/sheepdog 644 \
  && python3 "$SD_ROOT/tests/lib/tgz-edit.py" "$RC/sheepdog-macos-universal.tar.gz" "$FX/xh/$tag/sheepdog-macos-universal.tar.gz" mode Sheepdog.app/Contents/MacOS/sheepdog 644 \
  && setsum xh "$DT" && setsum xh sheepdog-macos-universal.tar.gz || fail "the both-0644 variant could not be made"
rel npm-check --out "$FX/xh" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q "the executable's mode is not 0755" "$FX/o" && pass "npm-check: the executable at 0644 in both tarballs refused (not 0755)" || fail "npm-check both 0644: $r $(tail -1 "$FX/o")"
# (f) the same as the rc, as npm reads it, but with an AppleDouble entry (manifest updated)
cpy "$RC" xd && python3 "$SD_ROOT/tests/lib/tgz-edit.py" "$RC/$DT" "$FX/xd/$tag/$DT" add package/Sheepdog.app/Contents/MacOS/._sheepdog x \
  && setsum xd "$DT" || fail "the AppleDouble variant could not be made"
rel npm-check --out "$FX/xd" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q 'AppleDouble' "$FX/o" && pass "npm-check: an AppleDouble entry in the npm package refused" || fail "npm-check AppleDouble: $r $(tail -1 "$FX/o")"
rel npm-check --out "$FX/c" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q "mode is 'control'" "$FX/o" && pass "npm-check: the control refused by its manifest's mode" || fail "npm-check control: $r $(tail -1 "$FX/o")"
sed -e 's/"mode": "control"/"mode": "signed"/' -e 's/"control": true/"control": false/' "$FX/c/$tag/MANIFEST.json" > "$FX/m" && mv "$FX/m" "$FX/c/$tag/MANIFEST.json"
grep -q '"control": false' "$FX/c/$tag/MANIFEST.json" && grep -q '"mode": "signed"' "$FX/c/$tag/MANIFEST.json" || fail "the control manifest was not cleared"
rel npm-check --out "$FX/c" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q 'stapler' "$FX/o" && pass "npm-check: the control with its flag cleared refused by stapler" || fail "npm-check cleared control: $r $(tail -1 "$FX/o")"

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
