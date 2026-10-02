#!/usr/bin/env python3
# A static npm registry (PHASE3.md S5 cell 18, S7's clean-user leg): static-registry.py DIR PORT SKIP TGZ-DIR
# For each .tgz in TGZ-DIR: a packument at DIR/<package name> whose one version is the tarball's
# package.json, with dist.tarball http://127.0.0.1:PORT/t/<file> and the file's own sha1 and
# sha512, and the file copied to DIR/t/. SKIP non-empty leaves the darwin package out. Served by
# `python3 -m http.server PORT --bind 127.0.0.1` from DIR. Its cell: tests/dist/t_static_registry.sh.
import sys, os, json, hashlib, base64, tarfile, shutil
d, port, skip, tg = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
os.makedirs(os.path.join(d, "@lukaso"), exist_ok=True); os.makedirs(os.path.join(d, "t"), exist_ok=True)
for f in sorted(os.listdir(tg)):
    if skip and "darwin" in f: continue
    b = open(os.path.join(tg, f), "rb").read()
    pj = json.load(tarfile.open(os.path.join(tg, f)).extractfile("package/package.json"))
    shutil.copy(os.path.join(tg, f), os.path.join(d, "t", f))
    v = dict(pj); v["dist"] = {"tarball": "http://127.0.0.1:%s/t/%s" % (port, f),
        "shasum": hashlib.sha1(b).hexdigest(), "integrity": "sha512-" + base64.b64encode(hashlib.sha512(b).digest()).decode()}
    doc = {"name": pj["name"], "dist-tags": {"latest": pj["version"]}, "versions": {pj["version"]: v}}
    json.dump(doc, open(os.path.join(d, pj["name"]), "w"))
