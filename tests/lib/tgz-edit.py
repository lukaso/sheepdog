#!/usr/bin/env python3
# Test helper: copy a .tgz entry by entry (raw, no extraction), with one change.
#   tgz-edit.py SRC DST add NAME TEXT        a regular file NAME holding TEXT, appended
#   tgz-edit.py SRC DST symlink NAME TARGET  a symlink NAME -> TARGET, appended
#   tgz-edit.py SRC DST mode NAME OCTAL      NAME's mode set to OCTAL
#   tgz-edit.py SRC DST only-symlink NAME TARGET   nothing but a symlink NAME -> TARGET
#   tgz-edit.py SRC DST after-null NAME TEXT       SRC's entries, ONE null block, then a regular
#                                                  file NAME holding TEXT, then the end: Python's
#                                                  tarfile and macOS tar stop at the null block,
#                                                  npm's tar (two null blocks for the end) reads on
import sys, tarfile, io, gzip
src, dst, op, name, arg = sys.argv[1:6]
if op == "after-null":
    buf = io.BytesIO()
    t = tarfile.open(fileobj=buf, mode="w", format=tarfile.PAX_FORMAT)
    with tarfile.open(src) as s:
        for m in s.getmembers():
            t.addfile(m, s.extractfile(m) if m.isreg() else None)
    body = buf.getvalue()  # the entries, no end blocks yet (close() is never called on t)
    b = arg.encode(); i = tarfile.TarInfo(name); i.size = len(b); i.mode = 0o644
    extra = i.tobuf(tarfile.PAX_FORMAT) + b + b"\0" * (-len(b) % 512)
    raw = body + b"\0" * 512 + extra + b"\0" * 1024
    raw += b"\0" * (-len(raw) % 10240)
    open(dst, "wb").write(gzip.compress(raw))
    sys.exit(0)
with tarfile.open(dst, "w:gz", format=tarfile.PAX_FORMAT) as out:
    if op != "only-symlink":
        with tarfile.open(src) as t:
            for m in t.getmembers():
                f = t.extractfile(m) if m.isreg() else None
                if op == "mode" and m.name == name:
                    m.mode = int(arg, 8)
                out.addfile(m, f)
    if op == "add":
        b = arg.encode(); i = tarfile.TarInfo(name); i.size = len(b); i.mode = 0o644
        out.addfile(i, io.BytesIO(b))
    elif op in ("symlink", "only-symlink"):
        i = tarfile.TarInfo(name); i.type = tarfile.SYMTYPE; i.linkname = arg; i.mode = 0o755
        out.addfile(i)
