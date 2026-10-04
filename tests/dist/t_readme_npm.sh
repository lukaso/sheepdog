#!/bin/sh
# The README's npm and pnpm lines, run as a reader runs them, against fixtures. The pnpm store
# removal (a `find ... -exec rm -rf`) removes exactly the store folders of the two packages a Mac
# installs, the main one and the darwin one, and leaves a package whose name only starts with the
# main one's (sheeprl, sheeprx) and the Linux packages; the two "where is Sheepr.app" lines print
# exactly the app. The install and removal commands name the main package of release.conf's
# SR_NPM_PKGS (the last one). The lines run with a stand-in npm and pnpm whose `root -g` is a
# directory of this cell's own fixture dir, checked before anything runs.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
. "$SR_ROOT/scripts/release.conf" || exit 3
main=$(printf '%s\n' $SR_NPM_PKGS | tail -1)
RM=$SR_ROOT/README.md
line() { # PREFIX: the one README line that starts with it (exactly one, or the row fails)
  n=$(grep -c -F -- "$1" "$RM"); [ "$n" = 1 ] || { echo "README has $n lines with: $1" >&2; return 1; }
  grep -F -- "$1" "$RM"
}
for c in "npm i -g $main" "npm rm -g $main" "pnpm rm -g $main"; do
  grep -q -F -- "$c" "$RM" && pass "the README says: $c" || fail "the README does not say: $c"
done

# the stand-ins: `npm root -g` and `pnpm root -g` print directories of this fixture dir
mkdir -p "$FX/bin" "$FX/n/lib/node_modules" "$FX/p/global/5/node_modules"
printf '#!/bin/sh\n[ "$*" = "root -g" ] && echo "%s" && exit 0; exit 1\n' "$FX/n/lib/node_modules" > "$FX/bin/npm"
printf '#!/bin/sh\n[ "$*" = "root -g" ] && echo "%s" && exit 0; exit 1\n' "$FX/p/global/5/node_modules" > "$FX/bin/pnpm"
chmod 755 "$FX/bin/npm" "$FX/bin/pnpm"
P="PATH=$FX/bin:/usr/bin:/bin"
case $(env "$P" sh -c 'pnpm root -g') in "$FX"/p/*) ;; *) fail "the stand-in pnpm does not point inside the fixture: nothing is run"; finish ;; esac
case $(env "$P" sh -c 'npm root -g') in "$FX"/n/*) ;; *) fail "the stand-in npm does not point inside the fixture: nothing is run"; finish ;; esac

# the pnpm store removal
S=$FX/p/global/5/.pnpm
for d in "$main@0.1.0" "$main-darwin-universal@0.1.0" "${main}l@1.0.0" "${main}x@1.0.0" "$main-linux-x64@0.1.0"; do mkdir -p "$S/$d/node_modules"; done
l=$(line 'find "$(pnpm root -g)/../.pnpm" -maxdepth 1') || fail "no single pnpm store removal line"
[ -n "$l" ] && env "$P" sh -c "$l" > "$FX/o" 2>&1; r=$?
left=$(ls "$S" | LC_ALL=C sort | tr '\n' ' ')
[ $r = 0 ] && [ "$left" = "$main-linux-x64@0.1.0 ${main}l@1.0.0 ${main}x@1.0.0 " ] \
  && pass "the pnpm store removal removes the main and darwin packages only (left: $left)" || fail "the pnpm store removal: rc=$r, left: $left $(cat "$FX/o")"

# where is Sheepr.app: npm (nested under the main package) and pnpm (in the store)
mkdir -p "$FX/n/lib/node_modules/$main/node_modules/$main-darwin-universal/Sheepr.app/Contents" "$S/$main-darwin-universal@0.1.0/node_modules/$main-darwin-universal/Sheepr.app/Contents"
l=$(line '- npm: `find "$(npm root -g)/') && l=${l#- npm: \`} && l=${l%\`}
o=$(env "$P" sh -c "$l" 2>&1)
[ "$o" = "$FX/n/lib/node_modules/$main/node_modules/$main-darwin-universal/Sheepr.app" ] && pass "the npm line prints the app" || fail "the npm line ($l): $o"
l=$(line '- pnpm: `find "$(pnpm root -g)/') && l=${l#- pnpm: \`} && l=${l%\`}
o=$(env "$P" sh -c "$l" 2>&1)
[ "$o" = "$FX/p/global/5/node_modules/../.pnpm/$main-darwin-universal@0.1.0/node_modules/$main-darwin-universal/Sheepr.app" ] && pass "the pnpm line prints the app" || fail "the pnpm line ($l): $o"
finish
