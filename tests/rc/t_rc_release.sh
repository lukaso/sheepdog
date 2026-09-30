#!/bin/sh
# PHASE3.md §3 `rc` leg, the release checks, against the operator's rc output (SD_RC_DIR) and its
# control (SD_RC_CONTROL_DIR), read only:
#   - verify (the executor's real-tool step): the rc accepted; the control refused by stapler,
#     and Gatekeeper (spctl) rejects the control's bundle too;
#   - npm-check: the rc accepted; a copy whose release archive holds another bundle refused by
#     the cdhash comparison, both bundles' hashes named; the control refused by its manifest; a
#     copy of the control with the manifest's mode and control flag cleared refused by stapler;
#   - the planner accepts the rc's manifest (with this checkout's tag as the remote tag) and its
#     plan passes --validate;
#   - the door: the rc's executable allowed; a copy with a changed Info.plist byte, and a copy
#     with an added file, refused. (No fixture separates the door's bundle check from the
#     executable's: codesign -v on a bundle's main executable validates the whole bundle.)
set -u
. "$(dirname "$0")/../dist/lib.sh"
[ "$(uname -s)" = Darwin ] || { echo "FAIL: the rc leg needs macOS"; exit 1; }
fx_dir
RC=$SD_RC_DIR CT=$SD_RC_CONTROL_DIR
tag=$(basename "$RC")
[ "$(basename "$CT")" = "$tag-control" ] || fail "the control dir is not $tag-control: $CT"
rel() { (cd "$SD_ROOT" && env HOME="$FX/h" sh scripts/release.sh "$@" < /dev/null); }
mkdir -p "$FX/h"
. "$SD_ROOT/scripts/release.conf"; . "$SD_ROOT/scripts/lib/realtools.sh"

# verify
rel verify --out "$(dirname "$RC")" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 0 ] && pass "verify: the rc is signed, notarized and stapled" || fail "verify rc: $r $(tail -1 "$FX/o")"
mkdir -p "$FX/c/$tag"; cp -R "$CT/." "$FX/c/$tag/"
rel verify --out "$FX/c" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q 'stapler' "$FX/o" && pass "verify: the control refused by stapler" || fail "verify control: $r $(tail -1 "$FX/o")"
mkdir -p "$FX/cb" && tar -xzf "$CT/sheepdog-macos-universal.tar.gz" -C "$FX/cb" || fail "cannot unpack the control"
# the spctl row counts only for a bundle that is there and meets the requirement (a missing one
# is rejected too)
if [ -d "$FX/cb/Sheepdog.app" ] && rt_meets "$FX/cb/Sheepdog.app"; then
  pass "the control's bundle meets the release requirement (Developer ID signed)"
  rt_spctl_ok "$FX/cb/Sheepdog.app" && fail "Gatekeeper accepts the control's bundle" || pass "Gatekeeper (spctl) rejects the control's bundle"
else fail "the control's bundle is missing or does not meet the requirement"; fi

# npm-check
rel npm-check --out "$(dirname "$RC")" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 0 ] && pass "npm-check: the rc accepted" || fail "npm-check rc: $r $(tail -1 "$FX/o")"
mkdir -p "$FX/x/$tag"; cp -R "$RC/." "$FX/x/$tag/"
# another universal bundle, ad-hoc signed (a non-release ID; never run)
mkdir -p "$FX/other/Sheepdog.app/Contents/MacOS"; fx_plist "$FX/other/Sheepdog.app/Contents/Info.plist" com.example.sdother
printf 'int main(){return 0;}\n' > "$FX/o.c"; cc -arch arm64 -arch x86_64 -o "$FX/other/Sheepdog.app/Contents/MacOS/sheepdog" "$FX/o.c" || fail "cc"
codesign -s - -f "$FX/other/Sheepdog.app" 2>/dev/null || fail "ad-hoc sign"
"$SD_ROOT/scripts/lib/archive.sh" make "$FX/other/Sheepdog.app" "$FX/x/$tag/sheepdog-macos-universal.tar.gz" || fail "archive"
rel npm-check --out "$FX/x" "$tag" > "$FX/o" 2>&1; r=$?
n=$(grep -o '[0-9a-f]\{40\}' "$FX/o" | sort -u | grep -c .)
[ $r = 1 ] && grep -q 'is not the release archive' "$FX/o" && [ "$n" -ge 2 ] \
  && pass "npm-check: another bundle in the release archive refused by the cdhash, both hashes named" || fail "npm-check other bundle: $r ($n hashes) $(tail -1 "$FX/o")"
rel npm-check --out "$FX/c" "$tag" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q -e 'control' -e 'mode' "$FX/o" && pass "npm-check: the control refused by its manifest" || fail "npm-check control: $r $(tail -1 "$FX/o")"
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
sh "$G" check "$FX/d1/Sheepdog.app/Contents/MacOS/sheepdog" 2>/dev/null && fail "the door allows a changed Info.plist" || pass "the door refuses the rc with a changed Info.plist byte"
cp -R "$FX/d" "$FX/d2" && mkdir -p "$FX/d2/Sheepdog.app/Contents/Resources" && echo extra > "$FX/d2/Sheepdog.app/Contents/Resources/extra.txt"
sh "$G" check "$FX/d2/Sheepdog.app/Contents/MacOS/sheepdog" 2>/dev/null && fail "the door allows an added file" || pass "the door refuses the rc with an added file"
finish
