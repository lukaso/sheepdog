#!/bin/sh
# scripts/lib/npm-same.py's reader against npm's own (the node-tar npm bundles, loaded offline),
# over whole .tgz files. The control is a real `npm pack` output (a package.json and a LICENSE-MIT
# whose last block leaves padding room): the parser accepts it and node-tar lists the same entries
# with no warning. Each fixture is that package with one change, on which the two readers were
# measured to part (reviews 5 and 6): a byte of 0x81 in uid, gid, mtime, devmajor or devminor
# (node-tar throws, warns, and reads on one block later), each with a header for binding.gyp in
# LICENSE-MIT's padding, which node-tar then reads; that padding header alone; a size with "_",
# "0o" or a leading NUL, in base-256, or with a trailing newline; a checksum of 8 digits; bytes
# after the name's NUL; a plain uid of 0, which node-tar reads fine but npm pack never writes (the
# header must be npm pack's own, byte for byte). Each must be refused, for the reason named. And for every fixture the
# parser accepts, node-tar must list exactly its entries, with no warning.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
NT=$(npm root -g 2>/dev/null)/npm/node_modules/tar
[ -f "$NT/index.js" ] || { fail "no npm-bundled node-tar at $NT"; finish; }
mkdir -p "$FX/pk" "$FX/h" && cp "$SD_ROOT/LICENSE-MIT" "$FX/pk/" && printf '{"name":"@lukaso/t","version":"0.0.1","files":["LICENSE-MIT"]}\n' > "$FX/pk/package.json"
(cd "$FX/pk" && env HOME="$FX/h" npm_config_cache="$FX/h/c" npm_config_userconfig=/dev/null npm pack --silent --pack-destination "$FX" > /dev/null) || { fail "npm pack"; finish; }
BASE=$FX/lukaso-t-0.0.1.tgz
[ -s "$BASE" ] || { fail "no $BASE"; finish; }
# the interpreter npm-check runs (its gzip code differs between versions)
/usr/bin/python3 -I -B - "$SD_ROOT/scripts/lib/npm-same.py" "$BASE" "$FX" <<'PY' > "$FX/rows"
import sys, gzip, io, json, importlib.util, contextlib
spec = importlib.util.spec_from_file_location("ns", sys.argv[1]); ns = importlib.util.module_from_spec(spec); spec.loader.exec_module(ns)
base, fx = sys.argv[2], sys.argv[3]
raw = gzip.decompress(open(base, "rb").read())
def heads(d):  # (offset, name) of each entry header, read in step
    o, out = 0, []
    while d[o:o + 512] != b"\0" * 512:
        out.append((o, d[o:o + 100].split(b"\0")[0])); size = int(d[o + 124:o + 136].strip(b"\0 "), 8); o += 512 + -(-size // 512) * 512
    return out
def at(d, name): return [o for o, n in heads(d) if n == name][0]
def resum(h):
    h = bytearray(h); h[148:156] = b" " * 8; h[148:156] = b"%06o \0" % sum(h); return bytes(h)
def setf(d, name, a, b, v, fix=True):  # bytes a:b of NAME's header set to v (the checksum redone)
    d = bytearray(d); o = at(bytes(d), name); d[o + a:o + b] = v
    if fix: d[o:o + 512] = resum(d[o:o + 512])
    return bytes(d)
L, P = b"package/LICENSE-MIT", b"package/package.json"
def padhdr(d):
    # LICENSE-MIT's last body block, whose padding follows the text's tail: a header there, named
    # "<the tail>/binding.gyp" (pacote strips the first path part), size 0, read only by a reader
    # that is a block out of step
    d = bytearray(d); o = at(bytes(d), L); size = int(d[o + 124:o + 136].strip(b"\0 "), 8)
    blk = o + 512 + 512 * (-(-size // 512) - 1); tail = o + 512 + size - blk
    assert 0 < tail < 88, "the fixture needs the text to end early in its last block"
    h = bytearray(d[o:o + 512]); h[0:tail] = d[blk:blk + tail]; h[tail:100] = b"/binding.gyp".ljust(100 - tail, b"\0")
    h[124:136] = b"%010o \0" % 0
    d[blk:blk + 512] = resum(bytes(h))
    return bytes(d)
cases = {"control": raw}
pd = padhdr(raw)
cases["a header in the padding (plain fields)"] = pd
for fld, a in (("uid", 108), ("gid", 116), ("mtime", 136), ("devmajor", 329), ("devminor", 337)):
    cases["0x81 in %s, a header in the padding" % fld] = setf(pd, L, a, a + 1, b"\x81")
cases['size with "_"'] = setf(raw, P, 124, 136, b"000000_0074\0")
cases['size with "0o"'] = setf(raw, P, 124, 136, b"0o0000000074")
cases["size with a leading NUL"] = setf(raw, P, 124, 136, b"\x000000000074\0")
cases["size in base-256"] = setf(raw, P, 124, 136, b"\x80" + b"\0" * 10 + b"\x3c")
cases["size with a trailing newline"] = setf(raw, P, 124, 136, b"0000000074 \n")
o8 = at(raw, P); h8 = bytearray(raw[o8:o8 + 512]); h8[148:156] = b" " * 8
cases["checksum of 8 digits"] = setf(raw, P, 148, 156, b"%08o" % sum(h8), fix=False)
cases["a plain uid of 0 (npm pack leaves it NUL)"] = setf(raw, P, 108, 116, b"0000000\0")
# two gzip members with a zero byte between: Python reads both, node's zlib and libarchive stop at
# the zero byte (package.json is in the second member)
gz_split = gzip.compress(raw[:at(raw, P)]) + b"\0" + gzip.compress(raw[at(raw, P):])
cases["a name ending in a newline"] = setf(raw, P, 0, 100, b"package/package.json\n".ljust(100, b"\0"))
cases["bytes after the name's NUL"] = setf(raw, P, 0, 100, b"package/package.json\0\nx".ljust(100, b"\0"))
out = {}
gz = {"a second gzip member after a zero byte": gz_split}
for i, (k, d) in enumerate(list(cases.items()) + list(gz.items())):
    p = "%s/c%d.tgz" % (fx, i); open(p, "wb").write(d if k in gz else gzip.compress(d))
    err = io.StringIO()
    try:
        with contextlib.redirect_stderr(err): f = ns.plain(p, k, False, canonical=True)
        acc, names = True, sorted(f)
    except (SystemExit, Exception) as e:  # a crash is a refusal too: release.sh stops on it
        acc, names = False, []
    out[k] = {"file": p, "accepted": acc, "entries": names, "why": err.getvalue().strip()}
print(json.dumps(out))
PY
[ -s "$FX/rows" ] || { fail "the fixtures were not made"; finish; }
node - "$NT" "$FX/rows" <<'JS' > "$FX/node.json"
const tar = require(process.argv[2]); const fs = require('fs');
const rows = JSON.parse(fs.readFileSync(process.argv[3], 'utf8')); const out = {};
for (const k of Object.keys(rows)) {
  const entries = [], warns = [];
  try {
    tar.t({ file: rows[k].file, sync: true, strict: false,
      onentry: e => { if (!/Link$/.test(e.type)) entries.push(e.path + (e.type === 'File' ? '' : ' (' + e.type + ')')); },
      onwarn: (code, msg) => warns.push(code + ' ' + msg) });
  } catch (e) { warns.push('threw ' + e.message); }
  out[k] = { entries: entries.sort(), warns };
}
console.log(JSON.stringify(out));
JS
/usr/bin/python3 -I -B - "$FX/rows" "$FX/node.json" <<'PY' > "$FX/cmp"
import sys, json
p = json.load(open(sys.argv[1])); n = json.load(open(sys.argv[2]))
def row(ok, msg): print(("ok: " if ok else "FAIL: ") + msg)
why = {"a header in the padding (plain fields)": "data in the padding", 'size with "_"': "not plain octal", 'size with "0o"': "not plain octal",
       "size with a leading NUL": "not plain octal", "size in base-256": "not plain octal", "size with a trailing newline": "not plain octal",
       "checksum of 8 digits": "not plain octal", "bytes after the name's NUL": "after the end of the name",
       "a plain uid of 0 (npm pack leaves it NUL)": "not the header npm pack writes",
       "a second gzip member after a zero byte": "not one gzip stream", "a name ending in a newline": "a name outside"}
c = p["control"]
row(c["accepted"] and c["entries"] == ["package/LICENSE-MIT", "package/package.json"] and n["control"] == {"entries": c["entries"], "warns": []},
    "control: a real npm pack output is accepted, and node-tar lists the same entries with no warning (%s / %s)" % (c, n["control"]))
for k in p:
    if k == "control": continue
    w = why.get(k, "not plain octal")  # 0x81 in a numeric field
    row(not p[k]["accepted"] and w in p[k]["why"], "%s: refused (%s) [%s]" % (k, w, p[k]["why"][:120]))
    if p[k]["accepted"]:
        row(n[k] == {"entries": p[k]["entries"], "warns": []}, "%s: node-tar lists the same entries (parser %s, node-tar %s)" % (k, p[k]["entries"], n[k]))
PY
[ $? = 0 ] || fail "the comparison did not run"
cat "$FX/cmp"
[ "$(grep -c '^ok' "$FX/cmp")" -ge 17 ] || fail "fewer than 17 rows passed (the control and 16 refusals)"
FAILS=$((FAILS + $(grep -c '^FAIL' "$FX/cmp")))
finish
