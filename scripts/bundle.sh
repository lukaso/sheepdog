#!/bin/sh
# Make Sheepdog.app around a sheepdog binary (PHASE3.md S0).
#
#   scripts/bundle.sh BINARY OUT-DIR VERSION BUILD-NUMBER [--release-id]
#
# VERSION is X.Y.Z and BUILD-NUMBER a positive integer (PHASE3.md D4: Apple's bundle versions are
# dotted integers, and plutil does not check them). The bundle ID is com.lukaso.sheepdog.dev
# unless --release-id is given (PHASE3.md D5): only the signing path passes it, and it signs the
# bundle in the same step. The bundle is not signed here.
set -u
usage() { echo "usage: bundle.sh BINARY OUT-DIR VERSION BUILD-NUMBER [--release-id]" >&2; exit 2; }
[ $# -eq 4 ] || [ $# -eq 5 ] || usage
bin=$1 out=$2 version=$3 build=$4
id=com.lukaso.sheepdog.dev
if [ $# -eq 5 ]; then [ "$5" = --release-id ] || usage; id=com.lukaso.sheepdog; fi
echo "$version" | grep -Eq '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' \
  || { echo "bundle.sh: version must be X.Y.Z (dotted integers): '$version'" >&2; exit 2; }
echo "$build" | grep -Eq '^[1-9][0-9]*$' \
  || { echo "bundle.sh: build number must be a positive integer: '$build'" >&2; exit 2; }
[ -f "$bin" ] || { echo "bundle.sh: no binary at $bin" >&2; exit 2; }

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
</dict>
</plist>
EOF
/usr/bin/plutil -lint "$app/Contents/Info.plist" >/dev/null || { rm -rf "$app"; exit 1; }
echo "$app"
