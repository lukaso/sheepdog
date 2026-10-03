#!/bin/sh
# PHASE3.md S3, the Mac happy path, against the operator's rc output (SD_RC_DIR; read only: the
# three files install.sh fetches are copied and served on 127.0.0.1): the release's own install.sh
# (rendered by the build) with a temp HOME and a temp TMPDIR.
#   - the install: install.sh names the version, commit and an active responsibility API; the
#     app in ~/Applications is a directory (not a link), file for file the release archive's
#     bundle; ~/.local/bin/sheepdog links to its executable; the PATH hint names ~/.local/bin,
#     and with SHELL=/bin/zsh its line goes into ~/.zprofile (control: no hint when it is on PATH);
#   - the installed sheepdog, after the door: a job's process is the app's executable, its exit
#     code passes through, and scripts/smoke.sh passes, all of its state under a temp HOME;
#   - after every install, nothing but Sheepdog.app is in Applications, and install.sh used its
#     TMPDIR and left it the same directory (not a link, the same inode), empty (controls for both);
#   - a second install over the first replaces the app (a planted file is gone) and the link still
#     resolves; a link that cannot be made (~/.local/bin a file, or a directory it cannot write)
#     exits 1 saying the app is in place; when neither the new app nor the old one can be moved into
#     place (a stand-in mv), it exits 1 naming where the old app is, and it is there;
#   - refused, with nothing installed and nothing left in Applications or TMPDIR: a copy of the rc
#     whose archive holds a tampered bundle (codesign), and the control (Gatekeeper: only a control
#     that carries the control marker can show this; rc.1's shares rc.1's notarized CDHash).
set -u
. "$(dirname "$0")/../dist/lib.sh"
[ "$(uname -s)" = Darwin ] || { echo "FAIL: the rc leg needs macOS"; exit 1; }
fx_dir
RC=${SD_RC_DIR%/} CT=${SD_RC_CONTROL_DIR%/}
ARC=sheepdog-macos-universal.tar.gz
three() { mkdir -p "$2" && cp "$1/install.sh" "$1/SHA256SUMS" "$1/$ARC" "$2/"; }
three "$RC" "$FX/srv" && three "$CT" "$FX/srvc" && three "$RC" "$FX/srvt" || { fail "cannot copy the rc's files"; finish; }
# the tampered copy: the rc bundle with one Info.plist byte changed, SHA256SUMS updated
mkdir -p "$FX/tb" && tar -xzf "$RC/$ARC" -C "$FX/tb" && perl -pi -e 's/<string>APPL<\/string>/<string>APPl<\/string>/' "$FX/tb/Sheepdog.app/Contents/Info.plist" \
  && grep -q APPl "$FX/tb/Sheepdog.app/Contents/Info.plist" && "$SD_ROOT/scripts/lib/archive.sh" make "$FX/tb/Sheepdog.app" "$FX/srvt/$ARC.new" \
  && mv "$FX/srvt/$ARC.new" "$FX/srvt/$ARC" && h=$(shasum -a 256 "$FX/srvt/$ARC" | cut -d' ' -f1) \
  && sed "s/^[0-9a-f]\{64\}  $ARC\$/$h  $ARC/" "$FX/srvt/SHA256SUMS" > "$FX/srvt/S" && mv "$FX/srvt/S" "$FX/srvt/SHA256SUMS" \
  && grep -q "^$h  $ARC\$" "$FX/srvt/SHA256SUMS" || fail "the tampered copy could not be made"
SPS=""; trap 'for s in $SPS; do { kill $s; wait $s; } 2>/dev/null; done; rm -rf "$FX"' EXIT
serve() { # dir: its port into dir.port
  p=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
  (cd "$1" && exec python3 -m http.server "$p" --bind 127.0.0.1) > "$1.log" 2>&1 & SPS="$SPS $!"
  i=0; until curl -fs "http://127.0.0.1:$p/" >/dev/null 2>&1 || [ $i -gt 50 ]; do sleep 0.1; i=$((i + 1)); done
  echo "$p" > "$1.port"
}
serve "$FX/srv"; serve "$FX/srvc"; serve "$FX/srvt"
n=0
inst() { # served-dir HOME [extra PATH] -> rc; output in $FX/o; install.sh's TMPDIR in $TD (fresh;
  # its mtime and inode before the install in $TDM, $TDI)
  n=$((n + 1)); TD=$FX/tmp.$n; mkdir -p "$TD"; TDM=$(stat -f %Fm "$TD") TDI=$(stat -f %i "$TD"); sleep 0.01
  env -i PATH="${3:+$3:}/usr/bin:/bin:/usr/sbin" HOME="$2" TMPDIR="$TD" ${ISHELL:+SHELL="$ISHELL"} SHEEPDOG_INSTALL_BASE="http://127.0.0.1:$(cat "$1.port")" sh "$1/install.sh" > "$FX/o" 2>&1
}
tmp_ok() { # dir mtime inode -> rc 0: still the same directory (not a link, the same inode), used by
  # install.sh (its mtime moved) and left with nothing in it
  [ -d "$1" ] && [ ! -L "$1" ] && [ "$(stat -f %i "$1" 2>/dev/null)" = "$3" ] && [ "$(stat -f %Fm "$1" 2>/dev/null)" != "$2" ] \
    && [ -z "$(ls -A "$1" 2>/dev/null)" ]
}
clean() { # what
  tmp_ok "$TD" "$TDM" "$TDI" && pass "$1: install.sh used TMPDIR and left nothing in it" \
    || fail "$1: TMPDIR $(ls -ld "$TD" 2>&1); inode $TDI -> $(stat -f %i "$TD" 2>&1), mtime $TDM -> $(stat -f %Fm "$TD" 2>&1), left: $(ls -A "$TD" 2>&1 | tr '\n' ' ')"
}
# controls of tmp_ok: a dir used and emptied passes; removed, removed and made again, replaced by a
# link to an empty dir, not used, or with a file left, each fails
tc() { # what expect(ok|bad) setup-command
  d=$FX/tc.$2.$(printf '%s' "$1" | tr -c 'a-z' _); mkdir -p "$d"; m=$(stat -f %Fm "$d") i=$(stat -f %i "$d"); sleep 0.01
  eval "$3"
  if tmp_ok "$d" "$m" "$i"; then [ $2 = ok ] && pass "control: TMPDIR $1 passes" || fail "control: TMPDIR $1 passes the check"
  else [ $2 = bad ] && pass "control: TMPDIR $1 fails the check" || fail "control: TMPDIR $1 fails the check"; fi
}
tc "used and emptied" ok ': > "$d/x"; rm "$d/x"'
tc "removed" bad 'rm -rf "$d"'
tc "removed and made again" bad 'rm -rf "$d"; mkdir "$d"; [ "$(stat -f %i "$d")" != "$i" ] || fail "control: the re-made TMPDIR kept its inode (the row would prove nothing)"'
tc "replaced by a link to an empty dir" bad 'rm -rf "$d"; mkdir "$d.e"; ln -s "$d.e" "$d"'
tc "not used" bad ':'
tc "with a file left" bad ': > "$d/x"'
apps_left() { ls -A "$1/Applications" 2>&1 | grep -v '^Sheepdog.app$'; } # home -> what else is in its Applications
apps() { # home what
  left=$(apps_left "$1"); [ -z "$left" ] && pass "$2: nothing but Sheepdog.app in Applications" || fail "$2: left in Applications: $left"
}
mkdir -p "$FX/ac/Applications/Sheepdog.app"; [ -z "$(apps_left "$FX/ac")" ] || fail "control: Applications with only Sheepdog.app is not clean"
mkdir "$FX/ac/Applications/.Sheepdog.app.stage"; [ -n "$(apps_left "$FX/ac")" ] && pass "control: a staging directory left in Applications is found" || fail "control: a staging directory left in Applications is not found"
xyz=${RC##*/}; xyz=${xyz#v}; xyz=${xyz%%-*}
c12=$(sed -n 's/^ *"commit": *"\([0-9a-f]\{12\}\).*/\1/p' "$RC/MANIFEST.json")
[ -n "$c12" ] || fail "no commit in the rc's manifest"
H=$FX/home; mkdir -p "$H"
A=$H/Applications/Sheepdog.app L=$H/.local/bin/sheepdog
ISHELL=/bin/zsh   # the first install as from macOS's default shell (the PATH hint's file is zsh's)
inst "$FX/srv" "$H"; r=$?; ISHELL=""
[ $r = 0 ] && grep -q "installed: sheepdog $xyz ($c12, macos, responsibility API: active)" "$FX/o" \
  && pass "install.sh installs the rc: sheepdog $xyz ($c12), the responsibility API active" || fail "install: rc=$r $(tail -2 "$FX/o" | tr '\n' ' ')"
grep -q "$H/.local/bin is not on your PATH" "$FX/o" && pass "the PATH hint names ~/.local/bin" || fail "no PATH hint naming $H/.local/bin"
grep -qF "echo 'export PATH=\"$H/.local/bin:\$PATH\"' >> ~/.zprofile" "$FX/o" && pass "with SHELL=/bin/zsh the hint's line goes into ~/.zprofile" || fail "the zsh hint: $(grep -F '.local/bin:' "$FX/o" | head -1)"
clean "the first install"
apps "$H" "the first install"
mkdir -p "$FX/x" && tar -xzf "$RC/$ARC" -C "$FX/x" || fail "cannot unpack the rc"
if [ -d "$A" ] && [ ! -L "$A" ] && python3 "$SD_ROOT/tests/lib/tree-same.py" "$A" "$FX/x/Sheepdog.app" > "$FX/d" 2>&1; then pass "the installed app is a directory, the release archive's bundle (paths, types, modes, content)"
else fail "the installed app: $(ls -ld "$A" 2>&1) $(head -3 "$FX/d" 2>/dev/null | tr '\n' ' ')"; fi
[ -L "$L" ] && [ "$(readlink "$L")" = "$A/Contents/MacOS/sheepdog" ] && pass "~/.local/bin/sheepdog links to the app's executable" || fail "the link: $(ls -l "$L" 2>&1)"
G="$SD_ROOT/scripts/lib/exec-guard.sh"
sh "$G" check "$L" 2> "$FX/o" && pass "the door allows the installed sheepdog" || { fail "the door refuses the installed sheepdog: $(cat "$FX/o")"; finish; }
J=$FX/jobhome; mkdir -p "$J"
job() { env -i PATH=/usr/bin:/bin HOME="$J" XDG_STATE_HOME="$J/x" SHEEPDOG_STATE="$J/s" TMPDIR="$J" "$@"; }
# the job's process: env in the background directly, so $! is the pid the door execs into (the
# door judges first, with several codesign calls: poll until it has)
env -i PATH=/usr/bin:/bin HOME="$J" XDG_STATE_HOME="$J/x" SHEEPDOG_STATE="$J/s" TMPDIR="$J" sh "$G" exec "$L" run -- /bin/sleep 5 >/dev/null 2>&1 & jp=$!
# the running file (lsof's txt entry, not argv[0]); wait while it is still env or the door's
# shell (/bin/sh runs bash here)
runs() { /usr/sbin/lsof -a -p "$1" -d txt -Fn 2>/dev/null | sed -n 's/^n//p' | head -1; }
i=0; f=$(runs $jp)
while case $f in */sh|*/bash|*/dash|*/zsh|*/env|'') true ;; *) false ;; esac && kill -0 $jp 2>/dev/null && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); f=$(runs $jp); done
fr=$(cd -P "$(dirname "$f")" 2>/dev/null && pwd -P)/$(basename "$f")
[ "$fr" = "$(cd -P "$A/Contents/MacOS" && pwd -P)/sheepdog" ] && pass "the job's process runs the installed app's executable (pid $jp)" || fail "pid $jp runs '$f'"
kill -TERM $jp 2>/dev/null; wait $jp 2>/dev/null
job sh "$G" exec "$L" run -- sh -c 'exit 7' >/dev/null 2>&1; r=$?
[ $r = 7 ] && pass "the installed sheepdog runs a job (exit 7 passed through)" || fail "the job's exit code: $r"
job sh "$SD_ROOT/scripts/smoke.sh" "$L" < /dev/null > "$FX/sm" 2>&1; r=$?
[ $r = 0 ] && pass "smoke.sh passes on the installed sheepdog" || fail "smoke: $(tr '\n' ' ' < "$FX/sm")"
# the upgrade: install again over the first
: > "$A/Contents/planted"
inst "$FX/srv" "$H"; r=$?
[ $r = 0 ] && [ ! -e "$A/Contents/planted" ] && [ -d "$A" ] && [ ! -L "$A" ] && pass "a second install replaces the app" || fail "second install: rc=$r planted=$(ls "$A/Contents/planted" 2>&1)"
[ "$(readlink "$L")" = "$A/Contents/MacOS/sheepdog" ] && [ -x "$L" ] && pass "the link still resolves" || fail "the link after the second install: $(ls -l "$L" 2>&1)"
apps "$H" "the second install"
clean "the second install"
# control of the PATH hint: ~/.local/bin on PATH, no hint
H3=$FX/home3; mkdir -p "$H3"
inst "$FX/srv" "$H3" "$H3/.local/bin"; r=$?
[ $r = 0 ] && ! grep -q 'is not on your PATH' "$FX/o" && pass "control: no PATH hint when ~/.local/bin is on PATH" || fail "PATH-hint control: rc=$r $(tail -1 "$FX/o")"
clean "the PATH-hint control install"
apps "$H3" "the PATH-hint control install"
# a failure after the app is in place says so: ~/.local/bin a file (mkdir -p fails), or a directory
# that cannot be written (ln fails)
for how in file dir; do
  H5=$FX/home5$how; mkdir -p "$H5/.local"
  if [ $how = file ]; then : > "$H5/.local/bin"; else mkdir "$H5/.local/bin" && chmod 555 "$H5/.local/bin"; fi
  inst "$FX/srv" "$H5"; r=$?
  [ $r = 1 ] && [ -d "$H5/Applications/Sheepdog.app" ] && grep -q "Sheepdog.app is installed in $H5/Applications, but $H5/.local/bin/sheepdog cannot be linked" "$FX/o" \
    && pass "a link that cannot be made (~/.local/bin a $how): exit 1, the app in place, and the message says so" || fail "cannot link ($how): rc=$r app=$(ls -d "$H5/Applications/Sheepdog.app" 2>&1) $(tail -1 "$FX/o")"
  clean "the cannot-link install ($how)"
  apps "$H5" "the cannot-link install ($how)"
  [ $how = dir ] && chmod 755 "$H5/.local/bin"
done
# neither move into Applications works (a stand-in mv refuses any move onto the app's path): the
# old app stays where it was moved aside, and the message says where
H6=$FX/home6; mkdir -p "$H6" "$FX/mvbin"; inst "$FX/srv" "$H6" >/dev/null 2>&1
ino=$(stat -f %i "$H6/Applications/Sheepdog.app" 2>/dev/null)   # the old app, by its inode
printf '#!/bin/sh\nfor a; do last=$a; done\ncase $last in */Applications/Sheepdog.app) exit 1 ;; esac\nexec /bin/mv "$@"\n' > "$FX/mvbin/mv"; chmod 755 "$FX/mvbin/mv"
inst "$FX/srv" "$H6" "$FX/mvbin"; r=$?
k=$(sed -n 's/.*nor the old one back; the old one is at \(.*\)$/\1/p' "$FX/o")
[ $r = 1 ] && [ -n "$k" ] && [ -n "$ino" ] && [ "$(stat -f %i "$k" 2>/dev/null)" = "$ino" ] && [ ! -e "$H6/Applications/Sheepdog.app" ] \
  && pass "neither move works: exit 1, the old app kept at the path the message names" || fail "both moves fail: rc=$r kept='$k' $(tail -1 "$FX/o")"
# refused: nothing installed, nothing left
refused() { # home what reason
  [ $r = 1 ] && grep -q "$3" "$FX/o" && [ ! -e "$1/.local/bin/sheepdog" ] && [ ! -L "$1/.local/bin/sheepdog" ] && [ -z "$(ls -A "$1/Applications" 2>/dev/null)" ] \
    && pass "$2: refused ($3), nothing installed or left in Applications" || fail "$2: rc=$r $(tail -1 "$FX/o"); Applications: $(ls -A "$1/Applications" 2>/dev/null | tr '\n' ' ')"
  clean "$2"
}
H4=$FX/home4; mkdir -p "$H4"
inst "$FX/srvt" "$H4"; r=$?
refused "$H4" "a tampered rc copy" "Developer ID signature"
H2=$FX/home2; mkdir -p "$H2"
inst "$FX/srvc" "$H2"; r=$?
refused "$H2" "the control (only a marked control can show this; rc.1's shares rc.1's notarized CDHash)" "Gatekeeper rejects"
finish
