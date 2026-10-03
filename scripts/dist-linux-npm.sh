#!/bin/sh
# The dist-linux leg's npm cells (PHASE3.md §3: cell 18 and D9 on Linux), run inside a pinned image
# as root. /pk holds two static registries made by scripts/lib/static-registry.py from what
# scripts/lib/npm-pack.sh packed around this tree's Linux builds (reg: the main package and the
# Linux ones; reg-noplat: the main one only), bin-<platform>: the binary in that platform's
# package, and launcher: the main package's launcher. Node and npm come from the image's package
# manager. A plain user installs @lukaso/sheepdog globally into a temp prefix from 127.0.0.1.
# Cells:
#   - the platform package's tarball came from this registry; the PATH entry is a link to the
#     main package's launcher, byte for byte the package's; the installed binary is this platform's
#     package's, byte for byte, the only one; every installed file's write bits are within the
#     user's umask;
#   - through the PATH entry (the launcher is POSIX sh: busybox ash or dash here): --version; a
#     job's exit code passes through; the running process is the installed binary, with the
#     launcher's pid (/proc/<pid>/exe); a job's signal dispositions and mask equal a direct run's;
#     TERM to that pid ends the job's tree, a setsid escapee included;
#   - control: from a registry without the platform package, the install still succeeds (the
#     dependency is optional), and the launcher exits 1 naming the missing package.
set -u
fails=0
ok() { echo "ok: $*"; }
bad() { echo "FAIL: $*"; fails=$((fails + 1)); }
case $(uname -m) in aarch64|arm64) plat=linux-arm64 ;; x86_64) plat=linux-x64 ;; *) echo "FAIL: no package for $(uname -m)"; exit 1 ;; esac
[ -f "/pk/bin-$plat" ] && [ -f /pk/launcher ] && [ -d /pk/reg ] && [ -d /pk/reg-noplat ] || { echo "FAIL: /pk is incomplete"; exit 1; }
if command -v apk >/dev/null 2>&1; then apk add -q nodejs npm >/dev/null 2>&1
else (apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq nodejs npm) >/dev/null 2>&1; fi
command -v node >/dev/null 2>&1 && command -v npm >/dev/null 2>&1 || { echo "FAIL: node and npm could not be added"; exit 1; }
echo "node $(node --version), npm $(npm --version), /bin/sh is $(readlink -f /bin/sh), $plat"

# the registry: a file under ROOT by its decoded path (npm asks for /@lukaso%2fsheepdog), nothing
# outside ROOT; every request logged
cat > /tmp/serve.js <<'JS'
const http = require("http"), fs = require("fs"), path = require("path");
const [root, port, log, up] = process.argv.slice(2);
http.createServer((q, s) => {
  fs.appendFileSync(log, q.method + " " + q.url + "\n");
  const f = path.join(root, decodeURIComponent(q.url.split("?")[0]));
  if (q.method !== "GET" || !f.startsWith(root + "/")) { s.writeHead(404); return s.end(); }
  fs.readFile(f, (e, b) => {
    if (e) { s.writeHead(404); return s.end(); }
    s.writeHead(200, { "content-type": f.endsWith(".tgz") ? "application/octet-stream" : "application/json" }); s.end(b);
  });
}).listen(+port, "127.0.0.1", () => fs.writeFileSync(up, ""));
JS
SPS=""
serve() { # root port
  rm -f "/tmp/up.$2"; node /tmp/serve.js "$1" "$2" "/tmp/reg.$2.log" "/tmp/up.$2" & SPS="$SPS $!"
  i=0; while [ ! -e "/tmp/up.$2" ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
  [ -e "/tmp/up.$2" ] || bad "the registry on port $2 did not start"
}
serve /pk/reg 4873; serve /pk/reg-noplat 4874
(adduser -D sduser 2>/dev/null || useradd -m sduser) >/dev/null 2>&1
UH=$(eval echo ~sduser)
asu() { su -s /bin/sh sduser -c "$1"; }
npmi() { # prefix port -> rc; output in /tmp/ni
  asu "env HOME='$UH' npm_config_cache='$UH/.npm-cache' npm_config_update_notifier=false npm_config_audit=false npm_config_fund=false \
    npm i -g --prefix '$1' --registry http://127.0.0.1:$2/ @lukaso/sheepdog" > /tmp/ni 2>&1
}

P=$UH/npm-global E=$UH/npm-global/bin/sheepdog
npmi "$P" 4873; r=$?
[ $r = 0 ] && ok "npm i -g @lukaso/sheepdog from the static registry" || bad "npm i -g: rc=$r $(tail -3 /tmp/ni | tr '\n' ' ')"
grep -q "^GET /t/lukaso-sheepdog-$plat-" /tmp/reg.4873.log && ok "the $plat tarball came from this registry" || bad "no GET of the $plat tarball: $(tr '\n' ' ' < /tmp/reg.4873.log)"
L=$P/lib/node_modules/@lukaso/sheepdog/bin/sheepdog
[ -L "$E" ] && [ "$(readlink -f "$E")" = "$(readlink -f "$L")" ] && cmp -s "$L" /pk/launcher \
  && ok "the PATH entry links to the main package's launcher, byte for byte" || bad "the PATH entry: $(ls -l "$E" 2>&1)"
B=$(find "$P/lib/node_modules" -path "*/@lukaso/sheepdog-$plat/bin/sheepdog" -type f | head -1)
n=$(find "$P/lib/node_modules" -path '*/@lukaso/sheepdog-*/bin/sheepdog' | wc -l | tr -d ' ')
[ -n "$B" ] && [ "$n" = 1 ] && cmp -s "$B" "/pk/bin-$plat" && ok "the installed binary is the $plat package's, byte for byte, the only one" || bad "the installed binary: '$B', $n found"
# write bits stay within the user's umask (npm applies it to every file and directory: Debian's su
# gives 002, so group-writable there, measured; bun was measured to make files world-writable)
um=$(asu umask); m=$(( 0$um ))
wb=""; [ $((m & 2)) != 0 ] && wb="-perm -002"; [ $((m & 16)) != 0 ] && wb="${wb:+$wb -o }-perm -020"
w=""; [ -n "$wb" ] && w=$(find "$P" ! -type l \( $wb \) | head -3 | tr '\n' ' ')
[ -n "$wb" ] && [ -z "$w" ] && ok "every installed file's write bits are within the user's umask ($um)" || bad "umask $um, write bits beyond it: $w"

v=$(asu "'$E' --version" 2>&1); case $v in "sheepdog "*", linux)") ok "--version through the PATH entry: $v" ;; *) bad "--version: $v" ;; esac
asu "'$E' run -- sh -c 'exit 7'" >/dev/null 2>&1; r=$?
[ $r = 7 ] && ok "a job through the PATH entry: exit 7 passed through" || bad "a job's exit code: $r"
# the same signal state as a direct run of the binary
sig="grep -E '^Sig(Ign|Blk):' /proc/self/status"
a=$(asu "'$E' run -- sh -c \"$sig\"" 2>&1) b=$(asu "'$B' run -- sh -c \"$sig\"" 2>&1)
[ -n "$a" ] && [ "$a" = "$b" ] && ok "a job's signal dispositions and mask equal a direct run's" || bad "signals: launcher '$a', direct '$b'"
# the process the PATH entry starts is the binary, with the launcher's pid (the launcher execs)
rm -f /tmp/lp /tmp/esc
asu "'$E' run -- sh -c 'setsid sleep 300 & echo \$! > /tmp/esc; exec sleep 300' & echo \$! > /tmp/lp; wait" >/dev/null 2>&1 & su=$!
i=0; while { [ ! -s /tmp/lp ] || [ ! -s /tmp/esc ]; } && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
lp=$(cat /tmp/lp 2>/dev/null) esc=$(cat /tmp/esc 2>/dev/null)
# read as the process's own user: root in a container has no CAP_SYS_PTRACE for another user's exe
x=""; i=0; while [ $i -lt 50 ]; do x=$(asu "readlink /proc/$lp/exe" 2>/dev/null); [ "$x" = "$(readlink -f "$B")" ] && break; sleep 0.1; i=$((i + 1)); done
[ -n "$lp" ] && [ "$x" = "$(readlink -f "$B")" ] && ok "the process the PATH entry started is the installed binary, with the launcher's pid ($lp)" || bad "pid $lp runs '$x', not $B"
[ -n "$esc" ] && [ "$(sed -n 's/^[^)]*) [A-Z] [0-9]* [0-9]* \([0-9]*\).*/\1/p' "/proc/$esc/stat" 2>/dev/null)" = "$esc" ] \
  && ok "the escapee is in a session of its own ($esc)" || bad "the escapee $esc did not leave the session"
[ -n "$lp" ] && kill -TERM "$lp"; wait $su
i=0; while kill -0 "$esc" 2>/dev/null && [ $i -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
[ -n "$esc" ] && ! kill -0 "$esc" 2>/dev/null && ok "TERM to that pid ended the job's tree, the setsid escapee included" || bad "the escapee $esc survived the TERM"

# control: the platform package missing from the registry
P2=$UH/npm-noplat
npmi "$P2" 4874; r=$?
[ $r = 0 ] && [ -z "$(find "$P2/lib/node_modules" -path '*/@lukaso/sheepdog-*' 2>/dev/null)" ] && ok "control: without the platform package, the install succeeds and holds no binary" || bad "control install: rc=$r $(tail -2 /tmp/ni | tr '\n' ' ')"
o=$(asu "'$P2/bin/sheepdog' --version" 2>&1); r=$?
[ $r = 1 ] && case $o in *"@lukaso/sheepdog-$plat"*) true ;; *) false ;; esac && ok "control: the launcher exits 1 naming @lukaso/sheepdog-$plat" || bad "control launcher: rc=$r $o"

for s in $SPS; do kill "$s" 2>/dev/null; done
[ -n "$esc" ] && kill -KILL "$esc" 2>/dev/null
echo "dist-linux npm: $fails failed"
[ $fails = 0 ]
