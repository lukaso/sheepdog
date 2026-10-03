#!/bin/sh
# scripts/s7-clean-user.sh, the helper of S7's clean-user leg (PHASE3.md S7), on fixtures: no
# network, no release ID, nothing installed. Its functions are read in library mode (SD_S7_LIB=1:
# functions only, no subcommand runs).
#   - the wall: its pure function both ways; every test-user subcommand run as this (admin) user is
#     refused with exit 3 and writes nothing, each set up so that without the wall it would meet a
#     second refusal (exit 1) before any removal or network use;
#   - prep: the layout (the door as door/lib/exec-guard.sh beside door/release.conf, the npm
#     packages in npm/, results/ at 1777, all readable), its refusals (an existing path, a planted
#     link, a tampered archive, a sheepdog.rb one byte off its rendering, a control directory: the
#     manifest's control or mode, another tag, a marked bundle); a release.conf planted beside the
#     directory is never read by the copied door (control: a flat copy of the door reads it);
#   - brew-after: the same listing passes; a sheepdog entry, a tap named s7 or a changed byte in
#     the operator's Sheepdog files fails; another entry changed passes, printed; another prefix is
#     refused;
#   - the cask's local copy: only the url line changes; a cask with two url lines is refused;
#   - a read's classification: "Operation not permitted" is denied, exit 0 allowed, a missing
#     directory neither, a status line without responsibility tracking or with a degradation refused;
#   - the Launch Services reading: no record or one at this bundle passes; one elsewhere or two stop;
#     another bundle ID is not counted;
#   - uninstall: lsregister -u before the removal, the state kept while a record is left, and run
#     again it finishes; the channel state (one at a time) and finish refused while one is installed.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
SC="$SD_ROOT/scripts/s7-clean-user.sh"
[ -f "$SC" ] || { fail "no scripts/s7-clean-user.sh"; finish; }
case " $(/usr/bin/id -Gn) " in *" admin "*) ;; *) fail "this cell needs an admin user (the operator's account)"; finish ;; esac
lib() { ( SD_S7_LIB=1; . "$SC"; "$@" ); }
tree() { (cd "$1" && find . -print | LC_ALL=C sort | while IFS= read -r f; do printf '%s %s\n' "$f" "$(/usr/bin/stat -f '%Sp %z' "$f")"; done); }

# --- the wall
w() { # expect groups uid owner role
  r=$(lib s7_wall "$2" "$3" "$4" "$5" 2>&1); rc=$?
  if [ "$1" = allow ]; then [ $rc = 0 ] && pass "wall allows: $5 ($2, uid $3, owner '$4')" || fail "wall refuses $5 ($2, uid $3, owner '$4'): $r"
  else [ $rc != 0 ] && pass "wall refuses: $5 ($2, uid $3, owner '$4')" || fail "wall allows $5 ($2, uid $3, owner '$4')"; fi
}
w allow "staff admin everyone" 501 "" operator
w refuse "staff everyone" 502 "" operator
w allow "staff everyone" 502 501 user
w refuse "staff admin everyone" 501 502 user
w refuse "staff everyone" 502 "" user
w refuse "staff everyone" 501 501 user
# the wall's inputs, read for a dir this user owns, one root owns, a missing one and a link
wi() { lib s7_wall_inputs "$1" | sed -n "$2p"; }
[ "$(wi "$FX" 1)" = "$(/usr/bin/id -Gn)" ] && [ "$(wi "$FX" 2)" = "$(/usr/bin/id -u)" ] && [ "$(wi "$FX" 3)" = "$(/usr/bin/id -u)" ] \
  && pass "the wall's inputs: this user's groups and uid, and the owner of a dir it owns" || fail "the wall's inputs for $FX: $(lib s7_wall_inputs "$FX" | tr '\n' '|')"
[ "$(wi /private/tmp 3)" = 0 ] && pass "the wall's inputs: /private/tmp's owner is root (0)" || fail "the wall's inputs: /private/tmp's owner read as '$(wi /private/tmp 3)'"
ln -s "$FX" "$FX/wlink"
[ "$(lib s7_wall_inputs "$FX/none" | wc -l | tr -d ' ')" = 3 ] && [ -z "$(wi "$FX/none" 3)" ] && [ -z "$(wi "$FX/wlink" 3)" ] \
  && pass "the wall's inputs: no owner for a missing dir or a link" || fail "the wall's inputs: a missing dir or a link gave an owner"
( SD_S7_LIB=1; . "$SC"; s7_walled user /private/tmp ) > /dev/null 2>&1; rc=$?
[ $rc = 3 ] && pass "the wall refuses this admin user's test-user step even where root owns the dir (only the groups read refuses it)" || fail "s7_walled user /private/tmp: rc=$rc"
n=0
asop() { # what cmd...: run as this admin user in a temp HOME; the wall must refuse (3) and nothing change
  n=$((n + 1)); h=$FX/op$n
  t0=$(tree "$h")
  env HOME="$h" TMPDIR="$FX/opt$n" PATH=/usr/bin:/bin sh "$SC" "$@" > "$FX/o" 2>&1; rc=$?
  t1=$(tree "$h")
  [ $rc = 3 ] && [ "$t0" = "$t1" ] && pass "as the operator, '$*' is refused by the wall (exit 3) and writes nothing" \
    || fail "as the operator, '$*': rc=$rc, home changed: $([ "$t0" = "$t1" ] && echo no || echo yes): $(head -2 "$FX/o" | tr '\n' ' ')"
}
mkdir -p "$FX/op1/homebrew" "$FX/opt1"; asop tools                 # second refusal: ~/homebrew exists
mkdir -p "$FX/op2" "$FX/opt2"; asop channel npm                     # second refusal: no ~/s7-env.sh
mkdir -p "$FX/op3" "$FX/opt3"; asop uninstall npm                   # second refusal: nothing installed
mkdir -p "$FX/op4" "$FX/opt4"; echo npm > "$FX/op4/.s7-channel"; asop finish   # second refusal: a channel installed

# --- a fixture rc directory (a stand-in bundle with a non-release ID)
V=0.1.0-rc.9 TAG=v0.1.0-rc.9
mkrc() { # dir [control-marker]
  mkdir -p "$1" "$FX/b.$$"; rm -rf "$FX/b.$$"/*
  fx_bundle "$FX/b.$$" com.example.sds7
  [ -n "${2:-}" ] && /usr/bin/plutil -insert SheepdogControlBuild -bool true "$FX/b.$$/Sheepdog.app/Contents/Info.plist"
  "$SD_ROOT/scripts/lib/archive.sh" make "$FX/b.$$/Sheepdog.app" "$1/sheepdog-macos-universal.tar.gz" || return 1
  printf '#!/bin/sh\n' > "$1/sheepdog-linux-aarch64"; printf '#!/bin/sh\n#x\n' > "$1/sheepdog-linux-x86_64"; printf '#!/bin/sh\n' > "$1/install.sh"
  sh "$SD_ROOT/scripts/lib/npm-pack.sh" "$V" "$1/sheepdog-macos-universal.tar.gz" "$1/sheepdog-linux-aarch64" "$1/sheepdog-linux-x86_64" "$SD_ROOT/npm/sheepdog/bin/sheepdog" "$1" > /dev/null || return 1
  sh "$SD_ROOT/scripts/lib/render-cask.sh" "$V" "$(shasum -a 256 "$1/sheepdog-macos-universal.tar.gz" | cut -d' ' -f1)" "$1/sheepdog.rb" || return 1
  (cd "$1" && shasum -a 256 sheepdog-macos-universal.tar.gz sheepdog-linux-aarch64 sheepdog-linux-x86_64 install.sh > SHA256SUMS)
  python3 - "$1" "$TAG" <<'PY'
import sys, os, json, hashlib
d, tag = sys.argv[1:3]
names = ["sheepdog-macos-universal.tar.gz", "sheepdog-linux-aarch64", "sheepdog-linux-x86_64", "install.sh"] + sorted(f for f in os.listdir(d) if f.endswith(".tgz"))
files = [{"name": n, "sha256": hashlib.sha256(open(os.path.join(d, n), "rb").read()).hexdigest()} for n in names]
json.dump({"v": 1, "tag": tag, "commit": "0123456789abcdef0123456789abcdef01234567", "mode": "signed", "control": False, "files": files}, open(os.path.join(d, "MANIFEST.json"), "w"))
PY
}
RC=$FX/rel/$TAG; mkrc "$RC" || { fail "the fixture rc"; finish; }
B=$FX/brew; mkdir -p "$B/Caskroom/firefox" "$B/bin" "$B/Library/Taps/homebrew/homebrew-core"; ln -s ../Cellar/jq/1/bin/jq "$B/bin/jq"
OH=$FX/oph; mkdir -p "$OH/Applications" "$OH/.local/bin"; fx_bundle "$OH/Applications" com.example.sds7op; ln -s "$OH/Applications/Sheepdog.app/Contents/MacOS/sheepdog" "$OH/.local/bin/sheepdog"
prep() { env HOME="$OH" SD_S7_DIR="$1" SD_S7_BREW="${3:-$B}" PATH=/usr/bin:/bin:/usr/sbin:/sbin sh "$SC" prep "$2" > "$FX/o" 2>&1; }
mkdir -p "$FX/x"; D=$FX/x/sd-s7
if prep "$D" "$RC"; then pass "prep: makes the leg's directory"; else fail "prep: $(tr '\n' ' ' < "$FX/o")"; finish; fi
for f in install.sh SHA256SUMS MANIFEST.json sheepdog-macos-universal.tar.gz sheepdog.rb smoke.sh s7-clean-user.sh lib/static-registry.py lib/tree-same.py door/lib/exec-guard.sh door/release.conf brew-before.txt \
  npm/lukaso-sheepdog-$V.tgz npm/lukaso-sheepdog-darwin-universal-$V.tgz npm/lukaso-sheepdog-linux-arm64-$V.tgz npm/lukaso-sheepdog-linux-x64-$V.tgz; do
  [ -f "$D/$f" ] || fail "prep: no $f"
done
cmp -s "$D/door/release.conf" "$SD_ROOT/scripts/release.conf" && cmp -s "$D/door/lib/exec-guard.sh" "$SD_ROOT/scripts/lib/exec-guard.sh" && cmp -s "$D/lib/static-registry.py" "$SD_ROOT/scripts/lib/static-registry.py" \
  && cmp -s "$D/lib/tree-same.py" "$SD_ROOT/tests/lib/tree-same.py" && cmp -s "$D/smoke.sh" "$SD_ROOT/scripts/smoke.sh" && cmp -s "$D/s7-clean-user.sh" "$SC" \
  && cmp -s "$D/sheepdog-macos-universal.tar.gz" "$RC/sheepdog-macos-universal.tar.gz" && cmp -s "$D/npm/lukaso-sheepdog-$V.tgz" "$RC/lukaso-sheepdog-$V.tgz" \
  && pass "prep: the copies are the repo's and the rc's" || fail "prep: a copy differs"
[ "$(/usr/bin/stat -f %Mp%Lp "$D/results")" = 1777 ] && [ -d "$D/results" ] && pass "prep: results/ at 1777" || fail "prep: results/ is $(/usr/bin/stat -f %Lp "$D/results" 2>&1)"
nr=$(find "$D" ! -path "$D/results*" \( ! -perm -a+r -o \( -type d ! -perm -a+x \) -o -perm -o+w \) | head -3)
[ -z "$nr" ] && pass "prep: everything readable by all, nothing but results/ writable by others" || fail "prep: modes: $nr"
# the door reads only its own release.conf
printf ': > "%s/planted-read"\n' "$FX" > "$FX/x/release.conf"
/bin/sh -p "$D/door/lib/exec-guard.sh" check "$FX/x/release.conf" > /dev/null 2>&1
[ ! -e "$FX/planted-read" ] && pass "the copied door never reads a release.conf planted beside the leg's directory" || fail "the copied door read the planted release.conf"
cp "$D/door/lib/exec-guard.sh" "$D/flat-exec-guard.sh"; /bin/sh -p "$D/flat-exec-guard.sh" check "$FX/x/release.conf" > /dev/null 2>&1
[ -e "$FX/planted-read" ] && pass "control: a flat copy of the door reads it" || fail "control: a flat copy of the door did not read the planted file (the row proves nothing)"
rm -f "$FX/planted-read" "$D/flat-exec-guard.sh"
# prep's refusals
pr() { # what dir rc-dir: refused, nothing made
  if prep "$2" "$3"; then fail "prep accepted $1"; elif [ -e "$2" ] && [ ! -L "$2" ] && [ "$1" != "an existing directory" ]; then fail "prep refused $1 but made $2"; else pass "prep refuses $1"; fi
}
mkdir "$FX/x2"; mkdir "$FX/x2/sd-s7"; pr "an existing directory" "$FX/x2/sd-s7" "$RC"
mkdir "$FX/x3"; ln -s "$FX/x" "$FX/x3/sd-s7"; pr "a planted link" "$FX/x3/sd-s7" "$RC"
cp -R "$RC" "$FX/rel/t1"; mv "$FX/rel/t1" "$FX/rel2"; T2=$FX/rel2; mkdir -p "$FX/relx"; mv "$T2" "$FX/relx/$TAG"; T2=$FX/relx/$TAG
printf 'x' >> "$T2/sheepdog-macos-universal.tar.gz"; pr "a tampered archive" "$FX/x4" "$T2"
T3=$FX/rely/$TAG; mkdir -p "$FX/rely"; cp -R "$RC" "$T3"; printf '\n' >> "$T3/sheepdog.rb"; pr "a sheepdog.rb one byte off its rendering" "$FX/x5" "$T3"
mf() { # dir key value
  python3 -c 'import json,sys; p,k,v=sys.argv[1:4]; m=json.load(open(p)); m[k]=json.loads(v); json.dump(m,open(p,"w"))' "$1/MANIFEST.json" "$2" "$3"; }
T4=$FX/relz/$TAG; mkdir -p "$FX/relz"; cp -R "$RC" "$T4"; mf "$T4" control true; pr "a manifest marked control" "$FX/x6" "$T4"
T5=$FX/relw/$TAG; mkdir -p "$FX/relw"; cp -R "$RC" "$T5"; mf "$T5" mode '"control"'; pr "a manifest whose mode is not signed" "$FX/x7" "$T5"
T6=$FX/relv/v0.1.0-rc.8; mkdir -p "$FX/relv"; cp -R "$RC" "$T6"; pr "a directory named for another tag" "$FX/x8" "$T6"
T7=$FX/relu/$TAG; mkdir -p "$FX/relu"; mkrc "$T7" marked || fail "the marked fixture"; pr "an archive whose bundle carries the control marker" "$FX/x9" "$T7"

# --- brew-after
ba() { env HOME="$OH" SD_S7_DIR="$D" SD_S7_BREW="${1:-$B}" PATH=/usr/bin:/bin:/usr/sbin:/sbin sh "$SC" brew-after > "$FX/o" 2>&1; }
ba && pass "brew-after: the same listing passes" || fail "brew-after on the same listing: $(tr '\n' ' ' < "$FX/o")"
chk() { # what expect(pass|fail) setup undo
  eval "$3"; ba; rc=$?; eval "$4"
  case $2 in pass) [ $rc = 0 ] && grep -q . "$FX/o" && pass "brew-after: $1 passes, printed" || fail "brew-after: $1: rc=$rc" ;;
    fail) [ $rc = 1 ] && pass "brew-after: $1 fails" || fail "brew-after: $1: rc=$rc $(tr '\n' ' ' < "$FX/o")" ;; esac
}
chk "a sheepdog cask added" fail 'mkdir "$B/Caskroom/sheepdog"' 'rmdir "$B/Caskroom/sheepdog"'
chk "a sheepdog link added to bin" fail 'ln -s x "$B/bin/sheepdog"' 'rm "$B/bin/sheepdog"'
chk "a tap named s7" fail 'mkdir -p "$B/Library/Taps/s7/homebrew-local"' 'rm -r "$B/Library/Taps/s7"'
byte_add() { printf x >> "$OH/Applications/Sheepdog.app/Contents/Info.plist"; }
byte_del() { python3 -c 'import sys; p=sys.argv[1]; b=open(p,"rb").read(); open(p,"wb").write(b[:-1])' "$OH/Applications/Sheepdog.app/Contents/Info.plist"; }
chk "a changed byte in the operator's Sheepdog.app" fail byte_add byte_del
chk "the operator's link moved" fail 'rm "$OH/.local/bin/sheepdog"; ln -s /x "$OH/.local/bin/sheepdog"' 'rm "$OH/.local/bin/sheepdog"; ln -s "$OH/Applications/Sheepdog.app/Contents/MacOS/sheepdog" "$OH/.local/bin/sheepdog"'
chk "another entry added (bin/gh)" pass 'ln -s x "$B/bin/gh"' 'rm "$B/bin/gh"'
ln -s x "$B/bin/gh"; ba; rm "$B/bin/gh"; grep -q "information, not the leg's: > entry bin/gh -> x" "$FX/o" && pass "brew-after prints another change as information" || fail "brew-after printed no information line: $(tr '\n' ' ' < "$FX/o")"
chk "an empty tap directory named s7" fail 'mkdir -p "$B/Library/Taps/s7"' 'rmdir "$B/Library/Taps/s7"'
ba && pass "brew-after: back to the same listing" || fail "brew-after: the undo left a change: $(tr '\n' ' ' < "$FX/o")"
mkdir -p "$FX/brew2"; ba "$FX/brew2"; rc=$?; [ $rc != 0 ] && [ $rc != 1 ] && pass "brew-after refuses another prefix (rc $rc)" || fail "brew-after with another prefix: rc=$rc"

# --- the cask's local copy
L=$FX/local.rb
if lib s7_cask_local "$RC/sheepdog.rb" "file:///Users/Shared/sd-s7/sheepdog-macos-universal.tar.gz" "$L"; then
  d=$(diff "$RC/sheepdog.rb" "$L" | grep -c '^[<>]')
  [ "$d" = 2 ] && grep -q '^  url "file:///Users/Shared/sd-s7/sheepdog-macos-universal.tar.gz"$' "$L" && [ "$(grep -c '^  url ' "$RC/sheepdog.rb")" = 1 ] \
    && pass "the cask's local copy changes only its url line" || fail "the cask's local copy: $(diff "$RC/sheepdog.rb" "$L" | head -4 | tr '\n' ' ')"
else fail "s7_cask_local refused the shipped cask"; fi
{ cat "$RC/sheepdog.rb"; echo '  url "https://example.com/x.tar.gz"'; } > "$FX/two.rb"
lib s7_cask_local "$FX/two.rb" "file:///x" "$FX/two-local.rb" 2>/dev/null && fail "a cask with two url lines was accepted" || pass "a cask with two url lines is refused"

# --- a read's classification
mkdir -p "$FX/c"
st() { printf '{"v":1,"job":"j-1","root":"exited","code":%s,"tracking":"%s","degraded":%s,"error":null,"notes":[]}\n' "$1" "$2" "$3" > "$FX/c/s"; }
cl() { # what expect rc err-text
  printf '%s\n' "$4" > "$FX/c/e"; got=$(lib s7_classify "$3" "$FX/c/s" "$FX/c/e" /Users/t/Library/Safari)
  case $got in "$2"*) pass "classify: $1 -> $2" ;; *) fail "classify: $1 -> '$got', not $2" ;; esac
}
st 1 responsibility null; cl "Operation not permitted" denied 1 "ls: /Users/t/Library/Safari: Operation not permitted"
st 0 responsibility null; cl "exit 0" allowed 0 ""
st 1 responsibility null; cl "a missing directory" bad 1 "ls: /Users/t/Library/Safari: No such file or directory"
st 1 puniq null; cl "a status line without responsibility tracking" bad 1 "ls: /Users/t/Library/Safari: Operation not permitted"
st 0 responsibility '["privacy: x"]'; cl "a degraded status line" bad 0 ""
: > "$FX/c/s"; cl "no status line" bad 0 ""
st 1 responsibility null; cl "Operation not permitted on another path" bad 1 "ls: /Users/t/Library/Mail: Operation not permitted"
st 1 responsibility null; sed 's/"error":null/"error":"spawn failed"/' "$FX/c/s" > "$FX/c/s2" && mv "$FX/c/s2" "$FX/c/s"; cl "a status line with an error" bad 1 "ls: /Users/t/Library/Safari: Operation not permitted"

# --- the Launch Services reading
rec() { printf -- '--------------------------------------------------------------------------------\npath:                       %s (0x4ee0)\nname:                       Sheepdog\nidentifier:                 %s\n' "$1" "$2"; }
lsr() { # what expect(pass|stop) bundle dump-text
  printf '%s\n' "$4" > "$FX/dump"; lib s7_ls_ok "$FX/dump" "$3" > /dev/null; rc=$?
  case $2 in pass) [ $rc = 0 ] && pass "Launch Services: $1 passes" || fail "Launch Services: $1 stops" ;; stop) [ $rc != 0 ] && pass "Launch Services: $1 stops" || fail "Launch Services: $1 passes" ;; esac
}
BU=/Users/t/Applications/Sheepdog.app
lsr "no record" pass "$BU" "$(rec /x/Other.app com.example.other)"
lsr "one record at this bundle" pass "$BU" "$(rec "$BU" com.lukaso.sheepdog)"
lsr "one record elsewhere" stop "$BU" "$(rec /Users/t/npm-global/x/Sheepdog.app com.lukaso.sheepdog)"
lsr "two records" stop "$BU" "$(rec "$BU" com.lukaso.sheepdog; rec /Users/t/x/Sheepdog.app com.lukaso.sheepdog)"
lsr "another bundle ID beside this bundle" pass "$BU" "$(rec "$BU" com.lukaso.sheepdog; rec /Users/t/x/Sheepdog.app com.lukaso.sheepdog.test.fake17)"

# --- uninstall, and the channel state
UH=$FX/uh; mkdir -p "$UH/Applications" "$UH/.local/bin" "$FX/ub"
fx_bundle "$UH/Applications" com.example.sds7u; ln -s "$UH/Applications/Sheepdog.app/Contents/MacOS/sheepdog" "$UH/.local/bin/sheepdog"
cat > "$FX/ub/lsregister" <<SH
#!/bin/sh
case \$1 in
  -u) echo "u \$2 \$([ -d "\$2" ] && echo present || echo gone)" >> "$FX/ulog" ;;
  -dump) cat "$FX/udump" ;;
esac
SH
chmod 755 "$FX/ub/lsregister"
printf 'channel=install-sh\nentry=%s\nbundle=%s\n' "$UH/.local/bin/sheepdog" "$UH/Applications/Sheepdog.app" > "$UH/.s7-channel"
rec "$UH/Applications/Sheepdog.app" com.lukaso.sheepdog > "$FX/udump"   # the record survives the -u: the state must stay
un() { ( SD_S7_LIB=1; . "$SC"; S7_LSREG="$FX/ub/lsregister"; HOME="$UH"; s7_uninstall "$1" ) > "$FX/o" 2>&1; }
un install-sh; rc=$?
[ $rc != 0 ] && [ -f "$UH/.s7-channel" ] && pass "uninstall keeps the state while a record is left" || fail "uninstall with a record left: rc=$rc, state $(ls "$UH/.s7-channel" 2>&1)"
[ "$(head -1 "$FX/ulog" 2>/dev/null)" = "u $UH/Applications/Sheepdog.app present" ] && pass "uninstall unregisters the bundle while it still exists" || fail "uninstall's lsregister -u: '$(cat "$FX/ulog" 2>/dev/null)'"
[ ! -e "$UH/Applications/Sheepdog.app" ] && [ ! -e "$UH/.local/bin/sheepdog" ] && [ ! -L "$UH/.local/bin/sheepdog" ] && pass "uninstall install-sh removes the README's two paths" || fail "uninstall install-sh left: $(ls -A "$UH/Applications" "$UH/.local/bin" 2>&1 | tr '\n' ' ')"
: > "$FX/udump"; : > "$FX/ulog"
un install-sh; rc=$?
[ $rc = 0 ] && [ ! -e "$UH/.s7-channel" ] && [ ! -s "$FX/ulog" ] && pass "uninstall run again finishes it (no record, nothing left to unregister)" || fail "uninstall run again: rc=$rc, state $(ls "$UH/.s7-channel" 2>&1), log '$(cat "$FX/ulog")'"
# bundle gone, record left: a rerun unregisters the record again
mkdir -p "$UH/x"; printf 'channel=install-sh\nentry=\nbundle=%s\n' "$UH/x/Sheepdog.app" > "$UH/.s7-channel"
rec "$UH/x/Sheepdog.app" com.lukaso.sheepdog > "$FX/udump"; : > "$FX/ulog"
cat > "$FX/ub/lsregister" <<SH
#!/bin/sh
case \$1 in
  -u) echo "u \$2 \$([ -d "\$2" ] && echo present || echo gone)" >> "$FX/ulog"; : > "$FX/udump" ;;
  -dump) cat "$FX/udump" ;;
esac
SH
un install-sh; rc=$?
[ $rc = 0 ] && [ ! -e "$UH/.s7-channel" ] && grep -qxF "u $UH/x/Sheepdog.app gone" "$FX/ulog" \
  && pass "uninstall with the bundle gone and its record left unregisters the record and finishes" || fail "bundle gone, record left: rc=$rc, log '$(cat "$FX/ulog")', state $(ls "$UH/.s7-channel" 2>&1)"
# pnpm rm -g leaves the packages in pnpm's global virtual store (measured, pnpm 10.18.2): uninstall
# removes that copy too (a stand-in node that, like pnpm, removes nothing)
PH=$FX/ph; PB="$PH/pnpm/global/5/.pnpm/@lukaso+sheepdog-darwin-universal@0.1.0-rc.9/node_modules/@lukaso/sheepdog-darwin-universal"
mkdir -p "$PB" "$PH/pnpm/global/5/.pnpm/@lukaso+sheepdog@0.1.0-rc.9/node_modules/@lukaso/sheepdog/bin" "$PH/pnpm/global/5/.pnpm/other@1.0.0" "$PH/node/bin"
fx_bundle "$PB" com.example.sds7p; mkdir -p "$PH/pnpm/global/5/node_modules/@lukaso/sheepdog"
# the stand-in removes only the global link, as pnpm rm -g does
printf '#!/bin/sh\necho "node $*" >> "%s/plog"\nrm -rf "%s/pnpm/global/5/node_modules/@lukaso/sheepdog"\n' "$FX" "$PH" > "$PH/node/bin/node"; chmod 755 "$PH/node/bin/node"
printf 'channel=pnpm\nentry=\nbundle=%s\n' "$PB/Sheepdog.app" > "$PH/.s7-channel"; : > "$FX/udump"; : > "$FX/ulog"
( SD_S7_LIB=1; . "$SC"; S7_LSREG="$FX/ub/lsregister"; S7_DIR="$FX/nodir"; HOME="$PH"; s7_uninstall pnpm ) > "$FX/o" 2>&1; rc=$?
[ $rc = 0 ] && grep -q 'rm -g @lukaso/sheepdog' "$FX/plog" && [ ! -e "$PB" ] && ! ls -d "$PH"/pnpm/global/5/.pnpm/@lukaso+sheepdog* > /dev/null 2>&1 \
  && [ -d "$PH/pnpm/global/5/.pnpm/other@1.0.0" ] && [ ! -e "$PH/.s7-channel" ] \
  && pass "uninstall pnpm runs pnpm rm -g, then removes the copy pnpm leaves in its global store (and nothing else there)" \
  || fail "uninstall pnpm: rc=$rc $(tr '\n' ' ' < "$FX/o"); left: $(ls "$PH/pnpm/global/5/.pnpm" 2>&1 | tr '\n' ' ')"
# rerun after pnpm already removed its link: no second pnpm rm -g (pnpm refuses one), the copy still removed
mkdir -p "$PB"; fx_bundle "$PB" com.example.sds7p; printf 'channel=pnpm\nentry=\nbundle=%s\n' "$PB/Sheepdog.app" > "$PH/.s7-channel"; : > "$FX/plog"
( SD_S7_LIB=1; . "$SC"; S7_LSREG="$FX/ub/lsregister"; S7_DIR="$FX/nodir"; HOME="$PH"; s7_uninstall pnpm ) > "$FX/o" 2>&1; rc=$?
[ $rc = 0 ] && [ ! -s "$FX/plog" ] && [ ! -e "$PB" ] && [ ! -e "$PH/.s7-channel" ] \
  && pass "uninstall pnpm run again with pnpm's link gone: no pnpm call, the store copy removed" || fail "uninstall pnpm rerun: rc=$rc plog '$(cat "$FX/plog")' $(tr '\n' ' ' < "$FX/o")"
st7() { ( SD_S7_LIB=1; . "$SC"; HOME="$UH"; "$@" ) > /dev/null 2>&1; }
st7 s7_state_free && pass "the channel state is free after the uninstall" || fail "the channel state is not free"
printf 'channel=npm\nentry=\nbundle=/x\n' > "$UH/.s7-channel"
st7 s7_state_free && fail "a second channel would be allowed while npm is installed" || pass "a channel is refused while another is installed"
st7 s7_state_is cask && fail "uninstall cask allowed while npm is installed" || pass "uninstall of a channel that is not installed is refused"
st7 s7_state_is npm && pass "uninstall of the installed channel is allowed" || fail "uninstall npm refused while npm is installed"
st7 s7_finish_ok && fail "finish allowed while a channel is installed" || pass "finish is refused while a channel is installed"
rm "$UH/.s7-channel"; st7 s7_finish_ok && pass "finish is allowed with no channel installed" || fail "finish refused with no channel installed"
# --- a channel driven end to end with stand-ins (install-sh; no network but 127.0.0.1, no terminal,
# no real tccutil: the reads answer from a scenario list, the answers come from a file, tccutil,
# mdfind and lsregister are stand-ins, the bundle has a non-release ID and its executable is a
# stand-in sheepdog script that logs every run to $FX/ran)
CD=$FX/cd; mkdir -p "$CD/lib" "$CD/door/lib" "$CD/results" "$FX/cb/Sheepdog.app/Contents/MacOS" "$FX/tt"
fx_plist "$FX/cb/Sheepdog.app/Contents/Info.plist" com.example.sds7ch
cat > "$FX/cb/Sheepdog.app/Contents/MacOS/sheepdog" <<SH
#!/bin/sh
case \$1 in
  --version) echo version >> "$FX/ran"; echo "sheepdog 0.1.0 (0123456789ab, macos, responsibility API: active)"; exit 0 ;;
  run) k=\$(sed -n 1p "$FX/reads"); sed 1d "$FX/reads" > "$FX/reads.n"; mv "$FX/reads.n" "$FX/reads"; echo "read \$k" >> "$FX/ran"
    [ ! -e "$FX/blockclear" ] || mkdir -p "\$HOME/.s7-channel.new"
    case \$k in *+block) mkdir -p "\$HOME/.s7-channel.new"; k=\${k%+block} ;; esac   # this read blocks the state write after it
    for a; do d=\$a; done
    case \$k in
      denied) printf '{"v":1,"code":1,"tracking":"responsibility","degraded":null,"error":null}\n' >&3; echo "ls: \$d: Operation not permitted" >&2; exit 1 ;;
      allowed) printf '{"v":1,"code":0,"tracking":"responsibility","degraded":null,"error":null}\n' >&3; echo History.db; exit 0 ;;
      *) printf '{"v":1,"code":124,"tracking":"responsibility","degraded":["timeout"],"error":null}\n' >&3; exit 124 ;;
    esac ;;
esac
exit 2
SH
chmod 755 "$FX/cb/Sheepdog.app/Contents/MacOS/sheepdog"
"$SD_ROOT/scripts/lib/archive.sh" make "$FX/cb/Sheepdog.app" "$CD/sheepdog-macos-universal.tar.gz" || { fail "the channel fixture's archive"; finish; }
: > "$CD/SHA256SUMS"
printf '{"v":1,"tag":"v0.1.0-rc.9","commit":"0123456789abcdef0123456789abcdef01234567","mode":"signed","control":false,"files":[]}\n' > "$CD/MANIFEST.json"
cp "$SD_ROOT/scripts/lib/exec-guard.sh" "$CD/door/lib/exec-guard.sh"; cp "$SD_ROOT/scripts/release.conf" "$CD/door/release.conf"; cp "$SD_ROOT/tests/lib/tree-same.py" "$CD/lib/tree-same.py"
printf '#!/bin/sh\necho smoke >> "%s/ran"\n[ ! -e "%s/blockstate" ] || mkdir -p "$HOME/.s7-channel.new"\n' "$FX" "$FX" > "$CD/smoke.sh"
cat > "$CD/install.sh" <<SH
#!/bin/sh
set -e
mkdir -p "\$HOME/Applications" "\$HOME/.local/bin"
tar -xzf "$CD/sheepdog-macos-universal.tar.gz" -C "\$HOME/Applications"
[ ! -e "$FX/tamper" ] || printf x >> "\$HOME/Applications/Sheepdog.app/Contents/Info.plist"
ln -sf "\$HOME/Applications/Sheepdog.app/Contents/MacOS/sheepdog" "\$HOME/.local/bin/sheepdog"
SH
printf '#!/bin/sh\necho "tccutil $*" >> "%s/tcclog"\n' "$FX" > "$FX/tccutil"; chmod 755 "$FX/tccutil"
: > "$FX/udump"
hn=0
mkhome() { hn=$((hn + 1)); CH=$FX/ch$hn; mkdir -p "$CH/Library/Safari"; ( SD_S7_LIB=1; . "$SC"; s7_env ) > "$CH/s7-env.sh"; }
inlib() { # home answers-file function args...: a library function with every outside effect replaced
  i_h=$1 i_a=$2; shift 2
  ( SD_S7_LIB=1; . "$SC"; s7_walled() { :; }; S7_DIR=$CD; S7_LSREG=$FX/ub/lsregister; S7_TCCUTIL=$FX/tccutil; S7_MDFIND=/usr/bin/true
    S7_CURL=/usr/bin/false; S7_TTY=$i_a; S7_TTY_OUT=/dev/null; HOME=$i_h; TMPDIR=$FX/tt; export HOME TMPDIR; "$@" ) < /dev/null > "$FX/o" 2>&1
}
drive() { # answers reads...: channel install-sh in a fresh home ($CH); rc in $rc
  d_a=$1; shift; printf '%s\n' "$@" > "$FX/reads"; : > "$FX/ran"; : > "$FX/tcclog"; rm -f "$CD/results/install-sh.txt"
  mkhome; printf "$d_a" > "$FX/answers"; inlib "$CH" "$FX/answers" s7_cmd_channel install-sh; rc=$?
}
res() { cat "$CD/results/install-sh.txt" 2>/dev/null; }
# the bundle is judged before it ever runs: a tampered one stops the channel with nothing run
: > "$FX/tamper"; drive 'n\nn\ny\ny\nn\n' denied allowed denied; rm -f "$FX/tamper"
[ $rc = 1 ] && res | grep -q "STOPPED at the bundle is not the rc archive's" && [ ! -s "$FX/ran" ] && grep -q '^bundle=.*/Sheepdog.app$' "$CH/.s7-channel" \
  && pass "channel: a bundle that is not the rc archive's stops it before its executable ever runs (the state names the bundle)" || fail "channel, tampered: rc=$rc ran '$(tr '\n' ' ' < "$FX/ran")' $(res | tail -1)"
# the whole channel: judged, run, smoke, the grant, the reset taken
drive 'n\nn\ny\ny\nn\n' denied allowed denied
[ $rc = 0 ] && [ "$(tr '\n' ' ' < "$FX/ran")" = "version smoke read denied read allowed read denied " ] && res | grep -q '(g) the reset took' \
  && grep -qx 'tccutil reset SystemPolicyAllFiles com.lukaso.sheepdog' "$FX/tcclog" && ! grep -q '^granted=' "$CH/.s7-channel" \
  && pass "channel: version, smoke, then denied / allowed / reset / denied; the grant flag cleared" || fail "channel, whole: rc=$rc ran '$(tr '\n' ' ' < "$FX/ran")' $(res | tail -2 | tr '\n' ' ') $(tr '\n' ' ' < "$FX/o" | cut -c1-200)"
# (g) allowed: the reset did not take; the removal is recorded and must end in a denied read
drive 'n\nn\ny\ny\ny\ny\nn\n' denied allowed allowed denied
[ $rc = 0 ] && res | grep -q '(g) the reset did not take' && res | grep -qx '  (g) denied after the removal, with − here' \
  && pass "channel: a reset that did not take is recorded, then the removal and a denied read" || fail "channel, (g) allowed: rc=$rc $(res | tail -3 | tr '\n' ' ')"
# (g) bad: never recorded as "did not take"; the grant flag stays
drive 'n\nn\ny\ny\n' denied allowed bad bad bad
[ $rc = 1 ] && res | grep -q 'STOPPED at (g)' && ! res | grep -q 'did not take' && grep -qx 'granted=yes' "$CH/.s7-channel" \
  && pass "channel: an unclear read after the reset stops it, recorded as unclear, never as 'did not take'; the grant flag stays" || fail "channel, (g) bad: rc=$rc $(res | tail -2 | tr '\n' ' ') state $(tr '\n' ' ' < "$CH/.s7-channel")"
GH=$CH
printf 'bad\n' > "$FX/reads"; inlib "$GH" /dev/null s7_uninstall install-sh; rc=$?
[ $rc = 1 ] && [ -e "$GH/Applications/Sheepdog.app" ] && grep -qx 'granted=yes' "$GH/.s7-channel" && pass "uninstall refuses while a grant may remain (the read is not denied), the bundle kept" || fail "uninstall with a grant left: rc=$rc $(tr '\n' ' ' < "$FX/o")"
printf 'denied\n' > "$FX/reads"; inlib "$GH" /dev/null s7_uninstall install-sh; rc=$?
[ $rc = 0 ] && [ ! -e "$GH/.s7-channel" ] && [ ! -e "$GH/Applications/Sheepdog.app" ] && pass "uninstall goes on once a read is denied" || fail "uninstall after a denied read: rc=$rc $(tr '\n' ' ' < "$FX/o")"
# (e) not allowed: stopped with the grant flag set; finish and uninstall refuse until a denied read
drive 'n\nn\ny\ny\n' denied denied
[ $rc = 1 ] && res | grep -q 'STOPPED at (e)' && grep -qx 'granted=yes' "$CH/.s7-channel" && pass "channel: a read not allowed after the grant stops it with the grant flag set" || fail "channel, (e): rc=$rc $(res | tail -1)"
inlib "$CH" /dev/null s7_cmd_finish; rc=$?; [ $rc = 1 ] && [ ! -e "$CH/s7-smoke.sh" ] && pass "finish refuses while a channel is installed" || fail "finish with a channel installed: rc=$rc"
printf 'allowed\n' > "$FX/reads"; inlib "$CH" /dev/null s7_uninstall install-sh; rc=$?
[ $rc = 1 ] && [ -e "$CH/.s7-channel" ] && pass "uninstall refuses while the read is still allowed" || fail "uninstall with the read allowed: rc=$rc"
# no answer: a stop, never a "n"
drive 'n\n' denied allowed denied
[ $rc = 1 ] && res | grep -q 'STOPPED at (c): no answer' && [ "$(res | grep -c -- '-> n')" = 1 ] && pass "channel: no answer is a stop, not a 'n'" || fail "channel, no answer: rc=$rc $(res | tail -2 | tr '\n' ' ')"
# channel refused while one is installed: nothing written
mkhome; printf 'channel=npm\nentry=\nbundle=/x\n' > "$CH/.s7-channel"; t0=$(tree "$CH"); : > "$CD/results/install-sh.txt"
inlib "$CH" /dev/null s7_cmd_channel install-sh; rc=$?
[ $rc = 1 ] && [ "$t0" = "$(tree "$CH")" ] && [ ! -s "$CD/results/install-sh.txt" ] && pass "channel refuses while another is installed, and writes nothing" || fail "channel with npm installed: rc=$rc"
inlib "$CH" /dev/null s7_uninstall cask; rc=$?; [ $rc = 1 ] && [ -e "$CH/.s7-channel" ] && pass "uninstall refuses a channel that is not the installed one" || fail "uninstall cask with npm installed: rc=$rc"
# uninstall loads the leg's environment (npm's prefix, Homebrew's temp)
mkhome; mkdir -p "$CH/npm-global/lib/node_modules/@lukaso/sheepdog/node_modules/x/Sheepdog.app" "$CH/node/bin"
printf '#!/bin/sh\necho "NPM_CONFIG_PREFIX=$NPM_CONFIG_PREFIX $*" >> "%s/elog"\nrm -rf "%s/npm-global/lib/node_modules/@lukaso"\n' "$FX" "$CH" > "$CH/node/bin/node"; chmod 755 "$CH/node/bin/node"
printf 'channel=npm\nentry=\nbundle=%s\n' "$CH/npm-global/lib/node_modules/@lukaso/sheepdog/node_modules/x/Sheepdog.app" > "$CH/.s7-channel"; : > "$FX/elog"
inlib "$CH" /dev/null s7_uninstall npm; rc=$?
[ $rc = 0 ] && grep -q "^NPM_CONFIG_PREFIX=$CH/npm-global .*rm -g @lukaso/sheepdog" "$FX/elog" && pass "uninstall npm runs npm rm -g with the leg's prefix" || fail "uninstall npm: rc=$rc elog '$(cat "$FX/elog")' $(tr '\n' ' ' < "$FX/o")"
mkhome; mkdir -p "$CH/homebrew/Caskroom/sheepdog" "$CH/homebrew/bin" "$CH/Applications/Sheepdog.app"
printf '#!/bin/sh\necho "HOMEBREW_TEMP=$HOMEBREW_TEMP $*" >> "%s/elog"\nrm -rf "%s/homebrew/Caskroom/sheepdog" "%s/Applications/Sheepdog.app"\n' "$FX" "$CH" "$CH" > "$CH/homebrew/bin/brew"; chmod 755 "$CH/homebrew/bin/brew"
printf 'channel=cask\nentry=\nbundle=%s\n' "$CH/Applications/Sheepdog.app" > "$CH/.s7-channel"; : > "$FX/elog"
inlib "$CH" /dev/null s7_uninstall cask; rc=$?
[ $rc = 0 ] && grep -q "^HOMEBREW_TEMP=$FX/tt uninstall --cask s7/local/sheepdog" "$FX/elog" && pass "uninstall cask runs brew uninstall with the leg's Homebrew temp" || fail "uninstall cask: rc=$rc elog '$(cat "$FX/elog")'"
# a brew that prints a deprecation while it uninstalls (as Homebrew 7.0.7 did for rc.2's cask): the
# uninstall finishes (the state is cleared), then exits 1 naming it, and the results record it
mkdir -p "$CH/homebrew/Caskroom/sheepdog" "$CH/Applications/Sheepdog.app"
printf '#!/bin/sh
echo "HOMEBREW_TEMP=$HOMEBREW_TEMP $*" >> "%s/elog"
echo "Warning: depends_on macos: \\">= :monterey\\" is deprecated!"
rm -rf "%s/homebrew/Caskroom/sheepdog" "%s/Applications/Sheepdog.app"
' "$FX" "$CH" "$CH" > "$CH/homebrew/bin/brew"
printf 'channel=cask\nentry=\nbundle=%s\n' "$CH/Applications/Sheepdog.app" > "$CH/.s7-channel"; : > "$FX/elog"; rm -f "$CD/results/cask.txt"
inlib "$CH" /dev/null s7_uninstall cask; rc=$?
[ $rc = 1 ] && [ ! -e "$CH/.s7-channel" ] && grep -q 'deprecation' "$FX/o" && grep -q 'deprecated' "$CD/results/cask.txt" 2>/dev/null \
  && pass "a deprecation printed by brew uninstall: the uninstall finishes, then exit 1 naming it, recorded" || fail "uninstall deprecation: rc=$rc state=$(cat "$CH/.s7-channel" 2>/dev/null | head -1) $(tr '\n' ' ' < "$FX/o")"
( SD_S7_LIB=1; . "$SC"; printf 'Warning: X is Deprecated\n' > "$FX/dep"; printf '==> Installing Cask sheepdog\n' > "$FX/nodep"
  ! s7_deprecations "$FX/dep" && s7_deprecations "$FX/nodep" ) && pass "s7_deprecations: a deprecation found (either case), a clean output passes" || fail "s7_deprecations"
# tools: a run that did not finish, and one that did, are refused before any download (curl is false)
mkhome; rm "$CH/s7-env.sh"; mkdir "$CH/homebrew"; inlib "$CH" /dev/null s7_cmd_tools; rc=$?
[ $rc = 1 ] && grep -q 'did not finish' "$FX/o" && grep -q 'homebrew' "$FX/o" && pass "tools after an unfinished run names what to remove" || fail "tools, unfinished: rc=$rc $(tr '\n' ' ' < "$FX/o")"
mkhome; inlib "$CH" /dev/null s7_cmd_tools; rc=$?
[ $rc = 1 ] && grep -q 'tools has run' "$FX/o" && pass "tools after a finished run refuses" || fail "tools, finished: rc=$rc $(tr '\n' ' ' < "$FX/o")"
mkhome; rm "$CH/s7-env.sh"; inlib "$CH" /dev/null s7_cmd_channel npm; rc=$?
[ $rc = 1 ] && grep -q 'run tools first' "$FX/o" && pass "channel refuses until tools has finished" || fail "channel without tools: rc=$rc"
# a state rewrite keeps the grant flag already in the file (a save after the grant step must not drop it)
mkhome; printf 'channel=cask\nentry=/e\ngranted=yes\nbundle=/b\n' > "$CH/.s7-channel"
( SD_S7_LIB=1; . "$SC"; HOME=$CH; s7_state_put cask /e2 "/b2" ) && grep -qx 'granted=yes' "$CH/.s7-channel" && grep -qx 'entry=/e2' "$CH/.s7-channel" && grep -qx 'bundle=/b2' "$CH/.s7-channel" \
  && pass "a state rewrite keeps the grant flag" || fail "a state rewrite: $(tr '\n' ' ' < "$CH/.s7-channel")"
# --- the grant flag follows every read: whatever stops a channel, it is set while the last read was
# not denied (build review 2)
fl() { grep -qx 'granted=yes' "$CH/.s7-channel"; }
drive 'n\nn\n' denied
[ $rc = 1 ] && res | grep -q 'STOPPED at (d): no answer' && fl && pass "channel: no answer at the grant step leaves the grant flag" || fail "channel, no answer at (d): rc=$rc flag $(fl && echo set || echo clear) $(res | tail -1)"
drive 'n\nn\ny\ny\ny\ny\ny\n' denied allowed allowed allowed allowed allowed
[ $rc = 1 ] && res | grep -q 'STOPPED at (g): the grant could not be removed' && fl && pass "channel: a grant not removed after three tries leaves the flag" || fail "channel, (g) not removed: rc=$rc flag $(fl && echo set || echo clear) $(res | tail -1)"
! res | grep -q 'denied after the removal' && ! res | grep -q 'removed with' && pass "channel: no removal, nor how it was made, is recorded while the read is still allowed" || fail "channel: a removal recorded though the read stayed allowed: $(res | grep 'removed with' | head -2 | tr '\n' ' ')"
drive 'n\nn\ny\ny\ny\ny\n' denied allowed denied allowed allowed allowed
[ $rc = 1 ] && res | grep -q 'STOPPED at (h)' && fl && pass "channel: an allowed read at (h) stops it with the grant flag set" || fail "channel, (h) allowed: rc=$rc flag $(fl && echo set || echo clear) $(res | tail -1)"
printf 'allowed\n' > "$FX/reads"; inlib "$CH" /dev/null s7_uninstall install-sh; rc=$?
[ $rc = 1 ] && [ -e "$CH/Applications/Sheepdog.app" ] && pass "uninstall refuses after that stop while the read is allowed" || fail "uninstall after (h): rc=$rc"
drive 'n\nn\ny\ny\ny\ny\n' denied allowed denied denied
[ $rc = 0 ] && ! fl && pass "control: the same drive with a denied read after the row's removal ends with the flag clear" || fail "control, (h) denied: rc=$rc flag $(fl && echo set || echo clear) $(res | tail -1)"
drive 'n\nn\ny\ny\nn\n' allowed
[ $rc = 1 ] && res | grep -q 'STOPPED at (c)' && fl && pass "channel: an allowed read before any grant stops it with the flag set" || fail "channel, (c) allowed: rc=$rc flag $(fl && echo set || echo clear)"
drive 'n\nn\ny\ny\nn\n' bad denied allowed denied
[ $rc = 0 ] && [ "$(grep -c '^read ' "$FX/ran")" = 4 ] && pass "channel: an unclear read is read again (bad, then denied, goes on)" || fail "channel, retry: rc=$rc ran '$(tr '\n' ' ' < "$FX/ran")'"
# uninstall after a grant: the denied read is recorded; with the entry gone, only the operator's
# recorded statement clears the flag
drive 'n\nn\ny\ny\n' denied allowed bad bad bad; UG=$CH
printf 'denied\n' > "$FX/reads"; inlib "$UG" /dev/null s7_uninstall install-sh; rc=$?
[ $rc = 0 ] && res | grep -q 'uninstall: the read is denied: no grant remains' && pass "uninstall records the denied read that clears the flag" || fail "uninstall's record: rc=$rc $(res | tail -1)"
drive 'n\nn\ny\ny\n' denied allowed bad bad bad; UG=$CH; rm -rf "$UG/Applications/Sheepdog.app" "$UG/.local/bin/sheepdog"
inlib "$UG" /dev/null s7_uninstall install-sh; rc=$?
[ $rc = 1 ] && fl && grep -q 'operator-removed-grant' "$FX/o" && pass "uninstall with the entry gone refuses and names the operator's way out" || fail "uninstall, entry gone: rc=$rc $(tr '\n' ' ' < "$FX/o")"
inlib "$UG" /dev/null s7_uninstall install-sh --operator-removed-grant; rc=$?
[ $rc = 0 ] && [ ! -e "$UG/.s7-channel" ] && res | grep -q 'the operator states the grant was removed' && pass "uninstall --operator-removed-grant, with the entry gone, records the statement and finishes" || fail "uninstall --operator-removed-grant: rc=$rc $(tr '\n' ' ' < "$FX/o")"
drive 'n\nn\ny\ny\n' denied allowed bad bad bad; UG=$CH; printf 'allowed\n' > "$FX/reads"
inlib "$UG" /dev/null s7_uninstall install-sh --operator-removed-grant; rc=$?
[ $rc = 1 ] && fl && pass "uninstall --operator-removed-grant is refused while the entry can still read (and the read is allowed)" || fail "uninstall --operator-removed-grant with the entry there: rc=$rc"
# a removal that leaves the bundle keeps the state and names it (control: the cask row above)
mkhome; mkdir -p "$CH/homebrew/Caskroom/sheepdog" "$CH/homebrew/bin" "$CH/Applications/Sheepdog.app"
printf '#!/bin/sh\nrm -rf "%s/homebrew/Caskroom/sheepdog"\n' "$CH" > "$CH/homebrew/bin/brew"; chmod 755 "$CH/homebrew/bin/brew"
printf 'channel=cask\nentry=\nbundle=%s\n' "$CH/Applications/Sheepdog.app" > "$CH/.s7-channel"
inlib "$CH" /dev/null s7_uninstall cask; rc=$?
[ $rc = 1 ] && [ -e "$CH/.s7-channel" ] && grep -q "$CH/Applications/Sheepdog.app is still there" "$FX/o" && pass "uninstall keeps the state and names a bundle the removal left" || fail "uninstall, bundle left: rc=$rc $(tr '\n' ' ' < "$FX/o")"
# (g)'s removal loop: an unclear read is recorded as unclear, never as "could not be removed"
drive 'n\nn\ny\ny\ny\n' denied allowed allowed bad bad bad
[ $rc = 1 ] && res | grep -q 'STOPPED at (g): the read after the removal is not a clear answer' && ! res | grep -q 'could not be removed' && fl \
  && pass "channel: an unclear read after a removal stops it as unclear, the flag set" || fail "channel, (g) loop unclear: rc=$rc $(res | tail -1)"
# uninstall: once a denied read cleared the flag, a rerun after a removal that failed partway reads nothing again
drive 'n\nn\ny\ny\n' denied allowed bad bad bad; UG=$CH
chmod 555 "$UG/Applications"; printf 'denied\n' > "$FX/reads"; inlib "$UG" /dev/null s7_uninstall install-sh; rc1=$?; chmod 755 "$UG/Applications"
printf 'allowed\n' > "$FX/reads"; inlib "$UG" /dev/null s7_uninstall install-sh; rc=$?
[ $rc1 = 1 ] && [ $rc = 0 ] && [ "$(cat "$FX/reads")" = allowed ] && ! grep -q 'operator-removed-grant' "$FX/o" \
  && pass "uninstall rerun after a denied read and a partial removal: no second read, no statement asked" || fail "uninstall rerun after a partial removal: rc1=$rc1 rc=$rc reads '$(cat "$FX/reads")' $(tr '\n' ' ' < "$FX/o")"
# the operator's statement is recorded before the flag clears; a failed record keeps the flag
drive 'n\nn\ny\ny\n' denied allowed bad bad bad; UG=$CH; rm -rf "$UG/Applications/Sheepdog.app" "$UG/.local/bin/sheepdog"
chmod 555 "$CD/results"; inlib "$UG" /dev/null s7_uninstall install-sh --operator-removed-grant; rc=$?; chmod 755 "$CD/results"
[ $rc = 1 ] && fl && pass "uninstall --operator-removed-grant with no place to record it keeps the flag" || fail "the statement with results/ read-only: rc=$rc flag $(fl && echo set || echo clear)"
# a read that cannot write the flag is a stop, the flag as it was (set before the read)
mkhome; printf 'channel=install-sh\nentry=/x\n' > "$CH/.s7-channel"; mkdir -p "$FX/rt"; : > "$FX/rr"
chmod 555 "$CH"; inlib "$CH" /dev/null eval 'c_e=/usr/bin/true c_t=$FX/rt c_r=$FX/rr; s7_read'; chmod 755 "$CH"; k=$(tail -1 "$FX/o")
case $k in "bad: cannot record the grant flag"*) pass "a read whose flag write fails returns a stop reason, not a read" ;; *) fail "a read with the state unwritable gave '$k'" ;; esac
# the statement clears the flag: a rerun after a removal that failed partway needs no second statement
drive 'n\nn\ny\ny\n' denied allowed bad bad bad; UG=$CH; rm -f "$UG/.local/bin/sheepdog"
chmod 555 "$UG/Applications"; inlib "$UG" /dev/null s7_uninstall install-sh --operator-removed-grant; rc1=$?; chmod 755 "$UG/Applications"
inlib "$UG" /dev/null s7_uninstall install-sh; rc=$?
[ $rc1 = 1 ] && [ $rc = 0 ] && [ ! -e "$UG/.s7-channel" ] && pass "uninstall rerun after the statement and a partial removal needs no second statement" || fail "rerun after the statement: rc1=$rc1 rc=$rc $(tr '\n' ' ' < "$FX/o")"
# a state write that fails stops the channel naming it: before the first read (nothing read), and
# when the clear after a denied read fails (the flag stays set)
: > "$FX/blockstate"; drive 'n\nn\n' denied; rm -f "$FX/blockstate"
[ $rc = 1 ] && res | grep -q 'STOPPED at .*cannot record the grant flag' && ! res | grep -q 'STOPPED.*is not denied' && [ "$(grep -c '^read ' "$FX/ran")" = 0 ] \
  && pass "channel: a state that cannot be written before the first read stops it, naming the write, with nothing read" || fail "channel, state unwritable: rc=$rc ran '$(tr '\n' ' ' < "$FX/ran")' $(res | tail -1)"
rm -rf "$CH/.s7-channel.new"
: > "$FX/blockclear"; drive 'n\nn\n' denied; rm -f "$FX/blockclear"
[ $rc = 1 ] && res | grep -q 'STOPPED at (c): bad: cannot record the grant flag' && fl && pass "channel: a clear that fails after a denied read stops it with the flag still set" || fail "channel, clear fails: rc=$rc flag $(fl && echo set || echo clear) $(res | tail -1)"
rm -rf "$CH/.s7-channel.new"
# "(g) Done?" answered n asks again: no read, no try counted
drive 'n\nn\ny\ny\nn\ny\ny\nn\n' denied allowed allowed denied
[ $rc = 0 ] && [ "$(grep -c '^read ' "$FX/ran")" = 4 ] && res | grep -qx '  (g) denied after the removal, with − here' && pass "channel: a 'n' to (g) Done? asks again without a read" || fail "channel, Done? n: rc=$rc ran '$(tr '\n' ' ' < "$FX/ran")' $(res | tail -2 | tr '\n' ' ')"
lastread() { [ "$(res | grep '^  read: ' | tail -1 | cut -d' ' -f4)" = "$1" ]; } # the last read recorded was WANT
# a failed clear at each (g) read names the write (the removal loop's and the one after the reset)
drive 'n\nn\ny\ny\n' denied allowed denied+block
[ $rc = 1 ] && res | grep -q 'STOPPED at (g): bad: cannot record the grant flag' && lastread denied && fl && pass "channel: a failed clear after the reset's denied read stops at (g), naming the write" || fail "channel, (g) clear fails: rc=$rc $(res | tail -1)"
rm -rf "$CH/.s7-channel.new"
drive 'n\nn\ny\ny\ny\n' denied allowed allowed denied+block
[ $rc = 1 ] && res | grep -q 'STOPPED at (g): bad: cannot record the grant flag' && lastread denied && fl && pass "channel: a failed clear in the removal loop stops at (g), naming the write" || fail "channel, (g) loop clear fails: rc=$rc $(res | tail -1)"
rm -rf "$CH/.s7-channel.new"
drive 'n\nn\ny\ny\ny\ny\n' denied allowed denied denied+block
[ $rc = 1 ] && res | grep -q 'STOPPED at (h): bad: cannot record the grant flag' && lastread denied && [ "$(res | grep -c '^  read: ')" = 4 ] && fl && pass "channel: a failed clear at (h) stops it there, naming the write (four reads recorded: c, e, g, h)" || fail "channel, (h) clear fails: rc=$rc $(res | tail -1)"
rm -rf "$CH/.s7-channel.new"
# (g): Done? is asked in every removal round; two 'n' answers are not tries
drive 'n\nn\ny\ny\ny\ny\ny\nn\n' denied allowed allowed allowed denied
[ $rc = 0 ] && [ "$(res | grep -c 'asked: (g) Done?')" = 2 ] && pass "channel: (g) Done? is asked in each removal round" || fail "channel, Done? per round: rc=$rc $(res | grep -c 'asked: (g) Done?') asks"
drive 'n\nn\ny\ny\nn\nn\ny\nn\nn\ny\ny\nn\n' denied allowed allowed allowed denied
[ $rc = 0 ] && res | grep -q '(g) denied after the removal' && ! res | grep -q 'could not be removed' && [ "$(res | grep -c 'asked: (g) Done?')" = 6 ] && pass "channel: 'n' answers to (g) Done? are not counted as removal tries (six asks, two reads)" || fail "channel, n not a try: rc=$rc $(res | tail -2 | tr '\n' ' ')"
# (h): a 'n' to Done? asks again with no read (control: the 'y' path, the (h) rows above)
drive 'n\nn\ny\ny\ny\nn\ny\n' denied allowed denied denied
[ $rc = 0 ] && [ "$(grep -c '^read ' "$FX/ran")" = 4 ] && [ "$(res | grep -c 'asked: (h) Done?')" = 2 ] && [ "$(res | sed -n '/asked: (h) Done? -> y/,$p' | grep -c '^  read: ')" = 1 ] \
  && pass "channel: a 'n' to (h) Done? asks again without a read, and the (h) read follows the 'y'" || fail "channel, (h) Done? n: rc=$rc ran '$(tr '\n' ' ' < "$FX/ran")'"
finish
