#!/bin/sh
# PHASE3.md §1.5: scripts/lib/archive.sh makes the macOS release archive (no xattrs, no ACLs, no
# mac metadata, owners 0/0 with no names) and checks one: exactly the bundle's files (unsigned:
# Info.plist and the executable; --signed: also the signature and the staple ticket), directory
# entries only the parents of those, no xattr after extraction, no xattr pax header, owners 0/0.
set -u
. "$(dirname "$0")/lib.sh"
[ "$(uname -s)" = Darwin ] || { echo "SKIP (macOS only)"; exit 0; }
fx_dir
A="$SD_ROOT/scripts/lib/archive.sh"
printf 'int main(){return 0;}\n' > "$FX/m.c"; cc -o "$FX/bin" "$FX/m.c" || exit 3
app=$("$SD_ROOT/scripts/bundle.sh" "$FX/bin" "$FX/b" 0.1.0 1) || exit 3
xattr -w com.apple.quarantine "0081;00000000;Safari;" "$app/Contents/Info.plist"
"$A" make "$app" "$FX/good.tar.gz" && pass "make" || fail "make failed"
"$A" check "$FX/good.tar.gz" && pass "a made archive passes (the planted xattr is not in it)" || fail "a made archive fails the check"
"$A" check "$FX/good.tar.gz" --signed >/dev/null 2>&1 && fail "--signed accepted an unsigned bundle" || pass "--signed refuses an unsigned bundle"
# the default tar keeps xattrs and owner names: refused
(cd "$FX/b" && tar -czf "$FX/plain.tar.gz" Sheepdog.app) 2>/dev/null
"$A" check "$FX/plain.tar.gz" >/dev/null 2>&1 && fail "a plain tar (xattr, names) accepted" || pass "a plain tar with an xattr and owner names refused"
# an xattr only (owners already 0/0 without names), and only com.apple.provenance, which macOS
# puts on every file made here and extraction adds anyway: only the pax-header check can see it
xattr -d com.apple.quarantine "$app/Contents/Info.plist" 2>/dev/null
xattr -l "$app/Contents/Info.plist" | grep -q com.apple.provenance || fail "the fixture has no provenance xattr"
(cd "$FX/b" && tar --uid 0 --gid 0 --uname '' --gname '' -czf "$FX/xattr.tar.gz" Sheepdog.app) 2>/dev/null
"$A" check "$FX/xattr.tar.gz" >/dev/null 2>&1 && fail "an archive with an xattr accepted" || pass "an archive with an xattr (and 0/0 owners) refused"
# an extra file: refused
echo x > "$app/Contents/extra"
"$A" make "$app" "$FX/extra.tar.gz" >/dev/null 2>&1
"$A" check "$FX/extra.tar.gz" >/dev/null 2>&1 && fail "an extra file accepted" || pass "an extra file refused"
rm -f "$app/Contents/extra"
# owner names only (no xattr): refused
xattr -c "$app/Contents/Info.plist"
(cd "$FX/b" && tar --no-xattrs --no-mac-metadata --no-acls -czf "$FX/names.tar.gz" Sheepdog.app) 2>/dev/null
"$A" check "$FX/names.tar.gz" >/dev/null 2>&1 && fail "owner names accepted" || pass "owner names refused"
finish
