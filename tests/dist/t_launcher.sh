#!/bin/sh
# PHASE3.md D9: the npm launcher (npm/sheepr/bin/sheepr) is POSIX sh and execs the platform
# executable. Through it the process keeps its pid (cell 18), and the whole signal disposition
# table, the signal mask, O_NONBLOCK on fds 0-2 and the environment equal a direct run, for three
# callers (default; HUP INT TERM ignored; TERM USR1 blocked), with stdout a pipe. Both npm layouts
# (the platform package nested under the main one, or hoisted beside it), reached through a PATH
# symlink as npm makes it. No platform package: exit 1 and the missing package named.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
L="$SR_ROOT/npm/sheepr/bin/sheepr"
case $(uname -s) in
  Darwin) plat=darwin-universal exe=Sheepr.app/Contents/MacOS/sheepr ;;
  Linux) case $(uname -m) in aarch64|arm64) plat=linux-arm64 ;; *) plat=linux-x64 ;; esac; exe=bin/sheepr ;;
esac
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
  printf("pid %d\n", (int)getpid());
  for (int s = 1; s < 32; s++) {
    struct sigaction a; if (sigaction(s, NULL, &a) != 0) continue;
    printf("sig %d %s %s\n", s, a.sa_handler == SIG_IGN ? "ign" : a.sa_handler == SIG_DFL ? "dfl" : "handler", sigismember(&m, s) ? "blocked" : "-");
  }
  for (int f = 0; f < 3; f++) { int fl = fcntl(f, F_GETFL); printf("fd %d nonblock %d\n", f, fl >= 0 && (fl & O_NONBLOCK) ? 1 : 0); }
  int n = 0; while (environ[n]) n++;
  qsort(environ, n, sizeof *environ, cmp);
  for (int i = 0; i < n; i++) if (strncmp(environ[i], "_=", 2)) printf("env %s\n", environ[i]);
  return 0;
}
C
cc -o "$FX/probe" "$FX/probe.c" || exit 3

# layouts: prefix/bin/sheepr -> ../lib/node_modules/sheepr/bin/sheepr (npm's link)
mk_main() { # prefix
  m="$1/lib/node_modules/sheepr"; mkdir -p "$m/bin" "$1/bin"
  cp "$L" "$m/bin/sheepr" && chmod 755 "$m/bin/sheepr"
  ln -s ../lib/node_modules/sheepr/bin/sheepr "$1/bin/sheepr"
}
mk_plat() { # dir-of-platform-package
  mkdir -p "$1/$(dirname "$exe")"; cp "$FX/probe" "$1/$exe"
  [ "$plat" = darwin-universal ] && "$SR_ROOT/scripts/bundle.sh" "$FX/probe" "$FX/tmpb.$$" 0.1.0 1 >/dev/null && rm -rf "$1/Sheepr.app" && mv "$FX/tmpb.$$/Sheepr.app" "$1/Sheepr.app"
  "$SR_ROOT/scripts/lib/exec-guard.sh" check "$1/$exe" || { fail "the door refused the fixture"; exit 1; }
}
mk_main "$FX/nested"; mk_plat "$FX/nested/lib/node_modules/sheepr/node_modules/sheepr-$plat"
mk_main "$FX/hoisted"; mk_plat "$FX/hoisted/lib/node_modules/sheepr-$plat"
mk_main "$FX/none"

# caller: default | ignore | block. Runs `sh -c 'echo shpid $$; exec "$@"'` so the probe's pid
# must equal the shell's; stdout is a pipe (through cat).
run() { # caller program
  case $1 in
    default) sh -c 'echo "shpid $$"; exec "$@"' sh "$2" ;;
    ignore) sh -c 'trap "" HUP INT TERM; echo "shpid $$"; exec "$@"' sh "$2" ;;
    block) perl -MPOSIX -e 'sigprocmask(SIG_BLOCK, POSIX::SigSet->new(SIGTERM, SIGUSR1)) or die; print "shpid $$\n"; $|=1; exec @ARGV or die' "$2" ;;
  esac | cat
}
state() { grep -v '^pid \|^shpid ' ; }
for layout in nested hoisted; do
  for caller in default ignore block; do
    run "$caller" "$FX/probe" > "$FX/direct"
    run "$caller" "$FX/$layout/bin/sheepr" > "$FX/via"
    sp=$(sed -n 's/^shpid //p' "$FX/via"); pp=$(sed -n 's/^pid //p' "$FX/via")
    [ -n "$pp" ] && [ "$sp" = "$pp" ] && pass "$layout/$caller: pid kept" || fail "$layout/$caller: pid $sp -> $pp"
    if state < "$FX/direct" > "$FX/d2" && state < "$FX/via" > "$FX/v2" && [ -s "$FX/d2" ] && cmp -s "$FX/d2" "$FX/v2"; then
      pass "$layout/$caller: signals, mask, fds and environment equal a direct run"
    else fail "$layout/$caller: differs: $(diff "$FX/d2" "$FX/v2" | head -4 | tr '\n' ' ')"; fi
  done
done
# the callers really differ (else the equality proves nothing)
run ignore "$FX/probe" | grep -q '^sig 15 ign' && run block "$FX/probe" | grep -q '^sig 15 dfl blocked' \
  && pass "the three callers differ" || fail "the callers do not set what they claim"
# pnpm's layout: the package in the .pnpm store, its platform package a sibling symlink there, and
# the global link pointing through node_modules/sheepr (itself a symlink into the store)
P="$FX/pnpm/global/5"
st="$P/node_modules/.pnpm/sheepr@0.1.0/node_modules"
mkdir -p "$st/sheepr/bin" "$FX/pnpm/bin"
cp "$L" "$st/sheepr/bin/sheepr" && chmod 755 "$st/sheepr/bin/sheepr"
mk_plat "$P/node_modules/.pnpm/sheepr-$plat@0.1.0/node_modules/sheepr-$plat"
ln -s "../../sheepr-$plat@0.1.0/node_modules/sheepr-$plat" "$st/sheepr-$plat"
ln -s .pnpm/sheepr@0.1.0/node_modules/sheepr "$P/node_modules/sheepr"
ln -s ../global/5/node_modules/sheepr/bin/sheepr "$FX/pnpm/bin/sheepr"
run default "$FX/pnpm/bin/sheepr" > "$FX/via"
sp=$(sed -n 's/^shpid //p' "$FX/via"); pp=$(sed -n 's/^pid //p' "$FX/via")
[ -n "$pp" ] && [ "$sp" = "$pp" ] && pass "pnpm layout: resolved, pid kept" || fail "pnpm layout: pid $sp -> $pp"

# the caller's variables that share the launcher's names reach the job unchanged
for layout in nested hoisted; do
  env -i PATH=/usr/bin:/bin me=M n=N l=L pkg=P plat=Q exe=E dir=D target=T "$FX/$layout/bin/sheepr" > "$FX/via"
  got=$(grep -E '^env (me|n|l|pkg|plat|exe|dir|target)=' "$FX/via" | sort | tr '\n' ' ')
  [ "$got" = "env dir=D env exe=E env l=L env me=M env n=N env pkg=P env plat=Q env target=T " ] && pass "$layout: caller variables with the launcher's names unchanged" || fail "$layout: $got"
done

# no platform package
"$FX/none/bin/sheepr" --version > "$FX/out" 2>&1; rc=$?
[ $rc = 1 ] && grep -q "sheepr-$plat" "$FX/out" && pass "missing platform package: exit 1, named" || fail "missing package: rc=$rc $(head -1 "$FX/out")"
# npm 7 and later spell it --omit=optional (--no-optional is the old spelling)
grep -q -- '--omit=optional' "$FX/out" && ! grep -q -- '--no-optional' "$FX/out" && pass "missing platform package: the message names --omit=optional" || fail "missing package message: $(head -1 "$FX/out")"
finish
