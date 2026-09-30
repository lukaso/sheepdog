#!/bin/sh
# Pack the four npm packages of a release (PHASE3.md S5, D9). Used by release.sh build and by the
# npm install cell, so the cell installs what the build makes.
#   npm-pack.sh VERSION ARCHIVE LINUX-AARCH64 LINUX-X86_64 LAUNCHER DEST
# ARCHIVE is the macOS release archive (its Sheepdog.app goes into the darwin package); LAUNCHER the
# sh launcher (npm/sheepdog/bin/sheepdog). The main package's optional dependencies are pinned to
# exactly VERSION. Each package carries the license (MIT OR Apache-2.0): the expression in
# package.json and both texts from the root of the tree this script is in. npm runs with a temp HOME, cache and userconfig (never the user's ~/.npmrc).
# Prints the four file names.
set -u
[ $# -eq 6 ] || { echo "usage: npm-pack.sh VERSION ARCHIVE LINUX-AARCH64 LINUX-X86_64 LAUNCHER DEST" >&2; exit 2; }
nv=$1 arc=$2 la=$3 lx=$4 launcher=$5 dest=$6
root=$(cd "$(dirname "$0")/../.." && pwd -P) || exit 1
pk=$(mktemp -d "${TMPDIR:-/tmp}/sd-npmpack.XXXXXX") || exit 1
trap 'rm -rf "$pk"' EXIT
mkdir -p "$pk/main/bin" "$pk/darwin" "$pk/linux-arm64/bin" "$pk/linux-x64/bin" "$pk/home" || exit 1
: > "$pk/npmrc"
cp "$launcher" "$pk/main/bin/sheepdog" && chmod 755 "$pk/main/bin/sheepdog" || exit 1
# each package.json from npm-same.py, which npm-check compares against (one source for both)
pj() { env -u DEVELOPER_DIR -u SDKROOT -u TOOLCHAINS /usr/bin/python3 -I "$root/scripts/lib/npm-same.py" pkgjson "$@"; }
pj main "$nv" > "$pk/main/package.json" || { echo "npm-pack: package.json (main)" >&2; exit 1; }
tar -xzf "$arc" -C "$pk/darwin" || exit 1
pj darwin "$nv" > "$pk/darwin/package.json" || { echo "npm-pack: package.json (darwin)" >&2; exit 1; }
cp "$la" "$pk/linux-arm64/bin/sheepdog" && cp "$lx" "$pk/linux-x64/bin/sheepdog" && chmod 755 "$pk/linux-arm64/bin/sheepdog" "$pk/linux-x64/bin/sheepdog" || exit 1
for a in arm64 x64; do
  pj "linux-$a" "$nv" > "$pk/linux-$a/package.json" || { echo "npm-pack: package.json (linux-$a)" >&2; exit 1; }
done
for d in main darwin linux-arm64 linux-x64; do
  cp "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$pk/$d/" || { echo "npm-pack: no license texts in $root" >&2; exit 1; }
  (cd "$pk/$d" && env HOME="$pk/home" npm_config_cache="$pk/cache" npm_config_userconfig="$pk/npmrc" npm pack --silent --pack-destination "$dest" >/dev/null) || { echo "npm-pack: npm pack $d failed" >&2; exit 1; }
done
for f in "lukaso-sheepdog-$nv.tgz" "lukaso-sheepdog-darwin-universal-$nv.tgz" "lukaso-sheepdog-linux-arm64-$nv.tgz" "lukaso-sheepdog-linux-x64-$nv.tgz"; do
  [ -s "$dest/$f" ] || { echo "npm-pack: no $f" >&2; exit 1; }
  echo "$f"
done
