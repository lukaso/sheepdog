#!/bin/sh
# The license (MIT OR Apache-2.0): both texts at the repo root, the SPDX expression in Cargo.toml,
# and in every npm package (packed by scripts/lib/npm-pack.sh, as release.sh build runs it) the
# same expression in package.json and both texts, byte-equal to the root's. Nothing is executed.
. "$(dirname "$0")/lib.sh"
fx_dir
L="MIT OR Apache-2.0"
for f in LICENSE-MIT LICENSE-APACHE; do
  [ -s "$SR_ROOT/$f" ] && pass "$f at the root" || fail "no $f at the root"
done
grep -q 'Apache License' "$SR_ROOT/LICENSE-APACHE" 2>/dev/null && grep -q 'Version 2.0' "$SR_ROOT/LICENSE-APACHE" \
  && pass "LICENSE-APACHE is the Apache 2.0 text" || fail "LICENSE-APACHE is not the Apache 2.0 text"
grep -q 'Permission is hereby granted, free of charge' "$SR_ROOT/LICENSE-MIT" 2>/dev/null \
  && pass "LICENSE-MIT is the MIT text" || fail "LICENSE-MIT is not the MIT text"
cl=$(cd "$SR_ROOT" && cargo metadata --no-deps --offline --format-version 1 2>/dev/null \
  | python3 -c 'import sys,json; print(json.load(sys.stdin)["packages"][0].get("license") or "")')
[ "$cl" = "$L" ] && pass "Cargo.toml: license = $L" || fail "Cargo.toml license is '$cl'"

# the npm packages, from a stand-in bundle with a non-release ID (the archive is only unpacked)
fx_bundle "$FX" com.example.sdlicense
mkdir -p "$FX/tgz" "$FX/lin"
"$SR_ROOT/scripts/lib/archive.sh" make "$FX/Sheepr.app" "$FX/lin/mac.tar.gz" || { fail "archive"; finish; }
printf '#!/bin/sh\n' > "$FX/lin/a"; printf '#!/bin/sh\n' > "$FX/lin/x"
sh "$SR_ROOT/scripts/lib/npm-pack.sh" 0.1.0 "$FX/lin/mac.tar.gz" "$FX/lin/a" "$FX/lin/x" "$SR_ROOT/npm/sheepr/bin/sheepr" "$FX/tgz" > "$FX/names" \
  || { fail "npm-pack.sh"; finish; }
[ "$(wc -l < "$FX/names" | tr -d ' ')" = 4 ] || fail "npm-pack.sh named $(wc -l < "$FX/names" | tr -d ' ') packages, not 4"
while read -r t; do
  python3 - "$FX/tgz/$t" "$SR_ROOT" "$L" <<'PY' && pass "$t: license $L and both texts" || fail "$t: see above"
import sys, json, tarfile
tg, root, want = sys.argv[1:4]
tf = tarfile.open(tg); names = tf.getnames(); ok = True
lic = json.load(tf.extractfile("package/package.json")).get("license")
if lic != want: print("  package.json license is %r" % lic); ok = False
for f in ("LICENSE-MIT", "LICENSE-APACHE"):
    if "package/" + f not in names: print("  no %s in the package" % f); ok = False
    elif tf.extractfile("package/" + f).read() != open(root + "/" + f, "rb").read(): print("  %s differs from the root's" % f); ok = False
sys.exit(0 if ok else 1)
PY
done < "$FX/names"
finish
