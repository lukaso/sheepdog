#!/bin/sh
# The checks in tests/lib/npm-gates.sh that cell 18 (t_npm_install.sh) runs before and after the
# first run of an install, on fixtures (no build, no install; the fixture "executables" are shell
# scripts that print their own path, in bundles with no Info.plist).
#   - npm_target, the gate's copy of the launcher's resolution, agrees with the launcher itself
#     (run through a link, as a pnpm shim runs it, and through a link chain) for each layout:
#     nested, hoisted, both, a linked platform package, a linked executable, a nested one that is
#     not executable, none;
#   - npm_gate passes a good npm layout and a good pnpm shim; it refuses: any bad copy of the
#     launcher whatever order find returns it in, a PATH entry that reaches an unjudged launcher
#     (a link or a pnpm shim), a shim that names no launcher, a shim whose `$basedir//bin/sh`
#     exists, a launcher copy that differs from the package's, an entry that is neither a link nor
#     a shim, and a layout that resolves to nothing; a copy that resolves to nothing beside a good
#     one passes (it runs nothing);
#   - runs_one_of: the judged file itself passes; a wrapper that spawns it (alive, its child
#     passing), a decoy at a path named like it, and a process whose argv[0] is the judged path
#     but which runs another file (ps shows the lie) all fail.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
. "$SD_ROOT/tests/lib/npm-gates.sh"
L="$SD_ROOT/npm/sheepdog/bin/sheepdog"
real() { printf '%s/%s\n' "$(cd -P "$(dirname "$1")" && pwd -P)" "$(basename "$1")"; }
X=Sheepdog.app/Contents/MacOS/sheepdog
exe_at() { # platform-package-dir -> an executable script at its bundle path that prints its own real path
  mkdir -p "$1/Sheepdog.app/Contents/MacOS" && : > "$1/$X" && printf '#!/bin/sh\necho %s\n' "$(real "$1/$X")" > "$1/$X" && chmod 755 "$1/$X"
}
pkgdir() { mkdir -p "$1/bin" && cp "$L" "$1/bin/sheepdog"; } # a main package with the launcher

# --- npm_target agrees with the launcher
agree() { # what arg0 [how: link|sh]
  if [ "${3:-sh}" = sh ]; then got=$(env -i PATH=/usr/bin:/bin HOME="$FX" sh "$2" 2>/dev/null); else got=$(env -i PATH=/usr/bin:/bin HOME="$FX" "$2" 2>/dev/null); fi
  want=$(npm_target "$2"); wr=$?
  if [ -z "$got" ]; then [ $wr != 0 ] && pass "agree, $1: neither resolves" || fail "agree, $1: the launcher resolves to nothing, npm_target to '$want'"
  else [ $wr = 0 ] && [ "$want" = "$got" ] && pass "agree, $1: $got" || fail "agree, $1: the launcher runs '$got', npm_target says '$want' (rc $wr)"; fi
}
A=$FX/agree
pkgdir "$A/nested/m/@lukaso/sheepdog"; exe_at "$A/nested/m/@lukaso/sheepdog/node_modules/@lukaso/sheepdog-darwin-universal"
mkdir -p "$A/nested/bin" && ln -s ../m/@lukaso/sheepdog/bin/sheepdog "$A/nested/bin/sheepdog"
agree "nested, through a relative link" "$A/nested/bin/sheepdog" link
ln -s "$A/nested/bin/sheepdog" "$A/chain"
agree "nested, through a link to a link" "$A/chain" link
pkgdir "$A/hoist/@lukaso/sheepdog"; exe_at "$A/hoist/@lukaso/sheepdog-darwin-universal"
agree "hoisted, run as a pnpm shim runs it" "$A/hoist/@lukaso/sheepdog/bin/sheepdog"
pkgdir "$A/both/@lukaso/sheepdog"; exe_at "$A/both/@lukaso/sheepdog/node_modules/@lukaso/sheepdog-darwin-universal"; exe_at "$A/both/@lukaso/sheepdog-darwin-universal"
agree "nested and hoisted (nested first)" "$A/both/@lukaso/sheepdog/bin/sheepdog"
pkgdir "$A/plink/@lukaso/sheepdog"; exe_at "$A/elsewhere/pd"; ln -s "$A/elsewhere/pd" "$A/plink/@lukaso/sheepdog-darwin-universal"
agree "a linked platform package (pnpm's sibling link)" "$A/plink/@lukaso/sheepdog/bin/sheepdog"
pkgdir "$A/xlink/@lukaso/sheepdog"; mkdir -p "$A/xlink/@lukaso/sheepdog-darwin-universal/Sheepdog.app/Contents/MacOS"; exe_at "$A/elsewhere/xd"
ln -s "$A/elsewhere/xd/$X" "$A/xlink/@lukaso/sheepdog-darwin-universal/$X"
agree "a linked executable" "$A/xlink/@lukaso/sheepdog/bin/sheepdog"
pkgdir "$A/noexec/@lukaso/sheepdog"; exe_at "$A/noexec/@lukaso/sheepdog/node_modules/@lukaso/sheepdog-darwin-universal"; chmod 644 "$A/noexec/@lukaso/sheepdog/node_modules/@lukaso/sheepdog-darwin-universal/$X"
exe_at "$A/noexec/@lukaso/sheepdog-darwin-universal"
agree "a nested executable without x (the hoisted one runs)" "$A/noexec/@lukaso/sheepdog/bin/sheepdog"
pkgdir "$A/none/@lukaso/sheepdog"
agree "no platform package" "$A/none/@lukaso/sheepdog/bin/sheepdog"

# --- npm_gate
gate() { # what expect(pass|refuse) home insts entry [launcher]
  npm_gate "$3" "$4" "$5" "${6:-$L}" > "$FX/g" 2>&1; r=$?
  case $2 in
    pass) [ $r = 0 ] && pass "gate passes $1" || fail "gate refuses $1: $(tr '\n' ' ' < "$FX/g")" ;;
    refuse) [ $r != 0 ] && [ -s "$FX/g" ] && pass "gate refuses $1 ($(head -1 "$FX/g"))" || fail "gate passes $1 (rc $r, '$(tr '\n' ' ' < "$FX/g")')" ;;
  esac
}
npm_home() { # home: an npm global install; INSTS its executable
  p=$1/prefix/lib/node_modules/@lukaso/sheepdog
  pkgdir "$p"; exe_at "$p/node_modules/@lukaso/sheepdog-darwin-universal"
  mkdir -p "$1/prefix/bin" && ln -s ../lib/node_modules/@lukaso/sheepdog/bin/sheepdog "$1/prefix/bin/sheepdog"
  INSTS=$(real "$p/node_modules/@lukaso/sheepdog-darwin-universal/$X")
}
shim() { # shim-path launcher-path-relative-to-the-shim's-dir: pnpm's global shim, in the shape pnpm
  # 10.2.1 wrote (measured); cell 18 puts the pinned 10.18.2's own shim through the gate
  cat > "$1" <<SH
#!/bin/sh
basedir=\$(dirname "\$(echo "\$0" | sed -e 's,\\\\,/,g')")

case \`uname\` in
    *CYGWIN*) basedir=\`cygpath -w "\$basedir"\`;;
esac

if [ -z "\$NODE_PATH" ]; then
  export NODE_PATH="/nonexistent"
else
  export NODE_PATH="/nonexistent:\$NODE_PATH"
fi
if [ -x "\$basedir//bin/sh" ]; then
  exec "\$basedir//bin/sh"  "\$basedir/$2" "\$@"
else
  exec /bin/sh  "\$basedir/$2" "\$@"
fi
SH
  chmod 755 "$1"
}
G=$FX/gate
npm_home "$G/ok"
gate "a good npm install" pass "$G/ok" "$INSTS" "$G/ok/prefix/bin/sheepdog"
# a second copy whose platform package is a link out of the home, to an executable the door never
# judged: refused whichever copy find returns first (n1 bad + n2 good, and n1 good + n2 bad)
exe_at "$FX/outside/pd"
for o in 1 2; do
  h=$G/two$o; npm_home "$h"; good=$INSTS
  for c in n1 n2; do
    pkgdir "$h/$c/@lukaso/sheepdog"
    if { [ $o = 1 ] && [ $c = n1 ]; } || { [ $o = 2 ] && [ $c = n2 ]; }; then ln -s "$FX/outside/pd" "$h/$c/@lukaso/sheepdog-darwin-universal"
    else exe_at "$h/$c/@lukaso/sheepdog-darwin-universal"; good="$good
$(real "$h/$c/@lukaso/sheepdog-darwin-universal/$X")"; fi
  done
  gate "two launcher copies, the bad one $([ $o = 1 ] && echo n1 || echo n2)" refuse "$h" "$good" "$h/prefix/bin/sheepdog"
done
# the PATH entry links to a launcher outside the home (every copy inside is good)
npm_home "$G/out"; pkgdir "$FX/outl/@lukaso/sheepdog"; ln -s "$FX/outside/pd" "$FX/outl/@lukaso/sheepdog-darwin-universal"
rm "$G/out/prefix/bin/sheepdog" && ln -s "$FX/outl/@lukaso/sheepdog/bin/sheepdog" "$G/out/prefix/bin/sheepdog"
gate "a PATH link to a launcher outside the home that runs an unjudged file" refuse "$G/out" "$INSTS" "$G/out/prefix/bin/sheepdog"
# pnpm: the shim, the virtual store, the platform package a sibling link
pn() { # home -> a pnpm global install; INSTS
  v=$1/pnpm/global/5/.pnpm/@lukaso+sheepdog@0.1.0/node_modules
  pkgdir "$v/@lukaso/sheepdog"; exe_at "$1/pnpm/global/5/.pnpm/@lukaso+sheepdog-darwin-universal@0.1.0/node_modules/@lukaso/sheepdog-darwin-universal"
  ln -s ../../../@lukaso+sheepdog-darwin-universal@0.1.0/node_modules/@lukaso/sheepdog-darwin-universal "$v/@lukaso/sheepdog-darwin-universal"
  shim "$1/pnpm/sheepdog" "global/5/.pnpm/@lukaso+sheepdog@0.1.0/node_modules/@lukaso/sheepdog/bin/sheepdog"
  INSTS=$(real "$1/pnpm/global/5/.pnpm/@lukaso+sheepdog-darwin-universal@0.1.0/node_modules/@lukaso/sheepdog-darwin-universal/$X")
}
pn "$G/pn"
gate "a good pnpm install (the shim)" pass "$G/pn" "$INSTS" "$G/pn/pnpm/sheepdog"
pn "$G/pnout"; shim "$G/pnout/pnpm/sheepdog" "../../../outl/@lukaso/sheepdog/bin/sheepdog"
[ -f "$G/pnout/pnpm/../../../outl/@lukaso/sheepdog/bin/sheepdog" ] || fail "the shim fixture does not reach the outside launcher (the row would prove nothing)"
gate "a pnpm shim naming a launcher outside the home that runs an unjudged file" refuse "$G/pnout" "$INSTS" "$G/pnout/pnpm/sheepdog"
pn "$G/pnnone"; printf '#!/bin/sh\nbasedir=$(dirname "$0")\nexec node "$@"\n' > "$G/pnnone/pnpm/sheepdog"
gate "a shim that names no launcher" refuse "$G/pnnone" "$INSTS" "$G/pnnone/pnpm/sheepdog"
pn "$G/pnsh"; mkdir -p "$G/pnsh/pnpm/bin" && printf '#!/bin/sh\n' > "$G/pnsh/pnpm/bin/sh" && chmod 755 "$G/pnsh/pnpm/bin/sh"
gate "a shim whose \$basedir//bin/sh exists" refuse "$G/pnsh" "$INSTS" "$G/pnsh/pnpm/sheepdog"
# a launcher copy that is not the package's
npm_home "$G/diff"; printf '# changed\n' >> "$G/diff/prefix/lib/node_modules/@lukaso/sheepdog/bin/sheepdog"
gate "a launcher that differs from the package's" refuse "$G/diff" "$INSTS" "$G/diff/prefix/bin/sheepdog"
npm_home "$G/plain"; rm "$G/plain/prefix/bin/sheepdog" && printf '#!/bin/sh\nexit 0\n' > "$G/plain/prefix/bin/sheepdog"
gate "a PATH entry that is neither a link nor a pnpm shim" refuse "$G/plain" "$INSTS" "$G/plain/prefix/bin/sheepdog"
npm_home "$G/none"; rm -rf "$G/none/prefix/lib/node_modules/@lukaso/sheepdog/node_modules"
gate "an install that resolves to nothing" refuse "$G/none" "$INSTS" "$G/none/prefix/bin/sheepdog"
npm_home "$G/idle"; pkgdir "$G/idle/cache/@lukaso/sheepdog"
gate "a good install beside a copy that resolves to nothing" pass "$G/idle" "$INSTS" "$G/idle/prefix/bin/sheepdog"

# --- runs_one_of
printf '#include <stdlib.h>\n#include <unistd.h>\nint main(int c,char**v){sleep(c>1?atoi(v[1]):3);return 0;}\n' > "$FX/s.c"
mkdir -p "$FX/j/Sheepdog.app/Contents/MacOS" "$FX/decoy/Sheepdog.app/Contents/MacOS"
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
"$FX/wrap" 5 & w=$!; PIDS="$PIDS $w"; cw $w '[ -n "$(pgrep -P $w)" ]'; k=$(pgrep -P $w | head -1)
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
finish
