#!/bin/sh
# Render the Homebrew cask for a release (PHASE3.md S4) from packaging/homebrew/sheepdog.rb.in:
#   render-cask.sh VERSION SHA256 OUT
# VERSION is X.Y.Z or X.Y.Z-rc.N; SHA256 the macOS archive's (64 lowercase hex digits).
set -u
[ $# -eq 3 ] || { echo "usage: render-cask.sh VERSION SHA256 OUT" >&2; exit 2; }
v=$1 h=$2 o=$3
lib=$(cd "$(dirname "$0")" && pwd -P) || exit 1
printf '%s\n' "$v" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-rc\.[1-9][0-9]*)?$' || { echo "render-cask: bad version $v" >&2; exit 2; }
printf '%s\n' "$h" | grep -Eq '^[0-9a-f]{64}$' || { echo "render-cask: bad sha256 $h" >&2; exit 2; }
sed -e "s/@VERSION@/$v/" -e "s/@SHA256@/$h/" "$lib/../../packaging/homebrew/sheepdog.rb.in" > "$o.tmp" && mv "$o.tmp" "$o"
