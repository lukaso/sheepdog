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
#   tgz-edit.py SRC DST raw-append NAME TYPE,MAGIC,PREFIX[,LINK]   SRC's entries, then one raw
#                                                  header (typeflag TYPE; MAGIC ustar00 or gnu;
#                                                  PREFIX or - for none; a link name) holding "x",
#                                                  then the end
#   tgz-edit.py SRC DST noend - -                  SRC's entries and no end blocks
#   tgz-edit.py SRC DST short NAME -               SRC's entries, then a header for 5000 bytes
#                                                  followed by 1 byte, and the stream ends
#   tgz-edit.py SRC DST badsum NAME -              SRC's entries, then a header whose checksum is
#                                                  wrong, then the end
#   tgz-edit.py SRC DST replace NAME TEXT          NAME's content replaced by TEXT
#   tgz-edit.py SRC DST json-set NAME KEY=JSON     NAME (a JSON file) with KEY set to JSON
import sys, tarfile, io, gzip, json
src, dst, op, name, arg = sys.argv[1:6]

def entries_raw(path):  # SRC's entries with no end blocks
    buf = io.BytesIO(); t = tarfile.open(fileobj=buf, mode="w", format=tarfile.PAX_FORMAT)
    with tarfile.open(path) as s:
        for m in s.getmembers():
            t.addfile(m, s.extractfile(m) if m.isreg() else None)
    return buf.getvalue()

def header(nm, typ, magic, prefix, size, badsum=False, link=""):
    h = bytearray(512)
    h[157:157 + len(link)] = link.encode()
    h[0:len(nm)] = nm.encode(); h[100:108] = b"0000644\0"; h[108:116] = b"0000000\0"; h[116:124] = b"0000000\0"
    h[124:136] = ("%011o\0" % size).encode(); h[136:148] = b"00000000000\0"; h[156:157] = typ.encode()
    h[257:265] = b"ustar\x0000" if magic == "ustar00" else b"ustar  \0"
    if prefix != "-": h[345:345 + len(prefix)] = prefix.encode()
    h[148:156] = b"        "
    c = sum(h) + (1 if badsum else 0)
    h[148:156] = ("%06o\0 " % c).encode()
    return bytes(h)

def finish(raw):
    raw += b"\0" * 1024; raw += b"\0" * (-len(raw) % 10240)
    open(dst, "wb").write(gzip.compress(raw)); sys.exit(0)

if op in ("raw-append", "badsum"):
    f = (["0", "ustar00", "-"] if op == "badsum" else arg.split(",")) + [""]
    finish(entries_raw(src) + header(name, f[0], f[1], f[2], 1, badsum=(op == "badsum"), link=f[3]) + b"x" + b"\0" * 511)
if op == "noend":
    open(dst, "wb").write(gzip.compress(entries_raw(src))); sys.exit(0)
if op == "short":
    open(dst, "wb").write(gzip.compress(entries_raw(src) + header(name, "0", "ustar00", "-", 5000) + b"x")); sys.exit(0)
if op in ("replace", "json-set"):
    with tarfile.open(dst, "w:gz", format=tarfile.PAX_FORMAT) as out, tarfile.open(src) as t:
        for m in t.getmembers():
            f = t.extractfile(m) if m.isreg() else None
            if m.name == name:
                if op == "replace": b = arg.encode()
                else:
                    d = json.loads(f.read()); k, v = arg.split("=", 1); d[k] = json.loads(v); b = json.dumps(d, indent=2).encode()
                m.size = len(b); f = io.BytesIO(b)
            out.addfile(m, f)
    sys.exit(0)
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
