#!/bin/sh
# PHASE3.md S3 and D6: install.sh, as rendered for a release (scripts/lib/render-install.sh embeds
# the version and the exec door), on macOS with a temp HOME and a local server on 127.0.0.1.
#   - SHEEPDOG_INSTALL_BASE: https, or http on exactly 127.0.0.1, ::1 or localhost; anything with
#     @, #, ? or \ before the host ends, another scheme, or another host is refused (exit 3) before
#     any download;
#   - a checksum mismatch, or no SHA256SUMS line for the file: refused (1), nothing installed;
#   - a .dev bundle, an ad-hoc release-ID bundle, a linker-signed executable in a release-ID
#     bundle: refused (1), nothing installed, nothing run (the door's recording stays empty);
#   - no curl and no wget: refused, both named.
# The happy path, the upgrade and the symlink's process are in the rc leg (a real release needed).
set -u
. "$(dirname "$0")/lib.sh"
[ "$(uname -s)" = Darwin ] || { echo "SKIP (macOS only)"; exit 0; }
fx_dir
I=$FX/install.sh
sh "$SD_ROOT/scripts/lib/render-install.sh" "$SD_ROOT/scripts/install.sh" 0.1.0 "$I" || { fail "render"; finish; }
grep -q '^SHEEPDOG_VERSION=0.1.0$' "$I" && pass "rendered with the version" || fail "the version is not rendered in"
sed -n "/<<'SD_DOOR_EOF'/,/^SD_DOOR_EOF$/p" "$I" | sed '1d;$d' > "$FX/door.embedded"
cmp -s "$FX/door.embedded" "$SD_ROOT/scripts/lib/exec-guard.sh" \
  && pass "the embedded door is exec-guard.sh, byte for byte" || fail "the embedded door differs from exec-guard.sh"

# the server
mkdir -p "$FX/srv"
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
(cd "$FX/srv" && exec python3 -m http.server "$port" --bind 127.0.0.1) > "$FX/srv.log" 2>&1 & sp=$!
trap 'kill $sp 2>/dev/null; rm -rf "$FX"' EXIT
i=0; until curl -fs "http://127.0.0.1:$port/" >/dev/null 2>&1 || [ $i -gt 50 ]; do sleep 0.1; i=$((i + 1)); done
BASE=http://127.0.0.1:$port
H=$FX/home; mkdir -p "$H"
inst() { # base -> rc; output in $FX/o
  rec=$FX/rec; : > "$rec"
  env HOME="$H" SHEEPDOG_INSTALL_BASE="$1" SD_EXEC_RECORD="$rec" sh "$I" > "$FX/o" 2>&1
}
nothing() { [ ! -e "$H/Applications/Sheepdog.app" ] && [ ! -e "$H/.local/bin/sheepdog" ] && [ -z "$(ls -A "$H/Applications" 2>/dev/null)" ] && [ "$(grep -c . "$FX/rec")" = 0 ]; }

# the URL rules (D6)
for b in "http://127.0.0.1.nip.io/" "http://localhost@evil.example/" "http://127.0.0.1:80@evil/" "http://evil.example#@127.0.0.1/" \
         "http://evil.example?@127.0.0.1/" 'http://evil.example\@127.0.0.1/' "ftp://127.0.0.1/" "http://10.0.0.1/" "file:///tmp/"; do
  inst "$b"; r=$?
  [ $r = 3 ] && pass "base '$b': refused (3)" || fail "base '$b': rc=$r"
done
for b in "$BASE/" "http://[::1]:$port/" "http://localhost:$port/"; do
  inst "$b"; r=$?
  [ $r != 3 ] && pass "base '$b': passes the URL rules (rc $r)" || fail "base '$b': refused by the URL rules"
done

# archives to serve
printf 'int main(){return 0;}\n' > "$FX/m.c"; cc -o "$FX/bin" "$FX/m.c" || exit 3
serve() { # bundle-id sign(adhoc|none) [bad-sum|no-line]
  rm -rf "$FX/srv"/* "$FX/b"
  app=$("$SD_ROOT/scripts/bundle.sh" "$FX/bin" "$FX/b" 0.1.0 1 $( [ "$1" = com.lukaso.sheepdog ] && echo --release-id)) || exit 3
  [ "$2" = adhoc ] && codesign -s - -f "$app" 2>/dev/null
  "$SD_ROOT/scripts/lib/archive.sh" make "$app" "$FX/srv/sheepdog-macos-universal.tar.gz" || exit 3
  (cd "$FX/srv" && shasum -a 256 sheepdog-macos-universal.tar.gz > SHA256SUMS)
  case ${3:-} in
    bad-sum) cp "$FX/srv/SHA256SUMS" "$FX/sum0"
      awk '{c=substr($0,1,1); print (c=="0"?"1":"0") substr($0,2)}' "$FX/sum0" > "$FX/srv/SHA256SUMS"
      cmp -s "$FX/sum0" "$FX/srv/SHA256SUMS" && fail "the corrupted sum did not change" ;;
    no-line) : > "$FX/srv/SHA256SUMS" ;;
  esac
  rm -rf "$FX/b"
}
serve com.lukaso.sheepdog.dev adhoc bad-sum; inst "$BASE"; r=$?
[ $r = 1 ] && grep -qi 'checksum' "$FX/o" && nothing && pass "a checksum mismatch: refused, nothing installed" || fail "bad sum: rc=$r $(tail -1 "$FX/o")"
grep -q 'GET /sheepdog-macos-universal.tar.gz' "$FX/srv.log" && pass "it reached the real download (the server's log)" || fail "no GET of the archive in the server's log"
serve com.lukaso.sheepdog.dev adhoc no-line; inst "$BASE"; r=$?
[ $r = 1 ] && grep -qi 'checksum' "$FX/o" && nothing && pass "no SHA256SUMS line: refused, nothing installed" || fail "no line: rc=$r $(tail -1 "$FX/o")"
serve com.lukaso.sheepdog.dev adhoc; inst "$BASE"; r=$?
[ $r = 1 ] && grep -q "Developer ID" "$FX/o" && nothing && pass "a .dev bundle: refused (not the Developer ID signature), nothing installed, nothing run" || fail ".dev: rc=$r $(tail -1 "$FX/o")"
serve com.lukaso.sheepdog adhoc; inst "$BASE"; r=$?
[ $r = 1 ] && grep -q "Developer ID" "$FX/o" && nothing && pass "an ad-hoc release-ID bundle: refused (not the Developer ID signature), nothing installed, nothing run" || fail "adhoc release: rc=$r $(tail -1 "$FX/o")"
serve com.lukaso.sheepdog none; inst "$BASE"; r=$?
[ $r = 1 ] && nothing && pass "a linker-signed executable in a release-ID bundle: refused, nothing run" || fail "linker-signed: rc=$r $(tail -1 "$FX/o")"

# no HOME: refused by name (not a shell error)
serve com.lukaso.sheepdog.dev adhoc; rec=$FX/rec; : > "$rec"
env -u HOME SHEEPDOG_INSTALL_BASE="$BASE" SD_EXEC_RECORD="$rec" sh "$I" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q 'HOME' "$FX/o" && ! grep -q 'parameter not set\|unbound variable' "$FX/o" && pass "no HOME: refused, by name" || fail "no HOME: rc=$r $(tail -1 "$FX/o")"
# no curl, no wget: a PATH with the tools install.sh needs, but neither downloader
mkdir -p "$FX/p"
for t in sh tar mkdir mv rm ln uname mktemp sed grep awk cat shasum dirname basename head od tr readlink chmod cut id env sleep ls cp; do
  for d in /usr/bin /bin; do [ -x "$d/$t" ] && { ln -sf "$d/$t" "$FX/p/$t"; break; }; done
done
rec=$FX/rec; : > "$rec"
env PATH="$FX/p" HOME="$H" SHEEPDOG_INSTALL_BASE="$BASE" SD_EXEC_RECORD="$rec" "$FX/p/sh" "$I" > "$FX/o" 2>&1; r=$?
[ $r = 1 ] && grep -q curl "$FX/o" && grep -q wget "$FX/o" && nothing && pass "no curl and no wget: refused, both named" || fail "no downloader: rc=$r $(tail -1 "$FX/o")"
finish
