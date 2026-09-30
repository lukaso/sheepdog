# The npm darwin package against the release archive, read raw, entry by entry (PHASE3.md §5 step
# 5). macOS tar folds AppleDouble (._) entries into xattrs and hides them from -t; npm drops the
# first path part and skips links; so neither tar's listing nor an extraction shows what npm
# installs. Run by release.sh npm-check as `python3 -I` with no DEVELOPER_DIR.
#   npm-same.py shape TGZ            the package holds only regular files, all under package/: no
#                                    link, directory entry, ._ name, xattr header, duplicate or odd
#                                    path; outside package/Sheepdog.app/ exactly package.json and
#                                    the two license texts
#   npm-same.py same TGZ ARCHIVE     the archive holds only regular files and directories under
#                                    Sheepdog.app (the same raw rules), and every file of the
#                                    package's Sheepdog.app equals the archive's: path, mode and
#                                    content; the executable is 0755
# Exit 0, or 1 with the reason on stderr.
import sys, tarfile, hashlib

def die(msg):
    sys.stderr.write("npm-same: " + msg + "\n"); sys.exit(1)

def raw(path, what, allow_dirs, prefix):
    files = {}; seen = set()
    try:
        t = tarfile.open(path, "r:gz")
        members = t.getmembers()
    except Exception as e:
        die("%s: cannot read %s (%s)" % (what, path, e))
    for m in members:
        n = m.name
        if n in seen: die("%s: %s appears twice" % (what, n))
        seen.add(n)
        parts = n.rstrip("/").split("/")
        if n.startswith("/") or any(p in ("", ".", "..") for p in parts): die("%s: an odd path: %r" % (what, n))
        if any(p.startswith("._") for p in parts): die("%s: an AppleDouble entry: %s" % (what, n))
        if any(("xattr" in k.lower()) or k.startswith(("SCHILY.", "LIBARCHIVE.")) for k in m.pax_headers):
            die("%s: an xattr header on %s" % (what, n))
        if parts[0] != prefix: die("%s: an entry outside %s/: %s" % (what, prefix, n))
        if m.isdir() and allow_dirs: continue
        if not m.isreg(): die("%s: %s is not a regular file" % (what, n))
        files[n] = (m.mode & 0o7777, hashlib.sha256(t.extractfile(m).read()).hexdigest())
    return files

def shape(tgz):
    f = raw(tgz, "the darwin package", False, "package")
    rest = sorted(n for n in f if not n.startswith("package/Sheepdog.app/"))
    want = ["package/LICENSE-APACHE", "package/LICENSE-MIT", "package/package.json"]
    if rest != want: die("the darwin package: outside package/Sheepdog.app/ it holds %s, not %s" % (rest, want))
    return {n[len("package/"):]: v for n, v in f.items() if n.startswith("package/Sheepdog.app/")}

if len(sys.argv) == 3 and sys.argv[1] == "shape":
    shape(sys.argv[2])
elif len(sys.argv) == 4 and sys.argv[1] == "same":
    p = shape(sys.argv[2])
    a = raw(sys.argv[3], "the release archive", True, "Sheepdog.app")
    exe = "Sheepdog.app/Contents/MacOS/sheepdog"
    if sorted(p) != sorted(a):
        die("the darwin package's bundle is not the release archive's bundle: files %s differ" % sorted(set(p) ^ set(a)))
    for n in sorted(p):
        if p[n][0] != a[n][0]: die("the darwin package's bundle is not the release archive's bundle: %s has mode %o, the archive's %o" % (n, p[n][0], a[n][0]))
        if p[n][1] != a[n][1]: die("the darwin package's bundle is not the release archive's bundle: %s differs in content" % n)
    if exe not in p or p[exe][0] & 0o777 != 0o755: die("the executable's mode is not 0755")
else:
    sys.stderr.write("usage: npm-same.py shape TGZ | same TGZ ARCHIVE\n"); sys.exit(2)
