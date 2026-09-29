#!/bin/sh
# The dist-linux leg's cells (PHASE3.md S3, the distribution cell's Linux half), run inside a
# pinned image (Alpine: busybox wget; Debian: no downloader) as root. /srv holds a rendered
# install.sh, the static binary as sheepdog-linux-<arch>, and SHA256SUMS; /tgt the Linux build
# (for sd-fixture's `serve`). The server runs in this container on 127.0.0.1, and its log shows
# every GET. Cells:
#   - with no curl and no wget: refused, both named, nothing installed;
#   - wget (if present), then curl (added with the image's package manager): installed, as root
#     into /usr/local/bin and as a plain user into ~/.local/bin; the installed binary runs a job and
#     passes its exit code through; the PATH hint when ~/.local/bin is not on PATH;
#   - a checksum mismatch: refused, nothing installed.
set -u
fails=0
ok() { echo "ok: $*"; }
bad() { echo "FAIL: $*"; fails=$((fails + 1)); }
art=sheepdog-linux-$(uname -m)
[ -f "/srv/$art" ] || { echo "FAIL: no /srv/$art"; exit 1; }
serve() { # dir -> sets PORT, SP; log in dir.log
  rm -f "$1.port"
  /tgt/debug/sd-fixture serve "$1" "$1.port" 2>"$1.log" & SP=$!
  i=0; while [ ! -s "$1.port" ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
  PORT=$(cat "$1.port")
}
cp -r /srv /tmp/good && serve /tmp/good; GP=$PORT; GS=$SP
cp -r /srv /tmp/badsum && sed -i 's/^./0/' /tmp/badsum/SHA256SUMS && serve /tmp/badsum; BP=$PORT; BS=$SP
inst() { # user base -> rc; output in /tmp/o
  if [ "$1" = root ]; then env SHEEPDOG_INSTALL_BASE="$2" sh /tmp/good/install.sh > /tmp/o 2>&1
  else su -s /bin/sh "$1" -c "env PATH=/usr/bin:/bin SHEEPDOG_INSTALL_BASE='$2' sh /tmp/good/install.sh" > /tmp/o 2>&1; fi
}
(adduser -D sduser 2>/dev/null || useradd -m sduser) >/dev/null 2>&1
UH=$(eval echo ~sduser)

if ! command -v curl >/dev/null 2>&1 && ! command -v wget >/dev/null 2>&1; then
  inst root "http://127.0.0.1:$GP"; r=$?
  [ $r = 1 ] && grep -q curl /tmp/o && grep -q wget /tmp/o && [ ! -e /usr/local/bin/sheepdog ] \
    && ok "no curl and no wget: refused, both named, nothing installed" || bad "no downloader: rc=$r $(tail -1 /tmp/o)"
fi
round() { # label
  rm -f /usr/local/bin/sheepdog "$UH/.local/bin/sheepdog"
  inst root "http://127.0.0.1:$GP"; r=$?
  if [ $r = 0 ] && [ -x /usr/local/bin/sheepdog ]; then ok "$1, root: installed into /usr/local/bin"; else bad "$1, root: rc=$r $(tail -2 /tmp/o | tr '\n' ' ')"; fi
  /usr/local/bin/sheepdog run -- sh -c 'exit 7' >/dev/null 2>&1; r=$?
  [ $r = 7 ] && ok "$1: the installed binary runs a job (exit 7 passed through)" || bad "$1: the job's exit code: $r"
  inst sduser "http://127.0.0.1:$GP"; r=$?
  [ $r = 0 ] && [ -x "$UH/.local/bin/sheepdog" ] && ok "$1, a plain user: installed into ~/.local/bin" || bad "$1, user: rc=$r $(tail -2 /tmp/o | tr '\n' ' ')"
  grep -q 'is not on your PATH' /tmp/o && ok "$1: the PATH hint" || bad "$1: no PATH hint"
}
if command -v wget >/dev/null 2>&1; then round wget; fi
if command -v apk >/dev/null 2>&1; then apk add -q curl >/dev/null 2>&1; else (apt-get update -qq && apt-get install -y -qq curl) >/dev/null 2>&1; fi
command -v curl >/dev/null 2>&1 && round curl || bad "curl could not be added"
grep -q "GET /$art" /tmp/good.log && grep -q 'GET /SHA256SUMS' /tmp/good.log && ok "the server's log shows the real downloads" || bad "no GET in the server's log"

rm -f /usr/local/bin/sheepdog
env SHEEPDOG_INSTALL_BASE="http://127.0.0.1:$BP" sh /tmp/good/install.sh > /tmp/o 2>&1; r=$?
[ $r = 1 ] && grep -qi checksum /tmp/o && [ ! -e /usr/local/bin/sheepdog ] && ok "a checksum mismatch: refused, nothing installed" || bad "bad sum: rc=$r $(tail -1 /tmp/o)"
kill "$GS" "$BS" 2>/dev/null
[ $fails = 0 ] && { echo "PASS dist-linux"; exit 0; }
echo "RED dist-linux: $fails"; exit 1
