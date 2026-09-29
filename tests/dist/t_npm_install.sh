#!/bin/sh
# PHASE3.md S5, cell 18 and D9, with a .dev bundle (the staple is covered in the rc leg): the four
# npm tarballs, packed as release.sh packs them around a release build of sheepdog, installed
# globally with npm, pnpm (pinned: a copy of the corepack cache, no network) and bun, each into a
# temp prefix with a temp HOME, from a static registry on 127.0.0.1 (no `npm publish`, no
# registry software). For each manager:
#   - the installed executable is the local tarball's (byte for byte), and the door allows it;
#   - the process started through the PATH entry is the bundle's executable (same pid: the sh
#     launcher execs), and TERM to that pid ends the job's whole tree, a setsid escapee included;
#   - the job's signal dispositions, mask and environment equal a direct run of the bundle (pnpm:
#     but NODE_PATH, a stated limit), for three callers;
#   - no installed file is group- or world-writable (npm, pnpm; bun's modes are recorded: a limit).
# Controls: a wrapper that spawns instead of exec'ing fails the process check; a registry without
# the platform package installs no bundle, and the launcher names the missing package.
set -u
. "$(dirname "$0")/lib.sh"
[ "$(uname -s)" = Darwin ] || { echo "SKIP (macOS only)"; exit 0; }
fx_dir
V=0.1.0-rc.1
(cd "$SD_ROOT" && env CARGO_TARGET_DIR="$FX/target" timeout 1200 cargo build -q --release --locked --bin sheepdog) || { fail "release build"; finish; }
app=$("$SD_ROOT/scripts/bundle.sh" "$FX/target/release/sheepdog" "$FX/bundle" 0.1.0 1) || { fail "bundle"; finish; }
EXE=$app/Contents/MacOS/sheepdog
"$SD_ROOT/scripts/lib/exec-guard.sh" check "$EXE" || { fail "the door refuses the .dev bundle"; finish; }

# the packages, packed by scripts/lib/npm-pack.sh (what release.sh build runs)
mkdir -p "$FX/tgz" "$FX/lin"
"$SD_ROOT/scripts/lib/archive.sh" make "$app" "$FX/lin/sheepdog-macos-universal.tar.gz" || { fail "archive"; finish; }
printf '#!/bin/sh\n' > "$FX/lin/a"; printf '#!/bin/sh\n' > "$FX/lin/x"
sh "$SD_ROOT/scripts/lib/npm-pack.sh" "$V" "$FX/lin/sheepdog-macos-universal.tar.gz" "$FX/lin/a" "$FX/lin/x" "$SD_ROOT/npm/sheepdog/bin/sheepdog" "$FX/tgz" >/dev/null \
  || { fail "npm-pack.sh"; finish; }
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
trap 'kill $rp 2>/dev/null; rm -rf "$FX"' EXIT
i=0; until curl -fs "http://127.0.0.1:$port/@lukaso%2fsheepdog" >/dev/null 2>&1 || [ $i -gt 50 ]; do sleep 0.1; i=$((i + 1)); done
R=http://127.0.0.1:$port/

# the managers, each with a temp HOME, prefix, cache and userconfig (only the registry)
cp -R "$HOME/.cache/node/corepack" "$FX/corepack" 2>/dev/null || fail "no corepack cache to pin pnpm"
install() { # manager -> sets BIN (the PATH entry dir); rc
  m=$1 h=$FX/home-$1; mkdir -p "$h"; printf 'registry=%s\n' "$R" > "$h/.npmrc"
  case $m in
    npm) env HOME="$h" npm_config_userconfig="$h/.npmrc" npm_config_cache="$h/cache" npm i -g --prefix "$h/prefix" --ignore-scripts --registry "$R" "@lukaso/sheepdog@$V" > "$FX/inst.$m" 2>&1; rc=$?; BIN=$h/prefix/bin ;;
    pnpm) env HOME="$h" XDG_CONFIG_HOME="$h/xdg" COREPACK_HOME="$FX/corepack" COREPACK_ENABLE_NETWORK=0 PNPM_HOME="$h/pnpm" PATH="$h/pnpm:$PATH" \
            npm_config_userconfig="$h/.npmrc" pnpm add -g --ignore-scripts --registry "$R" "@lukaso/sheepdog@$V" > "$FX/inst.$m" 2>&1; rc=$?; BIN=$h/pnpm
          pv=$(env HOME="$h" COREPACK_HOME="$FX/corepack" COREPACK_ENABLE_NETWORK=0 pnpm -v 2>/dev/null)
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

for m in npm pnpm bun; do
  if ! install $m; then fail "$m: install failed: $(tail -3 "$FX/inst.$m" | tr '\n' ' ')"; continue; fi
  e=$BIN/sheepdog
  [ -e "$e" ] || { fail "$m: no PATH entry at $e"; continue; }
  inst=$(find "$FX/home-$m" -path '*Sheepdog.app/Contents/MacOS/sheepdog' -type f | head -1)
  [ -n "$inst" ] && cmp -s "$inst" "$EXE" && pass "$m: the installed executable is the local tarball's" || fail "$m: installed executable differs or missing ($inst)"
  "$SD_ROOT/scripts/lib/exec-guard.sh" check "$inst" && pass "$m: the door allows the installed bundle" || fail "$m: the door refuses the installed bundle"
  rm -f "$FX/esc"
  # env in the background directly (a function in the background is a forked subshell, whose pid
  # is not the launched process's)
  env -i PATH="$JP" HOME="$RH" XDG_STATE_HOME="$RH/x" SHEEPDOG_STATE="$RH/s" "$e" run --no-sweep -- perl "$FX/escape.pl" "$FX/esc" >/dev/null 2>&1 & jp=$!
  i=0; while [ ! -s "$FX/esc" ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
  comm=$(ps -o comm= -p $jp 2>/dev/null)
  case $comm in *Sheepdog.app/Contents/MacOS/sheepdog) pass "$m: the PATH entry's process is the bundle's executable (pid $jp)" ;; *) fail "$m: pid $jp is '$comm'" ;; esac
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

# control: a wrapper that spawns the executable (no exec) is caught by the process check
printf '#!/bin/sh\n"%s" "$@"\n' "$EXE" > "$FX/wrap"; chmod 755 "$FX/wrap"
env -i PATH="$JP" HOME="$RH" XDG_STATE_HOME="$RH/x" SHEEPDOG_STATE="$RH/s" "$FX/wrap" run --no-sweep -- sh -c 'sleep 2' >/dev/null 2>&1 & wp=$!; sleep 0.7
case $(ps -o comm= -p $wp 2>/dev/null) in *Sheepdog.app/Contents/MacOS/sheepdog) fail "control: the spawning wrapper passed the process check" ;; *) pass "control: a spawning wrapper fails the process check" ;; esac
wait $wp 2>/dev/null
# control: no platform package in the registry
kill $rp 2>/dev/null; wait $rp 2>/dev/null
port2=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
reg "$FX/reg2" "$port2" skip
(cd "$FX/reg2" && exec python3 -m http.server "$port2" --bind 127.0.0.1) > "$FX/reg2.log" 2>&1 & rp=$!
i=0; until curl -fs "http://127.0.0.1:$port2/@lukaso%2fsheepdog" >/dev/null 2>&1 || [ $i -gt 50 ]; do sleep 0.1; i=$((i + 1)); done
R=http://127.0.0.1:$port2/
rm -rf "$FX/home-npm"; install npm
out=$(job "$FX/home-npm/prefix/bin/sheepdog" --version 2>&1); r=$?
[ $r = 1 ] && printf '%s' "$out" | grep -q '@lukaso/sheepdog-darwin-universal' && pass "control: no platform package: exit 1, the package named" || fail "control: no platform package: rc=$r $out"
finish
