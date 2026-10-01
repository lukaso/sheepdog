#!/usr/bin/env python3
# Test helper: copy a .tgz with one change. Untouched entries are copied byte for byte (header, data,
# padding); a new or changed entry gets a header built the way the source's are: npm pack's own
# (scripts/lib/npm-same.py npm_header) when every source header is one, else a plain ustar header.
# So a row that changes one thing is refused only for that thing.
#   tgz-edit.py SRC DST add NAME TEXT               a regular file NAME holding TEXT, appended
#   tgz-edit.py SRC DST symlink NAME TARGET         a symlink NAME -> TARGET, appended
#   tgz-edit.py SRC DST mode NAME OCTAL             NAME's mode set to OCTAL
#   tgz-edit.py SRC DST only-symlink NAME TARGET    nothing but a symlink NAME -> TARGET
#   tgz-edit.py SRC DST after-null NAME TEXT        SRC's entries, ONE null block, then a file NAME
#                                                   holding TEXT, then the end (npm's tar reads on
#                                                   past one null block; tarfile and macOS tar stop)
#   tgz-edit.py SRC DST raw-append NAME TYPE,MAGIC,PREFIX[,LINK]   SRC's entries, then one raw
#                                                   header (typeflag TYPE; MAGIC ustar00 or gnu;
#                                                   PREFIX or - for none; a link name) holding "x"
#   tgz-edit.py SRC DST badsum NAME -               SRC's entries, then a header whose checksum is wrong
#   tgz-edit.py SRC DST noend - -                   SRC's entries and no end blocks
#   tgz-edit.py SRC DST short NAME -                SRC's entries, then a header for 5000 bytes
#                                                   followed by 1 byte, and the stream ends
#   tgz-edit.py SRC DST replace NAME TEXT           NAME's content replaced by TEXT
#   tgz-edit.py SRC DST json-set NAME KEY=JSON      NAME (a JSON file) with KEY set to JSON
#   tgz-edit.py SRC DST remove NAME -               SRC without the entry NAME
import sys, gzip, json, os, importlib.util
src, dst, op, name, arg = sys.argv[1:6]
spec = importlib.util.spec_from_file_location("ns", os.path.join(os.path.dirname(os.path.abspath(__file__)), "../../scripts/lib/npm-same.py"))
ns = importlib.util.module_from_spec(spec); sys.dont_write_bytecode = True; spec.loader.exec_module(ns)

def entries(path):  # [(name, header, body)] read in step; the source is a plain archive
    d = gzip.decompress(open(path, "rb").read()); o, out = 0, []
    while d[o:o + 512] != b"\0" * 512:
        h = d[o:o + 512]; size = int(h[124:136].strip(b"\0 "), 8)
        out.append((h[0:100].split(b"\0")[0], h, d[o + 512:o + 512 + size])); o += 512 + -(-size // 512) * 512
    return out

def pad(b): return b + b"\0" * (-len(b) % 512)
def resum(h):
    h = bytearray(h); h[148:156] = b" " * 8; h[148:156] = b"%06o \0" % sum(h); return bytes(h)

def header(nm, typ="0", magic="ustar00", prefix="-", size=1, mode=0o644, badsum=False, link=""):
    h = bytearray(512)
    h[0:len(nm)] = nm.encode(); h[100:108] = b"%06o \0" % mode; h[108:116] = b"0000000\0"; h[116:124] = b"0000000\0"
    h[124:136] = b"%011o\0" % size; h[136:148] = b"00000000000\0"; h[156:157] = typ.encode()
    h[157:157 + len(link)] = link.encode()
    h[257:265] = b"ustar\x0000" if magic == "ustar00" else b"ustar  \0"
    if prefix != "-": h[345:345 + len(prefix)] = prefix.encode()
    h[148:156] = b" " * 8; c = sum(h) + (1 if badsum else 0); h[148:156] = b"%06o\0 " % c
    return bytes(h)

E = entries(src)
canon = all(h == ns.npm_header(n, int(h[100:108].strip(b"\0 "), 8), len(b)) for n, h, b in E)
def new(nm, body, mode=0o644):  # a regular file's header, built as the source's are
    return ns.npm_header(nm.encode(), mode, len(body)) if canon else header(nm, size=len(body), mode=mode)
def raw(es): return b"".join(h + pad(b) for n, h, b in es)
def finish(stream, end=True):
    if end: stream += b"\0" * 1024; stream += b"\0" * (-len(stream) % 10240)
    open(dst, "wb").write(gzip.compress(stream)); sys.exit(0)

N = name.encode()
if op == "add": finish(raw(E) + new(name, arg.encode()) + pad(arg.encode()))
if op == "symlink": finish(raw(E) + header(name, typ="2", size=0, mode=0o755, link=arg))
if op == "only-symlink": finish(header(name, typ="2", size=0, mode=0o755, link=arg))
if op == "after-null": finish(raw(E) + b"\0" * 512 + new(name, arg.encode()) + pad(arg.encode()))
if op == "raw-append":
    f = arg.split(",") + [""]
    finish(raw(E) + header(name, typ=f[0], magic=f[1], prefix=f[2], link=f[3]) + pad(b"x"))
if op == "badsum": finish(raw(E) + header(name, badsum=True) + pad(b"x"))
if op == "noend": finish(raw(E), end=False)
if op == "short": finish(raw(E) + header(name, size=5000) + b"x", end=False)
if op == "remove":
    assert any(n == N for n, h, b in E), "no entry " + name
    finish(raw([e for e in E if e[0] != N]))
if op in ("mode", "replace", "json-set"):
    out = []
    for n, h, b in E:
        if n == N:
            m = int(h[100:108].strip(b"\0 "), 8)
            if op == "mode": m = int(arg, 8)
            elif op == "replace": b = arg.encode()
            else:
                d = json.loads(b); k, v = arg.split("=", 1); d[k] = json.loads(v); b = json.dumps(d, indent=2).encode()
            h = new(n.decode(), b, m) if canon else resum(bytes(bytearray(h[:100]) + b"%06o \0" % m + h[108:124] + b"%011o\0" % len(b) + h[136:]))
        out.append((n, h, b))
    finish(raw(out))
sys.exit("tgz-edit: no op " + op)
