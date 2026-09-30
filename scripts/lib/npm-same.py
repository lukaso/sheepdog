# The npm packages against the release (PHASE3.md §5 step 5), each tarball read header by header
# by this parser alone. Readers disagree on archives (macOS tar folds AppleDouble entries into
# xattrs; tarfile and macOS tar end at the first null block, npm's tar at the second; pax records,
# GNU headers, the ustar prefix and sparse entries are read differently), so only the one shape
# on which they all agree is accepted: plain ustar headers (magic "ustar\0" "00", a valid
# checksum, an empty prefix field, no link name), type 0 (and 5 in the release archive), names
# from [A-Za-z0-9._+-] and "/", unique without case (APFS), nothing but zero bytes after the
# first null block. The mode, size and checksum fields must be spaces, octal digits and a NUL or
# space ending inside the field (uid, gid and mtime are not read: npm pack leaves uid and gid all
# NUL, and none of the three changes a path, a size, a mode or a file's content), and every byte of the name after its first NUL
# must be NUL: node-tar reads looser forms differently (tests/dist/t_tar_readers.sh compares the
# two readers). Run by release.sh npm-check as `python3 -I` with no DEVELOPER_DIR.
#   npm-same.py packages DIR VERSION REF   the four .tgz files in DIR: that shape; exactly the files
#       npm-pack.sh writes; the license texts and the main package's launcher equal the files in
#       REF (a directory: the tag's LICENSE-MIT, LICENSE-APACHE and launcher); each Linux
#       package's binary equals DIR's sheepdog-linux-<arch>; executables 0755; each package.json
#       equals, key for key, the one `pkgjson` writes (no scripts; the main one's bin and optional
#       dependencies pinned to VERSION)
#   npm-same.py pkgjson KIND VERSION        prints the package.json of KIND (main, darwin,
#       linux-arm64, linux-x64): npm-pack.sh writes each package's from this, and `packages`
#       compares against it
#   npm-same.py same TGZ ARCHIVE           the darwin package's Sheepdog.app equals the release
#       archive's, file by file: path, mode and content; the executable 0755
# Exit 0, or 1 with the reason on stderr.
import sys, gzip, hashlib, json, os, re

def die(msg):
    sys.stderr.write("npm-same: " + msg + "\n"); sys.exit(1)

NAME = re.compile(r"^[A-Za-z0-9._+-]+(/[A-Za-z0-9._+-]+)*/?$")

NUM = re.compile(rb"^ *[0-7]+[ \0]+$")

def octal(b, what, field, o):
    if not NUM.match(b): die("%s: the %s field is not plain octal at byte %d: %r" % (what, field, o, b))
    return int(b.strip(b"\0 "), 8)

def plain(path, what, allow_dirs):
    try: data = gzip.decompress(open(path, "rb").read())
    except Exception as e: die("%s: cannot read %s (%s)" % (what, path, e))
    files, seen, o = {}, set(), 0
    while True:
        if o + 512 > len(data): die("%s: the archive ends without an end block" % what)
        h = data[o:o + 512]
        if h == b"\0" * 512:
            if data[o:].strip(b"\0"): die("%s: data after the end of the archive (an entry after a single null block?)" % what)
            return files
        if octal(h[148:156], what, "checksum", o) != sum(h[:148]) + 8 * 32 + sum(h[156:]):
            die("%s: bad header checksum at byte %d" % (what, o))
        if h[257:265] != b"ustar\x0000": die("%s: not a plain ustar header at byte %d" % (what, o))
        if h[345:500].strip(b"\0"): die("%s: a ustar prefix at byte %d" % (what, o))
        typ = h[156:157].decode("latin-1")
        raw, _, tail = h[0:100].partition(b"\0")
        if tail.strip(b"\0"): die("%s: bytes after the end of the name at byte %d" % (what, o))
        name = raw.decode("latin-1")
        parts = name.rstrip("/").split("/")
        if any(p.startswith("._") for p in parts): die("%s: an AppleDouble entry: %s" % (what, name))
        if not NAME.match(name) or any(p in (".", "..") for p in parts): die("%s: a name outside [A-Za-z0-9._+-/]: %r" % (what, raw))
        if typ not in (("0", "5") if allow_dirs else ("0",)): die("%s: type %r is not allowed (%s)" % (what, typ, name))
        if h[157:257].strip(b"\0"): die("%s: a link name on %s" % (what, name))
        key = name.rstrip("/").lower()
        if key in seen: die("%s: %s appears twice (case ignored)" % (what, name))
        seen.add(key)
        size = octal(h[124:136], what, "size", o); mode = octal(h[100:108], what, "mode", o) & 0o7777
        if typ == "5":
            if size: die("%s: a directory with data: %s" % (what, name))
            o += 512; continue
        if name.endswith("/"): die("%s: a file name ending in /: %s" % (what, name))
        body = data[o + 512:o + 512 + size]
        if len(body) < size: die("%s: %s is cut short" % (what, name))
        files[name] = (mode, hashlib.sha256(body).hexdigest(), body)
        o += 512 + -(-size // 512) * 512

def ref(p): return open(p, "rb").read()

LIC = ["LICENSE-MIT", "LICENSE-APACHE"]
def pkgjson(kind, nv):
    """The package.json npm-pack.sh writes for KIND, as a dict (key order is the file's order)."""
    lic = "MIT OR Apache-2.0"
    if kind == "main":
        return {"name": "@lukaso/sheepdog", "version": nv,
                "description": "Run a command and kill every process it started, escapees included",
                "license": lic, "bin": {"sheepdog": "bin/sheepdog"}, "files": ["bin"] + LIC,
                "optionalDependencies": {"@lukaso/sheepdog-" + k: nv for k in ("darwin-universal", "linux-arm64", "linux-x64")}}
    if kind == "darwin":
        return {"name": "@lukaso/sheepdog-darwin-universal", "version": nv, "license": lic,
                "os": ["darwin"], "cpu": ["arm64", "x64"], "files": ["Sheepdog.app"] + LIC}
    if kind in ("linux-arm64", "linux-x64"):
        return {"name": "@lukaso/sheepdog-" + kind, "version": nv, "license": lic,
                "os": ["linux"], "cpu": [kind[len("linux-"):]], "files": ["bin"] + LIC}
    die("no package kind %s" % kind)

def packages(d, nv, r):
    lic = {n: ref(os.path.join(r, n)) for n in LIC}
    for kind, arch in (("main", None), ("darwin", None), ("linux-arm64", "aarch64"), ("linux-x64", "x86_64")):
        pk = "sheepdog" if kind == "main" else "sheepdog-darwin-universal" if kind == "darwin" else "sheepdog-" + kind
        fn = "lukaso-%s-%s.tgz" % (pk, nv)
        f = plain(os.path.join(d, fn), fn, False)
        for n in f:
            if not n.startswith("package/"): die("%s: an entry outside package/: %s" % (fn, n))
        want = {"package/package.json"} | {"package/" + n for n in LIC}
        want |= {n for n in f if n.startswith("package/Sheepdog.app/")} if kind == "darwin" else {"package/bin/sheepdog"}
        if set(f) != want: die("%s: the files are %s, not %s" % (fn, sorted(f), sorted(want)))
        for n, b in lic.items():
            if f["package/" + n][2] != b: die("%s: %s differs from the repo's (at the tag)" % (fn, n))
        try: pj = json.loads(f["package/package.json"][2])
        except ValueError: die("%s: package.json is not JSON" % fn)
        if pj != pkgjson(kind, nv): die("%s: package.json is not the one npm-pack.sh writes: %s" % (fn, json.dumps(pj, sort_keys=True)))
        if kind == "darwin": continue
        exp, src = (ref(os.path.join(r, "launcher")), "the repo's launcher (at the tag)") if kind == "main" \
            else (ref(os.path.join(d, "sheepdog-linux-" + arch)), "sheepdog-linux-" + arch)
        m, _, b = f["package/bin/sheepdog"]
        if b != exp: die("%s: bin/sheepdog differs from %s" % (fn, src))
        if m & 0o777 != 0o755: die("%s: bin/sheepdog's mode is %o, not 755" % (fn, m))

def same(tgz, arc):
    p = {n[len("package/"):]: v[:2] for n, v in plain(tgz, "the darwin package", False).items() if n.startswith("package/Sheepdog.app/")}
    a = {n: v[:2] for n, v in plain(arc, "the release archive", True).items()}
    for n in a:
        if not n.startswith("Sheepdog.app/"): die("the release archive: an entry outside Sheepdog.app/: %s" % n)
    if sorted(p) != sorted(a):
        die("the darwin package's bundle is not the release archive's bundle: files %s differ" % sorted(set(p) ^ set(a)))
    for n in sorted(p):
        if p[n][0] != a[n][0]: die("the darwin package's bundle is not the release archive's bundle: %s has mode %o, the archive's %o" % (n, p[n][0], a[n][0]))
        if p[n][1] != a[n][1]: die("the darwin package's bundle is not the release archive's bundle: %s differs in content" % n)
    exe = "Sheepdog.app/Contents/MacOS/sheepdog"
    if exe not in p or p[exe][0] & 0o777 != 0o755: die("the executable's mode is not 0755")

if __name__ == "__main__":
    if len(sys.argv) == 5 and sys.argv[1] == "packages": packages(*sys.argv[2:5])
    elif len(sys.argv) == 4 and sys.argv[1] == "pkgjson": print(json.dumps(pkgjson(sys.argv[2], sys.argv[3]), indent=2))
    elif len(sys.argv) == 4 and sys.argv[1] == "same": same(*sys.argv[2:4])
    else:
        sys.stderr.write("usage: npm-same.py packages DIR VERSION REF | same TGZ ARCHIVE | pkgjson KIND VERSION\n"); sys.exit(2)
