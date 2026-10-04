#!/bin/sh
# PHASE3.md S0: scripts/bundle.sh makes Sheepr.app. The ID is .dev unless --release-id (then
# the bundle is built only in the fixture dir and never run); the versions are dotted integers
# (D4); the Info.plist keys of S0; a universal input stays universal; the binary is copied as is.
set -u
. "$(dirname "$0")/lib.sh"
[ "$(uname -s)" = Darwin ] || { echo "SKIP (macOS only)"; exit 0; }
fx_dir
B="$SR_ROOT/scripts/bundle.sh"
printf 'int main(){return 0;}\n' > "$FX/m.c"
cc -arch arm64 -arch x86_64 -o "$FX/bin" "$FX/m.c" || exit 3
key() { /usr/bin/plutil -extract "$2" raw -o - "$1/Contents/Info.plist" 2>/dev/null; }

"$B" "$FX/bin" "$FX/dev" 0.1.0 7 >/dev/null 2>&1 || fail "bundle.sh failed"
A="$FX/dev/Sheepr.app"
/usr/bin/plutil -lint "$A/Contents/Info.plist" >/dev/null 2>&1 && pass "plist lints" || fail "plist does not lint"
[ "$(key "$A" CFBundleIdentifier)" = com.lukaso.sheepr.dev ] && pass ".dev ID without the flag" || fail "ID is $(key "$A" CFBundleIdentifier)"
for kv in CFBundleName=Sheepr CFBundleExecutable=sheepr CFBundlePackageType=APPL CFBundleInfoDictionaryVersion=6.0 \
          LSMinimumSystemVersion=12.0 LSUIElement=true CFBundleShortVersionString=0.1.0 CFBundleVersion=7; do
  k=${kv%%=*} v=${kv#*=}
  [ "$(key "$A" "$k")" = "$v" ] && pass "$k=$v" || fail "$k is '$(key "$A" "$k")', want '$v'"
done
cmp -s "$FX/bin" "$A/Contents/MacOS/sheepr" && pass "binary copied as is" || fail "binary differs or missing"
case $(lipo -archs "$A/Contents/MacOS/sheepr" 2>/dev/null) in *x86_64*arm64*|*arm64*x86_64*) pass "universal kept" ;; *) fail "not universal" ;; esac

"$B" "$FX/bin" "$FX/rel" 0.1.0 7 --release-id >/dev/null 2>&1 || fail "bundle.sh --release-id failed"
[ "$(key "$FX/rel/Sheepr.app" CFBundleIdentifier)" = com.lukaso.sheepr ] && pass "release ID with the flag" || fail "no release ID with the flag"
[ -z "$(key "$FX/rel/Sheepr.app" SheeprControlBuild)" ] && pass "no control marker without --control" || fail "a control marker without --control"
# the control build's marker (PHASE3.md S2, the control mode): its Info.plist, and so its CDHash, differ
"$B" "$FX/bin" "$FX/ctl" 0.1.0 7 --release-id --control >/dev/null 2>&1 || fail "bundle.sh --release-id --control failed"
[ "$(key "$FX/ctl/Sheepr.app" SheeprControlBuild)" = true ] && pass "the control marker with --control" || fail "no control marker with --control: '$(key "$FX/ctl/Sheepr.app" SheeprControlBuild)'"
# the mechanism: the same binary, ad-hoc signed (no identity, nothing run), gets another CDHash
cdh() { /usr/bin/codesign -d -vvv --arch "$2" "$1" 2>&1 | sed -n 's/^CDHash=//p'; }
codesign -s - -f "$FX/rel/Sheepr.app" 2>/dev/null && codesign -s - -f "$FX/ctl/Sheepr.app" 2>/dev/null || fail "ad-hoc signing"
d=0; for a in arm64 x86_64; do x=$(cdh "$FX/rel/Sheepr.app" $a) y=$(cdh "$FX/ctl/Sheepr.app" $a)
  [ -n "$x" ] && [ -n "$y" ] && [ "$x" != "$y" ] && d=$((d + 1)); done
[ $d = 2 ] && pass "the control marker changes the CDHash of both slices" || fail "the CDHash differs on $d of 2 slices"
"$B" "$FX/bin" "$FX/ctl2" 0.1.0 7 --control >/dev/null 2>&1 && fail "--control without --release-id accepted" || pass "--control without --release-id refused"
[ ! -e "$FX/ctl2/Sheepr.app" ] || fail "--control without --release-id made a bundle"

# an unsigned release-ID bundle is made only under /private/tmp (Launch Services does not index it)
out=$(mktemp -d /private/var/tmp/sr-p3-refuse.XXXXXX) || exit 3
if "$B" "$FX/bin" "$out/x" 0.1.0 7 --release-id >/dev/null 2>&1 || [ -e "$out/x/Sheepr.app" ]; then fail "--release-id outside /private/tmp accepted"
else pass "--release-id outside /private/tmp refused, nothing made"; fi
rm -rf "$out"
"$B" "$FX/bin" "$out/y" 0.1.0 7 >/dev/null 2>&1 && pass "control: a .dev bundle outside /private/tmp is fine" || fail "a .dev bundle outside /private/tmp refused"
rm -rf "$out"
for bad in 0.1.0-rc.1 v0.1.0 1.2.3.4 ""; do
  rm -rf "$FX/bad"
  if "$B" "$FX/bin" "$FX/bad" "$bad" 7 >/dev/null 2>&1 || [ -e "$FX/bad/Sheepr.app" ]; then fail "version '$bad' accepted"; else pass "version '$bad' refused"; fi
done
for bad in 0 07 x 1.0; do
  rm -rf "$FX/bad"
  if "$B" "$FX/bin" "$FX/bad" 0.1.0 "$bad" >/dev/null 2>&1 || [ -e "$FX/bad/Sheepr.app" ]; then fail "build number '$bad' accepted"; else pass "build number '$bad' refused"; fi
done
# the door lets the .dev bundle run (recorded, not run)
rec="$FX/rec"; : > "$rec"
SR_EXEC_RECORD="$rec" "$SR_ROOT/scripts/lib/exec-guard.sh" exec "$A/Contents/MacOS/sheepr" 2>/dev/null
[ "$(grep -c . "$rec")" = 1 ] && pass "the door allows the .dev bundle" || fail "the door refused the .dev bundle"
finish
