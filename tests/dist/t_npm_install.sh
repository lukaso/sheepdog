#!/bin/sh
# PHASE3.md S5, cell 18 and D9, with a .dev bundle (with SD_NPM_RC_DIR: the rc's own packages and
# its stapled bundle, run by tests/rc/t_rc_npm.sh; nothing is built then): the four
# npm tarballs, packed as release.sh packs them around a release build of sheepdog, installed
# globally with npm, pnpm (pinned: 10.18.2 by its path in a copy of the corepack cache) and bun, each into a
# temp prefix with a temp HOME, from a static registry on 127.0.0.1 (no `npm publish`, no
# registry software). For each manager:
#   - the installed executable is the local tarball's (byte for byte), and the door allows it;
#   - before anything runs, every installed copy of the executable is the tarball's and has the
#     exec door's yes, and tests/lib/npm-gates.sh's npm_gate passes: the launcher the PATH entry
#     reaches, and every installed copy of it, is the package's launcher (this tree's: the gate's
#     copy of its resolution is checked against it in t_npm_gates.sh) and resolves to one of those
#     judged files; else nothing of that manager runs; with SD_NPM_RC_DIR every installed bundle
#     is the release archive's, file by file (tests/lib/tree-same.py), and its staple validates;
#   - the process started through the PATH entry runs one of those judged files (lsof's txt entry;
#     same pid: the sh launcher execs), and TERM to that pid ends the job's whole tree, a setsid
#     escapee included;
#   - the job's signal dispositions, mask and environment equal a direct run of the bundle (pnpm:
#     but NODE_PATH, a stated limit), for three callers;
#   - no installed file is group- or world-writable (npm, pnpm; bun's modes are recorded: a limit).
# Controls: the executable itself passes the process check (its other controls, and the gate's,
# are t_npm_gates.sh's); a registry without the platform package installs no bundle (checked
# before the launcher runs), and the launcher names the missing package.
set -u
. "$(dirname "$0")/lib.sh"
[ "$(uname -s)" = Darwin ] || { echo "SKIP (macOS only)"; exit 0; }
fx_dir
mkdir -p "$FX/tgz" "$FX/lin"
if [ -n "${SD_NPM_RC_DIR:-}" ]; then
  # the rc leg: the release's own four packages (read only: copied), its darwin bundle the one to
  # compare with; nothing is built
  V=$(basename "$SD_NPM_RC_DIR"); V=${V#v}
  for p in sheepdog sheepdog-darwin-universal sheepdog-linux-arm64 sheepdog-linux-x64; do
    cp "$SD_NPM_RC_DIR/lukaso-$p-$V.tgz" "$FX/tgz/" || { fail "no lukaso-$p-$V.tgz in $SD_NPM_RC_DIR"; finish; }
  done
  mkdir -p "$FX/rcb" && tar -xzf "$FX/tgz/lukaso-sheepdog-darwin-universal-$V.tgz" -C "$FX/rcb" || { fail "cannot unpack the darwin package"; finish; }
  app=$FX/rcb/package/Sheepdog.app
  mkdir -p "$FX/rca" && tar -xzf "$SD_NPM_RC_DIR/sheepdog-macos-universal.tar.gz" -C "$FX/rca" || { fail "cannot unpack the release archive"; finish; }
else
  V=0.1.0-rc.1
  (cd "$SD_ROOT" && env CARGO_TARGET_DIR="$FX/target" timeout 1200 cargo build -q --release --locked --bin sheepdog) || { fail "release build"; finish; }
  app=$("$SD_ROOT/scripts/bundle.sh" "$FX/target/release/sheepdog" "$FX/bundle" 0.1.0 1) || { fail "bundle"; finish; }
fi
EXE=$app/Contents/MacOS/sheepdog
"$SD_ROOT/scripts/lib/exec-guard.sh" check "$EXE" || { fail "the door refuses the bundle"; finish; }

if [ -z "${SD_NPM_RC_DIR:-}" ]; then
  # the packages, packed by scripts/lib/npm-pack.sh (what release.sh build runs)
  "$SD_ROOT/scripts/lib/archive.sh" make "$app" "$FX/lin/sheepdog-macos-universal.tar.gz" || { fail "archive"; finish; }
  printf '#!/bin/sh\n' > "$FX/lin/a"; printf '#!/bin/sh\n' > "$FX/lin/x"
  sh "$SD_ROOT/scripts/lib/npm-pack.sh" "$V" "$FX/lin/sheepdog-macos-universal.tar.gz" "$FX/lin/a" "$FX/lin/x" "$SD_ROOT/npm/sheepdog/bin/sheepdog" "$FX/tgz" >/dev/null \
    || { fail "npm-pack.sh"; finish; }
fi
# a static registry: a packument per package, the tarballs beside
reg() { # dir port [skip-platform]
  python3 - "$1" "$2" "${3:-}" "$FX/tgz" <<'PY'
import sys, os, json, hashlib, base64, tarfile, shutil
d, port, skip, tg = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
os.makedirs(os.path.join(d, "@lukaso"), exist_ok=True); os.makedirs(os.path.join(d, "t"), exist_ok=True)
for f in sorted(os.listdir(tg)):
    if skip and "darwin" in f: continue
    b = open(os.path.join(tg, f), "rb").read()
    pj = json.load(tarfile.open(os.path.join(tg, f)).extractfile("package/package.json"))
    shutil.copy(os.path.join(tg, f), os.path.join(d, "t", f))
    v = dict(pj); v["dist"] = {"tarball": "http://127.0.0.1:%s/t/%s" % (port, f),
        "shasum": hashlib.sha1(b).hexdigest(), "integrity": "sha512-" + base64.b64encode(hashlib.sha512(b).digest()).decode()}
    doc = {"name": pj["name"], "dist-tags": {"latest": pj["version"]}, "versions": {pj["version"]: v}}
    json.dump(doc, open(os.path.join(d, pj["name"]), "w"))
PY
}
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
reg "$FX/reg" "$port"
(cd "$FX/reg" && exec python3 -m http.server "$port" --bind 127.0.0.1) > "$FX/reg.log" 2>&1 & rp=$!
trap '{ kill $rp; wait $rp; } 2>/dev/null; rm -rf "$FX"' EXIT
i=0; until curl -fs "http://127.0.0.1:$port/@lukaso%2fsheepdog" >/dev/null 2>&1 || [ $i -gt 50 ]; do sleep 0.1; i=$((i + 1)); done
R=http://127.0.0.1:$port/

# the managers, each with a temp HOME, prefix, cache and userconfig (only the registry)
# pnpm: the pinned version, run by its path in a copy of the operator's corepack cache, never the
# `pnpm` on PATH (that has been a corepack shim here, and Homebrew's 10.2.1 after a reboot)
cp -R "$HOME/.cache/node/corepack" "$FX/corepack" 2>/dev/null || fail "no corepack cache to pin pnpm"
PN=$FX/corepack/v1/pnpm/10.18.2/bin/pnpm.cjs
[ -f "$PN" ] || fail "no pnpm 10.18.2 in the corepack cache ($PN)"
install() { # manager -> sets BIN (the PATH entry dir); rc
  m=$1 h=$FX/home-$1; mkdir -p "$h"; printf 'registry=%s\n' "$R" > "$h/.npmrc"
  case $m in
    npm) env HOME="$h" npm_config_userconfig="$h/.npmrc" npm_config_cache="$h/cache" npm i -g --prefix "$h/prefix" --ignore-scripts --registry "$R" "@lukaso/sheepdog@$V" > "$FX/inst.$m" 2>&1; rc=$?; BIN=$h/prefix/bin ;;
    pnpm) env HOME="$h" XDG_CONFIG_HOME="$h/xdg" COREPACK_ENABLE_NETWORK=0 PNPM_HOME="$h/pnpm" PATH="$h/pnpm:$PATH" \
            npm_config_userconfig="$h/.npmrc" node "$PN" add -g --ignore-scripts --registry "$R" "@lukaso/sheepdog@$V" > "$FX/inst.$m" 2>&1; rc=$?; BIN=$h/pnpm
          pv=$(env HOME="$h" COREPACK_ENABLE_NETWORK=0 node "$PN" -v 2>/dev/null)
          [ "$pv" = 10.18.2 ] || fail "pnpm is $pv, not the pinned 10.18.2" ;;
    bun) env HOME="$h" XDG_CONFIG_HOME="$h/xdg" BUN_INSTALL="$h/bun" BUN_CONFIG_REGISTRY="$R" bun add -g --ignore-scripts "@lukaso/sheepdog@$V" > "$FX/inst.$m" 2>&1; rc=$?; BIN=$h/bun/bin ;;
  esac
  return $rc
}
cat > "$FX/probe.c" <<'C'
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
extern char **environ;
static int cmp(const void *a, const void *b) { return strcmp(*(char *const *)a, *(char *const *)b); }
int main(void) {
  sigset_t m; sigprocmask(SIG_BLOCK, NULL, &m);
  for (int s = 1; s < 32; s++) { struct sigaction a; if (sigaction(s, NULL, &a)) continue;
    printf("sig %d %s %s\n", s, a.sa_handler == SIG_IGN ? "ign" : "dfl", sigismember(&m, s) ? "blocked" : "-"); }
  for (int f = 0; f < 3; f++) { int fl = fcntl(f, F_GETFL); printf("fd %d nonblock %d\n", f, fl >= 0 && (fl & O_NONBLOCK) ? 1 : 0); }
  int n = 0; while (environ[n]) n++; qsort(environ, n, sizeof *environ, cmp);
  for (int i = 0; i < n; i++) if (strncmp(environ[i], "_=", 2) && strncmp(environ[i], "SHLVL=", 6)) printf("env %s\n", environ[i]);
  return 0;
}
C
cc -o "$FX/probe" "$FX/probe.c" || exit 3
cat > "$FX/escape.pl" <<'P'
use POSIX; my $f = shift;
my $p = fork(); if ($p == 0) { POSIX::setsid() != -1 or die; my $q = fork();
  if ($q == 0) { my $s = `LC_ALL=C ps -o lstart= -p $$`; chomp $s; open(my $h, '>', "$f.t"); print $h "$$ $s\n"; close $h; rename "$f.t", $f; sleep 300; exit 0 } exit 0 }
waitpid($p, 0); sleep 300;
P
RH=$FX/rh; mkdir -p "$RH"
# the job's PATH has node's directory too, as a user's would (so a node launcher could run: the
# D9 mutant must fail on the signal state, not on a missing node)
JP=/usr/bin:/bin:$(dirname "$(command -v node)")
job() { env -i PATH="$JP" HOME="$RH" XDG_STATE_HOME="$RH/x" SHEEPDOG_STATE="$RH/s" "$@"; }
callers() { # entry -> the probe's output for three callers, into $FX/c.<caller>
  for c in default ignore block; do
    case $c in
      default) job sh -c 'exec "$@"' sh "$1" run --no-sweep -- "$FX/probe" ;;
      ignore) job sh -c 'trap "" HUP INT TERM; exec "$@"' sh "$1" run --no-sweep -- "$FX/probe" ;;
      block) job perl -MPOSIX -e 'sigprocmask(SIG_BLOCK, POSIX::SigSet->new(SIGTERM, SIGUSR1)) or die; exec @ARGV' "$1" run --no-sweep -- "$FX/probe" ;;
    esac 2>/dev/null | cat > "$FX/c.$c"
  done
}
callers "$EXE"; for c in default ignore block; do cp "$FX/c.$c" "$FX/d.$c"; done
[ -s "$FX/d.default" ] && ! cmp -s "$FX/d.default" "$FX/d.ignore" && pass "control: the callers differ in a direct run" || fail "control: the callers do not differ"

. "$SD_ROOT/tests/lib/npm-gates.sh"
# the launcher as the sheepdog package holds it (the gate judges each install against it)
mkdir -p "$FX/lp" && tar -xzf "$FX/tgz/lukaso-sheepdog-$V.tgz" -C "$FX/lp" package/bin/sheepdog && cp "$FX/lp/package/bin/sheepdog" "$FX/launcher" \
  || { fail "no launcher in lukaso-sheepdog-$V.tgz"; finish; }
cmp -s "$FX/launcher" "$SD_ROOT/npm/sheepdog/bin/sheepdog" \
  || { fail "the package's launcher is not this tree's npm/sheepdog/bin/sheepdog (t_npm_gates.sh proves the gate against this tree's): run the cell from the package's commit"; finish; }
for m in npm pnpm bun; do
  if ! install $m; then fail "$m: install failed: $(tail -3 "$FX/inst.$m" | tr '\n' ' ')"; continue; fi
  e=$BIN/sheepdog
  [ -e "$e" ] || { fail "$m: no PATH entry at $e"; continue; }
  # every installed copy of the bundle (bun keeps one in its cache too) is the tarball's and has
  # the door's yes, or nothing of this manager's install is run; the process row below then
  # checks the launcher ran one of these very files
  insts=$(find "$FX/home-$m" -path '*Sheepdog.app/Contents/MacOS/sheepdog' -type f | while IFS= read -r f; do printf '%s/%s\n' "$(cd -P "$(dirname "$f")" && pwd -P)" "$(basename "$f")"; done | sort -u)
  [ -n "$insts" ] || { fail "$m: no installed bundle"; continue; }
  ok=yes
  for inst in $insts; do
    cmp -s "$inst" "$EXE" || { fail "$m: an installed executable differs ($inst)"; ok=no; }
    "$SD_ROOT/scripts/lib/exec-guard.sh" check "$inst" || { fail "$m: the door refuses $inst"; ok=no; }
  done
  [ $ok = yes ] || continue
  pass "$m: every installed copy of the executable ($(printf '%s\n' "$insts" | grep -c .)) is the local tarball's, and the door allows it"
  # the launcher's own target (npm/sheepdog/bin/sheepdog's resolution, for every installed copy of
  # the launcher) must be one of the judged files before anything runs
  if ! npm_gate "$FX/home-$m" "$insts" "$e" "$FX/launcher" > "$FX/gate"; then
    while IFS= read -r l; do fail "$m: $l"; done < "$FX/gate"; continue
  fi
  if [ -n "${SD_NPM_RC_DIR:-}" ]; then
    for inst in $insts; do
    # the whole installed bundle is the release archive's (paths, types, modes, content), stapled
    ia=${inst%/Contents/MacOS/sheepdog}
    python3 "$SD_ROOT/tests/lib/tree-same.py" "$ia" "$FX/rca/Sheepdog.app" || { fail "$m: an installed bundle differs from the release archive's ($ia)"; ok=no; }
    env -u DEVELOPER_DIR -u SDKROOT -u TOOLCHAINS /usr/bin/xcrun stapler validate "$ia" >/dev/null 2>&1 || { fail "$m: no valid staple ticket on $ia"; ok=no; }
    done
    [ $ok = yes ] || continue
    pass "$m: every installed bundle is the release archive's, file by file, and its staple ticket validates"
  fi
  rm -f "$FX/esc"
  # env in the background directly (a function in the background is a forked subshell, whose pid
  # is not the launched process's)
  env -i PATH="$JP" HOME="$RH" XDG_STATE_HOME="$RH/x" SHEEPDOG_STATE="$RH/s" "$e" run --no-sweep -- perl "$FX/escape.pl" "$FX/esc" >/dev/null 2>&1 & jp=$!
  i=0; while [ ! -s "$FX/esc" ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
  if runs_one_of $jp "$insts"; then pass "$m: the PATH entry's process runs one of the bundle executables the door judged (pid $jp)"
  else fail "$m: pid $jp runs '$(/usr/sbin/lsof -a -p $jp -d txt -Fn 2>/dev/null | sed -n 's/^n//p' | head -1)', not one the door judged"
    kill -TERM $jp 2>/dev/null; wait $jp 2>/dev/null; continue; fi
  ep=$(cut -d' ' -f1 "$FX/esc" 2>/dev/null); es=$(cut -d' ' -f2- "$FX/esc" 2>/dev/null)
  [ -n "$ep" ] && [ "$(LC_ALL=C ps -o lstart= -p "$ep" 2>/dev/null)" = "$es" ] && pass "$m: the escapee is alive before the TERM" || fail "$m: the escapee did not start (record '$(cat "$FX/esc" 2>/dev/null)', ps '$(ps -o lstart= -p "$ep" 2>/dev/null)')"
  kill -TERM $jp 2>/dev/null; wait $jp 2>/dev/null; sleep 0.5
  if [ -n "$ep" ] && [ "$(LC_ALL=C ps -o lstart= -p "$ep" 2>/dev/null)" = "$es" ]; then fail "$m: TERM left the escapee alive"; kill -KILL "$ep" 2>/dev/null
  else pass "$m: TERM to the PATH entry's process ended the tree, the escapee included"; fi
  callers "$e"
  for c in default ignore block; do
    # SHEEPDOG_OUTER names each job's own registration socket: its presence counts, not its value
    # (PWD: sh, the launcher, adds it for a caller that had none, a stated limit of D9)
    x='s/^env SHEEPDOG_OUTER=.*/env SHEEPDOG_OUTER=(set)/; /^env PWD=/d'
    if [ $m = pnpm ]; then sed "$x" "$FX/c.$c" | grep -v '^env NODE_PATH=' > "$FX/c2"; sed "$x" "$FX/d.$c" | grep -v '^env NODE_PATH=' > "$FX/d2"
    else sed "$x" "$FX/c.$c" > "$FX/c2"; sed "$x" "$FX/d.$c" > "$FX/d2"; fi
    cmp -s "$FX/c2" "$FX/d2" && pass "$m, $c caller: the job's signals, mask, fds and environment equal a direct run" || fail "$m, $c caller: differs: $(diff "$FX/d2" "$FX/c2" | head -3 | tr '\n' ' ')"
  done
  ww=$(find "$FX/home-$m" -path '*@lukaso*' \( -perm -g+w -o -perm -o+w \) -type f 2>/dev/null | head -3)
  if [ $m = bun ]; then echo "note: bun's installed modes: $(find "$FX/home-$m" -path '*@lukaso*' -name sheepdog -type f -exec stat -f %Sp {} \; | head -2 | tr '\n' ' ')"
  else [ -z "$ww" ] && pass "$m: no installed file is group- or world-writable" || fail "$m: writable: $ww"; fi
done

# control of the process check (runs_one_of, the rows' own function) on the bundle's own
# executable: it passes (the controls that must fail are t_npm_gates.sh's)
ER=$(cd -P "$(dirname "$EXE")" && pwd -P)/sheepdog
env -i PATH="$JP" HOME="$RH" XDG_STATE_HOME="$RH/x" SHEEPDOG_STATE="$RH/s" "$EXE" run --no-sweep -- sh -c 'sleep 2' >/dev/null 2>&1 & wp=$!; sleep 0.7
runs_one_of $wp "$ER" && pass "control: the executable itself passes the process check" || fail "control: the executable itself fails the process check"
wait $wp 2>/dev/null
# control: no platform package in the registry
kill $rp 2>/dev/null; wait $rp 2>/dev/null
port2=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
reg "$FX/reg2" "$port2" skip
(cd "$FX/reg2" && exec python3 -m http.server "$port2" --bind 127.0.0.1) > "$FX/reg2.log" 2>&1 & rp=$!
i=0; until curl -fs "http://127.0.0.1:$port2/@lukaso%2fsheepdog" >/dev/null 2>&1 || [ $i -gt 50 ]; do sleep 0.1; i=$((i + 1)); done
R=http://127.0.0.1:$port2/
rm -rf "$FX/home-npm"; install npm
# the launcher runs only if nothing it could exec was installed
nb=$(find "$FX/home-npm" \( -name Sheepdog.app -o -name 'sheepdog-darwin-universal*' \) | head -3)
if [ -n "$nb" ]; then fail "control: no platform package in the registry, but npm installed $nb (the launcher is not run)"
else
  out=$(job "$FX/home-npm/prefix/bin/sheepdog" --version 2>&1); r=$?
  [ $r = 1 ] && printf '%s' "$out" | grep -q '@lukaso/sheepdog-darwin-universal' && pass "control: no platform package: none installed, exit 1, the package named" || fail "control: no platform package: rc=$r $out"
fi
finish
