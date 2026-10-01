#!/usr/bin/env python3
# Test helper: tree-same.py A B -> exit 0 if the two directory trees are the same: the root's own
# mode, and every entry's path, type, mode (all 12 bits), a file's content (sha256) and a link's
# target; lstat throughout (a link is compared as a link, never followed). Otherwise exit 1, naming
# up to 5 differences. Its cell: tests/dist/t_tree_same.sh.
import os, sys, stat, hashlib
def tree(r):
    st = os.lstat(r); out = {".": (stat.S_IFMT(st.st_mode), st.st_mode & 0o7777)}
    for d, ds, fs in os.walk(r):
        for n in ds + fs:
            p = os.path.join(d, n); st = os.lstat(p); k = os.path.relpath(p, r)
            v = (stat.S_IFMT(st.st_mode), st.st_mode & 0o7777)
            if stat.S_ISREG(st.st_mode): v += (hashlib.sha256(open(p, "rb").read()).hexdigest(),)
            elif stat.S_ISLNK(st.st_mode): v += (os.readlink(p),)
            out[k] = v
    return out
a, b = sys.argv[1], sys.argv[2]
for x in (a, b):
    if os.path.islink(x) or not os.path.isdir(x): print("  not a directory: %s" % x); sys.exit(1)
ta, tb = tree(a), tree(b)
bad = sorted(k for k in set(ta) | set(tb) if ta.get(k) != tb.get(k))
if bad: print("  differs:", bad[:5]); sys.exit(1)
