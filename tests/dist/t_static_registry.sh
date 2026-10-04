#!/bin/sh
# scripts/lib/static-registry.py, the npm registry of cell 18 (t_npm_install.sh) and of the S7
# clean-user leg: for each .tgz in a dir, a packument at its package name whose one version is the
# tarball's package.json, its dist.tarball the file on 127.0.0.1:PORT under t/, its shasum and
# integrity the file's own sha1 and sha512, and the file copied there; with SKIP, the darwin
# package is left out (cell 18's control). Packages from npm-pack.sh around a stand-in bundle with
# a non-release ID; nothing is executed.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
fx_bundle "$FX" com.example.sdregistry
mkdir -p "$FX/tgz" "$FX/lin"
"$SR_ROOT/scripts/lib/archive.sh" make "$FX/Sheepr.app" "$FX/lin/mac.tar.gz" || { fail "archive"; finish; }
printf '#!/bin/sh\n' > "$FX/lin/a"; printf '#!/bin/sh\n' > "$FX/lin/x"
sh "$SR_ROOT/scripts/lib/npm-pack.sh" 0.1.0-rc.9 "$FX/lin/mac.tar.gz" "$FX/lin/a" "$FX/lin/x" "$SR_ROOT/npm/sheepr/bin/sheepr" "$FX/tgz" > /dev/null \
  || { fail "npm-pack.sh"; finish; }
check() { # registry-dir port skip expected-count
  python3 - "$1" "$2" "$3" "$4" "$FX/tgz" <<'PY'
import sys, os, json, hashlib, base64, tarfile
d, port, skip, want, tg = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), sys.argv[5]
ok = True; n = 0
for f in sorted(os.listdir(tg)):
    pj = json.load(tarfile.open(os.path.join(tg, f)).extractfile("package/package.json"))
    doc_p = os.path.join(d, pj["name"])
    if skip and "darwin" in f:
        if os.path.exists(doc_p) or os.path.exists(os.path.join(d, "t", f)): print("  %s served though skipped" % f); ok = False
        continue
    n += 1
    if not os.path.isfile(doc_p): print("  no packument for %s" % pj["name"]); ok = False; continue
    doc = json.load(open(doc_p)); b = open(os.path.join(tg, f), "rb").read()
    v = doc.get("versions", {}).get(pj["version"])
    if doc.get("name") != pj["name"] or doc.get("dist-tags") != {"latest": pj["version"]} or list(doc.get("versions", {})) != [pj["version"]] or v is None:
        print("  %s: name, dist-tags or versions wrong" % f); ok = False; continue
    dist = v.get("dist", {}); rest = {k: x for k, x in v.items() if k != "dist"}
    if rest != pj: print("  %s: the version is not the tarball's package.json" % f); ok = False
    if dist != {"tarball": "http://127.0.0.1:%s/t/%s" % (port, f), "shasum": hashlib.sha1(b).hexdigest(),
                "integrity": "sha512-" + base64.b64encode(hashlib.sha512(b).digest()).decode()}: print("  %s: dist wrong: %s" % (f, dist)); ok = False
    if open(os.path.join(d, "t", f), "rb").read() != b: print("  %s: the served copy differs" % f); ok = False
if n != want: print("  %d packages, not %d" % (n, want)); ok = False
sys.exit(0 if ok else 1)
PY
}
python3 "$SR_ROOT/scripts/lib/static-registry.py" "$FX/r1" 41234 "" "$FX/tgz" && check "$FX/r1" 41234 "" 4 \
  && pass "four packuments: each the tarball's package.json, its dist the served file's URL, sha1 and sha512" || fail "the registry (see above)"
python3 "$SR_ROOT/scripts/lib/static-registry.py" "$FX/r2" 41235 skip "$FX/tgz" && check "$FX/r2" 41235 skip 3 \
  && pass "with skip: the darwin package left out, the other three served" || fail "the registry with skip (see above)"
# control: the check fails on a registry whose integrity is not the file's
python3 "$SR_ROOT/scripts/lib/static-registry.py" "$FX/r3" 41236 "" "$FX/tgz" \
  && python3 -c 'import json,sys; p=sys.argv[1]; d=json.load(open(p)); v=list(d["versions"].values())[0]; v["dist"]["integrity"]="sha512-AAAA"; json.dump(d,open(p,"w"))' "$FX/r3/sheepr" \
  && { check "$FX/r3" 41236 "" 4 >/dev/null && fail "control: a wrong integrity passed the check" || pass "control: a wrong integrity fails the check"; }
finish
