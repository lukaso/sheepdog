#!/bin/sh
# PHASE3.md S3, the Mac happy path, against the operator's rc output (SD_RC_DIR; read only: a copy
# is served on 127.0.0.1): the release's own install.sh (rendered by the build) with a temp HOME.
#   - the install: Sheepdog.app in ~/Applications, byte for byte the release archive's bundle;
#     ~/.local/bin/sheepdog links to its executable; the PATH hint when ~/.local/bin is not on PATH;
#   - the installed sheepdog runs a job (its exit code passes through) and passes scripts/smoke.sh
#     (cell 1, cell 3 and its control), all of its state under the temp HOME;
#   - a second install over the first replaces the app (a file planted in the old one is gone),
#     the link still resolves, and no staging directory or old app is left;
#   - the control (SD_RC_CONTROL_DIR) is refused by Gatekeeper and nothing is installed (only a
#     control that carries the control marker can show this: rc.1's shares rc.1's notarized CDHash).
# The executable runs only after the door allows it (it is Developer ID signed).
set -u
. "$(dirname "$0")/../dist/lib.sh"
[ "$(uname -s)" = Darwin ] || { echo "FAIL: the rc leg needs macOS"; exit 1; }
fx_dir
RC=$SD_RC_DIR CT=$SD_RC_CONTROL_DIR
mkdir -p "$FX/srv" "$FX/srvc" && cp -R "$RC/." "$FX/srv/" && cp -R "$CT/." "$FX/srvc/" || { fail "cannot copy the rc"; finish; }
serve() { # dir -> port; the server's pid in $SPS
  p=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
  (cd "$1" && exec python3 -m http.server "$p" --bind 127.0.0.1) > "$1.log" 2>&1 & SPS="$SPS $!"
  i=0; until curl -fs "http://127.0.0.1:$p/" >/dev/null 2>&1 || [ $i -gt 50 ]; do sleep 0.1; i=$((i + 1)); done
  echo "$p" > "$1.port"
}
SPS=""; trap 'for s in $SPS; do kill $s 2>/dev/null; done; rm -rf "$FX"' EXIT
serve "$FX/srv"; serve "$FX/srvc"
H=$FX/home; mkdir -p "$H"
inst() { # dir-served HOME -> rc; output in $FX/o
  env -i PATH=/usr/bin:/bin:/usr/sbin HOME="$2" SHEEPDOG_INSTALL_BASE="http://127.0.0.1:$(cat "$1.port")" sh "$1/install.sh" > "$FX/o" 2>&1
}
A=$H/Applications/Sheepdog.app L=$H/.local/bin/sheepdog
inst "$FX/srv" "$H"; r=$?
[ $r = 0 ] && grep -q 'installed: sheepdog' "$FX/o" && pass "install.sh installs the rc" || fail "install: rc=$r $(tail -2 "$FX/o" | tr '\n' ' ')"
grep -q 'is not on your PATH' "$FX/o" && pass "the PATH hint" || fail "no PATH hint"
mkdir -p "$FX/x" && tar -xzf "$RC/sheepdog-macos-universal.tar.gz" -C "$FX/x" || fail "cannot unpack the rc"
diff -r -q "$A" "$FX/x/Sheepdog.app" > "$FX/d" 2>&1 && pass "the installed app is the release archive's bundle" || fail "the installed app differs: $(head -3 "$FX/d" | tr '\n' ' ')"
[ -L "$L" ] && [ "$(readlink "$L")" = "$A/Contents/MacOS/sheepdog" ] && pass "~/.local/bin/sheepdog links to the app's executable" || fail "the link: $(ls -l "$L" 2>&1)"
G="$SD_ROOT/scripts/lib/exec-guard.sh"
sh "$G" check "$L" 2> "$FX/o" && pass "the door allows the installed sheepdog" || { fail "the door refuses the installed sheepdog: $(cat "$FX/o")"; finish; }
J=$FX/jobhome; mkdir -p "$J"
job() { env -i PATH=/usr/bin:/bin HOME="$J" XDG_STATE_HOME="$J/x" SHEEPDOG_STATE="$J/s" TMPDIR="$J" "$@"; }
job sh "$G" exec "$L" run -- sh -c 'exit 7' >/dev/null 2>&1; r=$?
[ $r = 7 ] && pass "the installed sheepdog runs a job (exit 7 passed through)" || fail "the job's exit code: $r"
job sh "$SD_ROOT/scripts/smoke.sh" "$L" < /dev/null > "$FX/sm" 2>&1; r=$?
[ $r = 0 ] && pass "smoke.sh passes on the installed sheepdog" || fail "smoke: $(tr '\n' ' ' < "$FX/sm")"
# the upgrade: install again over the first
: > "$A/Contents/planted"
inst "$FX/srv" "$H"; r=$?
[ $r = 0 ] && [ ! -e "$A/Contents/planted" ] && pass "a second install replaces the app" || fail "second install: rc=$r planted=$(ls "$A/Contents/planted" 2>&1)"
[ "$(readlink "$L")" = "$A/Contents/MacOS/sheepdog" ] && [ -x "$L" ] && pass "the link still resolves" || fail "the link after the second install: $(ls -l "$L" 2>&1)"
left=$(ls -A "$H/Applications" | grep -v '^Sheepdog.app$')
[ -z "$left" ] && pass "no staging directory or old app left" || fail "left in Applications: $left"
# the control: refused by Gatekeeper, nothing installed
H2=$FX/home2; mkdir -p "$H2"
inst "$FX/srvc" "$H2"; r=$?
if [ $r = 1 ] && grep -q 'Gatekeeper rejects' "$FX/o" && [ ! -e "$H2/Applications/Sheepdog.app" ] && [ ! -e "$H2/.local/bin/sheepdog" ]; then pass "install.sh refuses the control (Gatekeeper), nothing installed"
else fail "install.sh and the control: rc=$r $(tail -1 "$FX/o") (rc.1's control shares rc.1's notarized CDHash: only a marked control can show this)"; fi
finish
