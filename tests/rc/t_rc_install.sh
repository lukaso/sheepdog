#!/bin/sh
# PHASE3.md S3, the Mac happy path, against the operator's rc output (SD_RC_DIR; read only: the
# three files install.sh fetches are copied and served on 127.0.0.1): the release's own install.sh
# (rendered by the build) with a temp HOME and a temp TMPDIR.
#   - the install: install.sh names the version, commit and an active responsibility API; the
#     app in ~/Applications is a directory (not a link), file for file the release archive's
#     bundle; ~/.local/bin/sheepdog links to its executable; the PATH hint names ~/.local/bin
#     (control: no hint when it is on PATH);
#   - the installed sheepdog, after the door: a job's process is the app's executable, its exit
#     code passes through, and scripts/smoke.sh passes, all of its state under a temp HOME;
#   - a second install over the first replaces the app (a planted file is gone), the link still
#     resolves, and nothing is left in Applications or TMPDIR;
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
SPS=""; trap 'for s in $SPS; do kill $s 2>/dev/null; done; rm -rf "$FX"' EXIT
serve() { # dir: its port into dir.port
  p=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
  (cd "$1" && exec python3 -m http.server "$p" --bind 127.0.0.1) > "$1.log" 2>&1 & SPS="$SPS $!"
  i=0; until curl -fs "http://127.0.0.1:$p/" >/dev/null 2>&1 || [ $i -gt 50 ]; do sleep 0.1; i=$((i + 1)); done
  echo "$p" > "$1.port"
}
serve "$FX/srv"; serve "$FX/srvc"; serve "$FX/srvt"
n=0
inst() { # served-dir HOME [extra PATH] -> rc; output in $FX/o; install.sh's TMPDIR in $TD (fresh;
  # its mtime before the install in $TDM)
  n=$((n + 1)); TD=$FX/tmp.$n; mkdir -p "$TD"; TDM=$(stat -f %Fm "$TD"); sleep 0.01
  env -i PATH="${3:+$3:}/usr/bin:/bin:/usr/sbin" HOME="$2" TMPDIR="$TD" SHEEPDOG_INSTALL_BASE="http://127.0.0.1:$(cat "$1.port")" sh "$1/install.sh" > "$FX/o" 2>&1
}
clean() { # what -> install.sh used TMPDIR (its mtime moved) and left nothing in it
  [ "$(stat -f %Fm "$TD")" != "$TDM" ] && [ -z "$(ls -A "$TD")" ] && pass "$1: install.sh used TMPDIR and left nothing in it" \
    || fail "$1: TMPDIR mtime $TDM -> $(stat -f %Fm "$TD"), left: $(ls -A "$TD" | tr '\n' ' ')"
}
xyz=${RC##*/}; xyz=${xyz#v}; xyz=${xyz%%-*}
c12=$(sed -n 's/^ *"commit": *"\([0-9a-f]\{12\}\).*/\1/p' "$RC/MANIFEST.json")
[ -n "$c12" ] || fail "no commit in the rc's manifest"
H=$FX/home; mkdir -p "$H"
A=$H/Applications/Sheepdog.app L=$H/.local/bin/sheepdog
inst "$FX/srv" "$H"; r=$?
[ $r = 0 ] && grep -q "installed: sheepdog $xyz ($c12, macos, responsibility API: active)" "$FX/o" \
  && pass "install.sh installs the rc: sheepdog $xyz ($c12), the responsibility API active" || fail "install: rc=$r $(tail -2 "$FX/o" | tr '\n' ' ')"
grep -q "$H/.local/bin is not on your PATH" "$FX/o" && pass "the PATH hint names ~/.local/bin" || fail "no PATH hint naming $H/.local/bin"
clean "the first install"
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
while case $f in */sh|*/bash|*/dash|*/env|'') true ;; *) false ;; esac && kill -0 $jp 2>/dev/null && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); f=$(runs $jp); done
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
left=$(ls -A "$H/Applications" | grep -v '^Sheepdog.app$')
[ -z "$left" ] && pass "no staging directory or old app left in Applications" || fail "left in Applications: $left"
clean "the second install"
# control of the PATH hint: ~/.local/bin on PATH, no hint
H3=$FX/home3; mkdir -p "$H3"
inst "$FX/srv" "$H3" "$H3/.local/bin"; r=$?
[ $r = 0 ] && ! grep -q 'is not on your PATH' "$FX/o" && pass "control: no PATH hint when ~/.local/bin is on PATH" || fail "PATH-hint control: rc=$r $(tail -1 "$FX/o")"
clean "the PATH-hint control install"
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
