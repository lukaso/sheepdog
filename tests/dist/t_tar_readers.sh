#!/bin/sh
# scripts/lib/npm-same.py's header parser against npm's own tar reader (the node-tar that npm
# bundles, loaded offline): for every crafted header the parser accepts, node-tar must read the
# same path and size and find the checksum valid. The crafted headers are the forms on which the
# two readers were measured to differ (review 5): a size with "_", with "0o", with a leading NUL,
# in base-256; a checksum of 8 digits with no terminator; bytes after the first NUL of the name.
# Each must be refused. Control: a plain header, accepted, and read the same by both.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
T=$(npm root -g 2>/dev/null)/npm/node_modules/tar/lib/header.js
[ -f "$T" ] || { fail "no npm-bundled node-tar at $T"; finish; }
python3 - "$SD_ROOT/scripts/lib/npm-same.py" "$FX" <<'PY' > "$FX/rows"
import sys, gzip, importlib.util, json
spec = importlib.util.spec_from_file_location("ns", sys.argv[1]); ns = importlib.util.module_from_spec(spec); spec.loader.exec_module(ns)
fx = sys.argv[2]
def hdr(name=b"package/f", size=b"%011o\0" % 1024, ck=None, mode=b"0000644\0"):
    h = bytearray(512); h[0:len(name)] = name; h[100:108] = mode; h[108:116] = b"0000000\0"; h[116:124] = b"0000000\0"
    h[124:136] = size; h[136:148] = b"00000000000\0"; h[156:157] = b"0"; h[257:265] = b"ustar\x0000"
    h[148:156] = b" " * 8; c = sum(h)
    h[148:156] = (b"%06o\0 " % c) if ck is None else ck(c)
    return bytes(h)
cases = {
  "control": hdr(),
  "size with _": hdr(size=b"0000002_000\0"),
  "size with 0o": hdr(size=b"0o000002000\0"),
  "size with a leading NUL": hdr(size=b"\x0000000002000"),
  "size in base-256": hdr(size=b"\x80" + b"\0" * 9 + b"\x04\x00"),
  "checksum of 8 digits, no terminator": hdr(ck=lambda c: b"%08o" % c),
  "bytes after the name's NUL": hdr(name=b"package/bin/sheepdog\0\nx"),
}
out = {}
for k, h in cases.items():
    body = b"y" * 1024
    p = "%s/%s.tgz" % (fx, abs(hash(k)))
    open(p, "wb").write(gzip.compress(h + body + b"\0" * 1024 + b"\0" * 8192))
    try:
        f = ns.plain(p, k, False); acc = True; name, size = list(f)[0], len(list(f.values())[0][2])
    except SystemExit:
        acc, name, size = False, None, None
    open(p + ".h", "wb").write(h)
    out[k] = {"accepted": acc, "name": name, "size": size, "hdr": p + ".h"}
print(json.dumps(out))
PY
node - "$T" "$FX/rows" <<'JS' > "$FX/node.json"
const Header = require(process.argv[2]); const fs = require('fs');
const rows = JSON.parse(fs.readFileSync(process.argv[3], 'utf8')); const out = {};
for (const k of Object.keys(rows)) { const h = new Header(fs.readFileSync(rows[k].hdr)); out[k] = {path: h.path, size: h.size, ok: h.cksumValid}; }
console.log(JSON.stringify(out));
JS
python3 - "$FX/rows" "$FX/node.json" <<'PY' > "$FX/cmp"
import sys, json
p = json.load(open(sys.argv[1])); n = json.load(open(sys.argv[2]))
def row(ok, msg): print(("ok: " if ok else "FAIL: ") + msg)
row(p["control"]["accepted"] and n["control"] == {"path": p["control"]["name"], "size": p["control"]["size"], "ok": True},
    "control: a plain header is accepted, and node-tar reads the same path and size (%s / %s)" % (p["control"], n["control"]))
for k in p:
    if k == "control": continue
    row(not p[k]["accepted"], "%s: refused by the parser" % k)
    if p[k]["accepted"]:
        row(n[k] == {"path": p[k]["name"], "size": p[k]["size"], "ok": True}, "%s: node-tar reads it the same (parser %s, node-tar %s)" % (k, p[k], n[k]))
PY
[ $? = 0 ] || fail "the comparison did not run"
cat "$FX/cmp"
[ "$(grep -c '^ok' "$FX/cmp")" -ge 7 ] || fail "fewer than 7 rows passed (the control and the six refusals)"
FAILS=$((FAILS + $(grep -c '^FAIL' "$FX/cmp")))
finish
