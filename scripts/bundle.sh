#!/bin/sh
# Make Sheepdog.app around a sheepdog binary (PHASE3.md S0).
#
#   scripts/bundle.sh BINARY OUT-DIR VERSION BUILD-NUMBER [--release-id [--control]]
#
# VERSION is X.Y.Z and BUILD-NUMBER a positive integer (PHASE3.md D4: Apple's bundle versions are
# dotted integers, and plutil does not check them). The bundle ID is com.lukaso.sheepdog.dev
# unless --release-id is given (PHASE3.md D5): only the signing path passes it, and it signs the
# bundle in the same step, under /private/tmp. The bundle is not signed here. --control (with
# --release-id only) adds the key SheepdogControlBuild = true, so a control build's CDHash differs
# from the release's: the build is reproducible, and a control of the same commit would otherwise
# share the release's CDHashes and pass Gatekeeper on Apple's online ticket (measured with rc.1).
set -u
usage() { echo "usage: bundle.sh BINARY OUT-DIR VERSION BUILD-NUMBER [--release-id [--control]]" >&2; exit 2; }
[ $# -ge 4 ] && [ $# -le 6 ] || usage
bin=$1 out=$2 version=$3 build=$4
id=com.lukaso.sheepdog.dev ctl=""
if [ $# -ge 5 ]; then [ "$5" = --release-id ] || usage; id=com.lukaso.sheepdog; fi
if [ $# -eq 6 ]; then [ "$6" = --control ] || usage; ctl='	<key>SheepdogControlBuild</key>
	<true/>
'; fi
echo "$version" | grep -Eq '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' \
  || { echo "bundle.sh: version must be X.Y.Z (dotted integers): '$version'" >&2; exit 2; }
echo "$build" | grep -Eq '^[1-9][0-9]*$' \
  || { echo "bundle.sh: build number must be a positive integer: '$build'" >&2; exit 2; }
[ -f "$bin" ] || { echo "bundle.sh: no binary at $bin" >&2; exit 2; }

# an unsigned bundle with the release ID lives only under /private/tmp, which Launch Services does
# not index (PLAN.md §4.4): the signing path builds and signs it there (PHASE3.md §1.1)
if [ "$id" = com.lukaso.sheepdog ]; then
  mkdir -p "$out" || exit 1
  o=$(cd -P "$out" && pwd -P) || exit 1
  case $o/ in /private/tmp/*) ;; *) rmdir "$out" 2>/dev/null; echo "bundle.sh: --release-id only under /private/tmp, not $o" >&2; exit 2 ;; esac
fi
app=$out/Sheepdog.app
[ -e "$app" ] && { echo "bundle.sh: $app exists" >&2; exit 2; }
mkdir -p "$app/Contents/MacOS" || exit 1
cp "$bin" "$app/Contents/MacOS/sheepdog" && chmod 755 "$app/Contents/MacOS/sheepdog" || exit 1
cat > "$app/Contents/Info.plist" <<EOF || exit 1
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key>
	<string>$id</string>
	<key>CFBundleName</key>
	<string>Sheepdog</string>
	<key>CFBundleExecutable</key>
	<string>sheepdog</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleShortVersionString</key>
	<string>$version</string>
	<key>CFBundleVersion</key>
	<string>$build</string>
	<key>LSMinimumSystemVersion</key>
	<string>12.0</string>
	<key>LSUIElement</key>
	<true/>
${ctl}</dict>
</plist>
EOF
/usr/bin/plutil -lint "$app/Contents/Info.plist" >/dev/null || { rm -rf "$app"; exit 1; }
echo "$app"
