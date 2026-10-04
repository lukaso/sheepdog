#!/bin/sh
# The checks in tests/lib/npm-gates.sh that cell 18 (t_npm_install.sh) runs before and after the
# first run of an install, on fixtures (no build, no install; the fixture "executables" are shell
# scripts that print their own path, in bundles with no Info.plist).
#   - npm_target, the gate's copy of the launcher's resolution, agrees with the launcher itself
#     (run through a link, as a pnpm shim runs it, and through a link chain) for each layout:
#     nested, hoisted, both, a linked platform package, a linked executable, a nested one that is
#     not executable, none;
#   - npm_gate passes a good npm layout and a good pnpm shim (pnpm 10.18.2's measured text); it
#     refuses: any bad copy of the launcher whatever order find returns it in, a PATH entry that
#     reaches an unjudged launcher (a link or a pnpm shim), a shim that names no launcher, a shim
#     whose `$basedir//bin/sh` exists, a shim that is not pnpm 10.18.2's text though its exec lines
#     name the good launcher (a line before them, an exec with no arguments, a trailing comment,
#     pnpm 10.2.1's shim; and one exec that runs another file with the launcher as its argument,
#     whose captured target holds a quote and $, so the character check refuses it first), pnpm
#     10.18.2's text whose target or NODE_PATH holds a forbidden character (a command in NODE_PATH,
#     a $ in the target), a launcher copy that differs from the package's, an entry that is neither a link nor
#     a shim, and a layout that resolves to nothing; a copy that resolves to nothing beside a good
#     one passes (it runs nothing);
#   - runs_one_of: the judged file itself passes; a wrapper that spawns it (alive, its child
#     passing), a decoy at a path named like it, and a process whose argv[0] is the judged path
#     but which runs another file (ps shows the lie) all fail;
#   - no process the cell started outlives it.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
. "$SR_ROOT/tests/lib/npm-gates.sh"
L="$SR_ROOT/npm/sheepr/bin/sheepr"
real() { printf '%s/%s\n' "$(cd -P "$(dirname "$1")" && pwd -P)" "$(basename "$1")"; }
X=Sheepr.app/Contents/MacOS/sheepr
exe_at() { # platform-package-dir -> an executable script at its bundle path that prints its own real path
  mkdir -p "$1/Sheepr.app/Contents/MacOS" && : > "$1/$X" && printf '#!/bin/sh\necho %s\n' "$(real "$1/$X")" > "$1/$X" && chmod 755 "$1/$X"
}
pkgdir() { mkdir -p "$1/bin" && cp "$L" "$1/bin/sheepr"; } # a main package with the launcher

# --- npm_target agrees with the launcher
agree() { # what arg0 [how: link|sh]
  if [ "${3:-sh}" = sh ]; then got=$(env -i PATH=/usr/bin:/bin HOME="$FX" sh "$2" 2>/dev/null); else got=$(env -i PATH=/usr/bin:/bin HOME="$FX" "$2" 2>/dev/null); fi
  want=$(npm_target "$2"); wr=$?
  if [ -z "$got" ]; then [ $wr != 0 ] && pass "agree, $1: neither resolves" || fail "agree, $1: the launcher resolves to nothing, npm_target to '$want'"
  else [ $wr = 0 ] && [ "$want" = "$got" ] && pass "agree, $1: $got" || fail "agree, $1: the launcher runs '$got', npm_target says '$want' (rc $wr)"; fi
}
A=$FX/agree
pkgdir "$A/nested/m/sheepr"; exe_at "$A/nested/m/sheepr/node_modules/sheepr-darwin-universal"
mkdir -p "$A/nested/bin" && ln -s ../m/sheepr/bin/sheepr "$A/nested/bin/sheepr"
agree "nested, through a relative link" "$A/nested/bin/sheepr" link
ln -s "$A/nested/bin/sheepr" "$A/chain"
agree "nested, through a link to a link" "$A/chain" link
pkgdir "$A/hoist/sheepr"; exe_at "$A/hoist/sheepr-darwin-universal"
agree "hoisted, run as a pnpm shim runs it" "$A/hoist/sheepr/bin/sheepr"
pkgdir "$A/both/sheepr"; exe_at "$A/both/sheepr/node_modules/sheepr-darwin-universal"; exe_at "$A/both/sheepr-darwin-universal"
agree "nested and hoisted (nested first)" "$A/both/sheepr/bin/sheepr"
pkgdir "$A/plink/sheepr"; exe_at "$A/elsewhere/pd"; ln -s "$A/elsewhere/pd" "$A/plink/sheepr-darwin-universal"
agree "a linked platform package (pnpm's sibling link)" "$A/plink/sheepr/bin/sheepr"
pkgdir "$A/xlink/sheepr"; mkdir -p "$A/xlink/sheepr-darwin-universal/Sheepr.app/Contents/MacOS"; exe_at "$A/elsewhere/xd"
ln -s "$A/elsewhere/xd/$X" "$A/xlink/sheepr-darwin-universal/$X"
agree "a linked executable" "$A/xlink/sheepr/bin/sheepr"
pkgdir "$A/noexec/sheepr"; exe_at "$A/noexec/sheepr/node_modules/sheepr-darwin-universal"; chmod 644 "$A/noexec/sheepr/node_modules/sheepr-darwin-universal/$X"
exe_at "$A/noexec/sheepr-darwin-universal"
agree "a nested executable without x (the hoisted one runs)" "$A/noexec/sheepr/bin/sheepr"
pkgdir "$A/none/sheepr"
agree "no platform package" "$A/none/sheepr/bin/sheepr"

# --- npm_gate
gate() { # what expect(pass|refuse) home insts entry [launcher]
  npm_gate "$3" "$4" "$5" "${6:-$L}" > "$FX/g" 2>&1; r=$?
  case $2 in
    pass) [ $r = 0 ] && pass "gate passes $1" || fail "gate refuses $1: $(tr '\n' ' ' < "$FX/g")" ;;
    refuse) [ $r != 0 ] && [ -s "$FX/g" ] && pass "gate refuses $1 ($(head -1 "$FX/g"))" || fail "gate passes $1 (rc $r, '$(tr '\n' ' ' < "$FX/g")')" ;;
  esac
}
npm_home() { # home: an npm global install; INSTS its executable
  p=$1/prefix/lib/node_modules/sheepr
  pkgdir "$p"; exe_at "$p/node_modules/sheepr-darwin-universal"
  mkdir -p "$1/prefix/bin" && ln -s ../lib/node_modules/sheepr/bin/sheepr "$1/prefix/bin/sheepr"
  INSTS=$(real "$p/node_modules/sheepr-darwin-universal/$X")
}
# pnpm's global shim for a #!/bin/sh bin: the text pnpm 10.18.2 writes (measured 2026-10-02 with
# the pinned pnpm on a throwaway package), with @T@ the launcher's path relative to the shim's dir
# and @NP@ the NODE_PATH value; and 10.2.1's (measured 2026-10-02), to show another version's shim
# is refused
cat > "$FX/shim.1018" <<'SH'
#!/bin/sh
basedir=$(dirname "$(echo "$0" | sed -e 's,\\,/,g')")

case `uname` in
    *CYGWIN*|*MINGW*|*MSYS*)
        if command -v cygpath > /dev/null 2>&1; then
            basedir=`cygpath -w "$basedir"`
        fi
    ;;
esac

if [ -z "$NODE_PATH" ]; then
  export NODE_PATH="@NP@"
else
  export NODE_PATH="@NP@:$NODE_PATH"
fi
if [ -x "$basedir//bin/sh" ]; then
  exec "$basedir//bin/sh"  "$basedir/@T@" "$@"
else
  exec /bin/sh  "$basedir/@T@" "$@"
fi
SH
cat > "$FX/shim.1021" <<'SH'
#!/bin/sh
basedir=$(dirname "$(echo "$0" | sed -e 's,\\,/,g')")

case `uname` in
    *CYGWIN*) basedir=`cygpath -w "$basedir"`;;
esac

if [ -z "$NODE_PATH" ]; then
  export NODE_PATH="@NP@"
else
  export NODE_PATH="@NP@:$NODE_PATH"
fi
if [ -x "$basedir//bin/sh" ]; then
  exec "$basedir//bin/sh"  "$basedir/@T@" "$@"
else
  exec /bin/sh  "$basedir/@T@" "$@"
fi
SH
grep -q 'sed -e .s,\\\\,/,g.' "$FX/shim.1018" || fail "the shim template lost its sed backslashes"
shim() { # shim-path target [template] [NODE_PATH]
  python3 -c 'import sys; t, d, tg, np = sys.argv[1:5]; open(d, "w").write(open(t).read().replace("@T@", tg).replace("@NP@", np))' \
    "${3:-$FX/shim.1018}" "$1" "$2" "${4:-/nonexistent/.pnpm/node_modules}"
  chmod 755 "$1"
}
G=$FX/gate
npm_home "$G/ok"
gate "a good npm install" pass "$G/ok" "$INSTS" "$G/ok/prefix/bin/sheepr"
# a second copy whose platform package is a link out of the home, to an executable the door never
# judged: refused whichever copy find returns first (n1 bad + n2 good, and n1 good + n2 bad)
exe_at "$FX/outside/pd"
for o in 1 2; do
  h=$G/two$o; npm_home "$h"; good=$INSTS
  for c in n1 n2; do
    pkgdir "$h/$c/sheepr"
    if { [ $o = 1 ] && [ $c = n1 ]; } || { [ $o = 2 ] && [ $c = n2 ]; }; then ln -s "$FX/outside/pd" "$h/$c/sheepr-darwin-universal"
    else exe_at "$h/$c/sheepr-darwin-universal"; good="$good
$(real "$h/$c/sheepr-darwin-universal/$X")"; fi
  done
  gate "two launcher copies, the bad one $([ $o = 1 ] && echo n1 || echo n2)" refuse "$h" "$good" "$h/prefix/bin/sheepr"
done
# the PATH entry links to a launcher outside the home (every copy inside is good)
npm_home "$G/out"; pkgdir "$FX/outl/sheepr"; ln -s "$FX/outside/pd" "$FX/outl/sheepr-darwin-universal"
rm "$G/out/prefix/bin/sheepr" && ln -s "$FX/outl/sheepr/bin/sheepr" "$G/out/prefix/bin/sheepr"
gate "a PATH link to a launcher outside the home that runs an unjudged file" refuse "$G/out" "$INSTS" "$G/out/prefix/bin/sheepr"
# pnpm: the shim, the virtual store, the platform package a sibling link
pn() { # home -> a pnpm global install; INSTS
  v=$1/pnpm/global/5/.pnpm/sheepr@0.1.0/node_modules
  pkgdir "$v/sheepr"; exe_at "$1/pnpm/global/5/.pnpm/sheepr-darwin-universal@0.1.0/node_modules/sheepr-darwin-universal"
  ln -s ../../sheepr-darwin-universal@0.1.0/node_modules/sheepr-darwin-universal "$v/sheepr-darwin-universal"
  [ -d "$v/sheepr-darwin-universal/" ] || fail "the pnpm fixture's sibling link does not resolve (its rows would prove nothing)"
  shim "$1/pnpm/sheepr" "global/5/.pnpm/sheepr@0.1.0/node_modules/sheepr/bin/sheepr"
  INSTS=$(real "$1/pnpm/global/5/.pnpm/sheepr-darwin-universal@0.1.0/node_modules/sheepr-darwin-universal/$X")
}
pn "$G/pn"
gate "a good pnpm install (the shim)" pass "$G/pn" "$INSTS" "$G/pn/pnpm/sheepr"
pn "$G/pnout"; shim "$G/pnout/pnpm/sheepr" "../../../outl/sheepr/bin/sheepr"
[ -f "$G/pnout/pnpm/../../../outl/sheepr/bin/sheepr" ] || fail "the shim fixture does not reach the outside launcher (the row would prove nothing)"
gate "a pnpm shim naming a launcher outside the home that runs an unjudged file" refuse "$G/pnout" "$INSTS" "$G/pnout/pnpm/sheepr"
pn "$G/pnnone"; printf '#!/bin/sh\nbasedir=$(dirname "$0")\nexec node "$@"\n' > "$G/pnnone/pnpm/sheepr"
gate "a shim that names no launcher" refuse "$G/pnnone" "$INSTS" "$G/pnnone/pnpm/sheepr"
pn "$G/pnsh"; mkdir -p "$G/pnsh/pnpm/bin" && printf '#!/bin/sh\n' > "$G/pnsh/pnpm/bin/sh" && chmod 755 "$G/pnsh/pnpm/bin/sh"
gate "a shim whose \$basedir//bin/sh exists" refuse "$G/pnsh" "$INSTS" "$G/pnsh/pnpm/sheepr"
# shims that are not what pnpm 10.18.2 writes; each still names the good launcher in its exec lines
# (a gate that reads only those lines passes them), and each would run the unjudged outside file
PL=global/5/.pnpm/sheepr@0.1.0/node_modules/sheepr/bin/sheepr
EV='"$basedir/../../../outside/pd/Sheepr.app/Contents/MacOS/sheepr"'
bsn=0
badshim() { # what old new: the good shim with old replaced by new (once)
  bsn=$((bsn + 1)); h=$G/bs$bsn; pn "$h"
  python3 -c 'import sys; p, a, b = sys.argv[1:4]; s = open(p).read(); assert s.count(a) >= 1, a; open(p, "w").write(s.replace(a, b, 1))' "$h/pnpm/sheepr" "$2" "$3" \
    || { fail "$1: the fixture edit did not apply"; return; }
  gate "$1" refuse "$h" "$INSTS" "$h/pnpm/sheepr"
}
badshim "a shim with a line before its exec block that runs another file" 'if [ -x "$basedir//bin/sh" ]' "$EV \"\$@\"
if [ -x \"\$basedir//bin/sh\" ]"
badshim "a shim with an exec that passes no arguments" '
case' "
exec $EV
case"
badshim "a shim whose exec runs another file with the launcher as its argument" 'exec /bin/sh  "$basedir/' "exec /bin/sh  $EV \"\$basedir/"
badshim "a shim with an exec line that ends in a comment" '
case' "
exec $EV \"\$@\" # x
case"
bsn=$((bsn + 1)); h=$G/bs$bsn; pn "$h"; shim "$h/pnpm/sheepr" "$PL" "" '$(/usr/bin/true)'
grep -q 'NODE_PATH="\$(/usr/bin/true)"' "$h/pnpm/sheepr" || fail "the NODE_PATH fixture did not apply"
gate "a shim whose NODE_PATH runs a command" refuse "$h" "$INSTS" "$h/pnpm/sheepr"
bsn=$((bsn + 1)); h=$G/bs$bsn; pn "$h"; shim "$h/pnpm/sheepr" "$PL" "$FX/shim.1021"
gate "pnpm 10.2.1's shim (another version's text)" refuse "$h" "$INSTS" "$h/pnpm/sheepr"
# a target with a $ in it: read literally it reaches a good launcher (a directory named $Q), but sh
# expands $Q to nothing and runs the outside launcher
bsn=$((bsn + 1)); h=$G/bs$bsn; pn "$h"; mkdir -p "$h/pnpm/\$Q"
pkgdir "$G/outl/sheepr"; exe_at "$G/outl/sheepr-darwin-universal"
shim "$h/pnpm/sheepr" '$Q/../../../outl/sheepr/bin/sheepr'
cmp -s "$h/pnpm/\$Q/../../../outl/sheepr/bin/sheepr" "$L" && [ -f "$h/pnpm/../../../outl/sheepr/bin/sheepr" ] \
  || fail "the \$ fixture does not reach a good launcher literally and the outside one expanded (the row would prove nothing)"
gate "a shim whose target has a \$ in it" refuse "$h" "$INSTS
$(real "$G/outl/sheepr-darwin-universal/$X")" "$h/pnpm/sheepr"
# a launcher copy that is not the package's
npm_home "$G/diff"; printf '# changed\n' >> "$G/diff/prefix/lib/node_modules/sheepr/bin/sheepr"
gate "a launcher that differs from the package's" refuse "$G/diff" "$INSTS" "$G/diff/prefix/bin/sheepr"
npm_home "$G/plain"; rm "$G/plain/prefix/bin/sheepr" && printf '#!/bin/sh\nexit 0\n' > "$G/plain/prefix/bin/sheepr"
gate "a PATH entry that is neither a link nor a pnpm shim" refuse "$G/plain" "$INSTS" "$G/plain/prefix/bin/sheepr"
npm_home "$G/none"; rm -rf "$G/none/prefix/lib/node_modules/sheepr/node_modules"
gate "an install that resolves to nothing" refuse "$G/none" "$INSTS" "$G/none/prefix/bin/sheepr"
npm_home "$G/idle"; pkgdir "$G/idle/cache/sheepr"
gate "a good install beside a copy that resolves to nothing" pass "$G/idle" "$INSTS" "$G/idle/prefix/bin/sheepr"

# --- runs_one_of
printf '#include <stdlib.h>\n#include <unistd.h>\nint main(int c,char**v){sleep(c>1?atoi(v[1]):3);return 0;}\n' > "$FX/s.c"
mkdir -p "$FX/j/Sheepr.app/Contents/MacOS" "$FX/decoy/Sheepr.app/Contents/MacOS"
cc -o "$FX/j/$X" "$FX/s.c" && cc -o "$FX/decoy/$X" "$FX/s.c" || exit 3
J=$(real "$FX/j/$X")
PIDS=""; trap 'for p in $PIDS; do { kill $p; wait $p; } 2>/dev/null; done; rm -rf "$FX"' EXIT
cw() { # pid predicate: wait (up to 5 s) until the shell predicate holds
  i=0; while ! eval "$2" && kill -0 "$1" 2>/dev/null && [ $i -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
}
pcomm() { c=$(ps -o comm= -p "$1" 2>/dev/null); [ -n "$c" ] && real "$c" 2>/dev/null; }
"$J" 5 & p=$!; PIDS="$PIDS $p"; cw $p '[ "$(pcomm $p)" = "$J" ]'
runs_one_of $p "$J" && pass "control: the judged file itself passes the process check" || fail "control: the judged file itself fails the process check"
printf '#!/bin/sh\n"%s" "$@"\nexit $?\n' "$J" > "$FX/wrap"; chmod 755 "$FX/wrap"
"$FX/wrap" 30 & w=$!; PIDS="$PIDS $w"; cw $w '[ -n "$(pgrep -P $w)" ]'; k=$(pgrep -P $w | head -1); PIDS="$PIDS $k"
if kill -0 $w 2>/dev/null && [ -n "$k" ] && runs_one_of "$k" "$J"; then
  runs_one_of $w "$J" && fail "control: a spawning wrapper passed the process check" || pass "control: a spawning wrapper (alive, its child passing) fails the process check"
else fail "control: the wrapper is not alive with a child that runs the judged file (the row would prove nothing)"; fi
"$FX/decoy/$X" 5 & d=$!; PIDS="$PIDS $d"; cw $d '[ -n "$(pcomm $d)" ]'
case $(pcomm $d) in */$X) runs_one_of $d "$J" && fail "control: a decoy named like the judged file passed" || pass "control: a decoy named like the judged file fails the process check" ;;
  *) fail "control: the decoy does not run under the bundle's name (the row would prove nothing)" ;; esac
perl -e 'exec {"/bin/sleep"} $ARGV[0], "5" or die' "$J" & q=$!; PIDS="$PIDS $q"; cw $q '[ "$(pcomm $q)" = "$J" ]'
if [ "$(pcomm $q)" = "$J" ]; then
  runs_one_of $q "$J" && fail "control: a process with the judged file's argv[0] that runs /bin/sleep passed" || pass "control: argv[0] that names the judged file, running /bin/sleep, fails the process check (ps shows the lie)"
else fail "control: ps does not show the argv[0] lie ('$(pcomm $q)'): the row would prove nothing"; fi
# nothing the cell started outlives it (the wrapper's child too); control: the same pgrep lists the
# wrapper and its child while they live
fxp=$FX/
live=$(pgrep -f "$fxp")
printf '%s\n' "$live" | grep -qx "$w" && printf '%s\n' "$live" | grep -qx "$k" \
  && pass "control: the leftover check lists the live wrapper and its child" || fail "control: pgrep -f '$fxp' does not list the live wrapper $w and its child $k ('$live')"
for p in $PIDS; do { kill $p; wait $p; } 2>/dev/null; done
i=0; while [ -n "$(pgrep -f "$fxp")" ] && [ $i -lt 30 ]; do sleep 0.1; i=$((i + 1)); done
left=$(pgrep -fl "$fxp")
[ -z "$left" ] && pass "no fixture process is left" || { fail "fixture processes left: $left"; for x in $(pgrep -f "$fxp"); do kill "$x"; done; }
finish
