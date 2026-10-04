#!/bin/sh
# A smoke test of an installed sheepr (PHASE3.md S7): needs no repo and no cargo.
#   smoke.sh [SHEEPR]      (default: the sheepr on PATH)
# It first names what it tests (the resolved path and `--version`). Cell 1: the command's exit
# code passes through. Cell 3: an escapee (a fork, a new session, a fork again) leaves smoke's
# process group, is alive when it records itself (its start time is read from the live process),
# and is gone when the job ends, in well under its 300 s lifetime (so a kill, not a wait). Control:
# the same escapee without sheepr survives. Exit 0 only if all three hold. Processes are
# identified by pid and start time, never by name; every escapee still alive at exit (or on INT,
# TERM or HUP) is killed by that identity. The escapee's session id is not read (macOS `ps` has
# no session id); its process group, which setsid also changes, is.
set -u
SD=${1:-sheepr}
t=$(mktemp -d "${TMPDIR:-/tmp}/sr-smoke.XXXXXX") || exit 1
fails=0
ok() { echo "ok: $*"; }
bad() { echo "FAIL: $*"; fails=$((fails + 1)); }
start() { # pid -> its start time (Linux: /proc; macOS: ps, in the C locale)
  if [ -r "/proc/$1/stat" ]; then sed 's/.*) //' "/proc/$1/stat" | cut -d' ' -f20
  else LC_ALL=C ps -o lstart= -p "$1" 2>/dev/null; fi
}
rec() { # record-file -> p, s, g; 0 only if complete (a numeric pid and a start time)
  p=$(sed -n 1p "$1" 2>/dev/null); s=$(sed -n 2p "$1" 2>/dev/null); g=$(sed -n 3p "$1" 2>/dev/null)
  case $p in ''|*[!0-9]*) return 1 ;; esac
  [ -n "$s" ]
}
alive() { rec "$1" && [ "$(start "$p")" = "$s" ]; }
reap() { for f in "$t/e1" "$t/e2"; do alive "$f" && kill -KILL "$p" 2>/dev/null; done; }
trap 'reap; rm -rf "${t:?}"' EXIT
trap 'exit 129' HUP; trap 'exit 130' INT; trap 'exit 143' TERM
pgid() { # pid -> its process group (Linux: /proc, which busybox and slim images have; macOS: ps)
  if [ -r "/proc/$1/stat" ]; then sed 's/.*) //' "/proc/$1/stat" | cut -d' ' -f3
  else ps -o pgid= -p "$1" | tr -d ' '; fi
}
me=$(pgid $$)
# the escapee: fork, a new session, fork again. The grandchild writes its own record (pid, start
# time, process group) before it becomes `sleep 300` (exec keeps the pid and the start time), so
# every escapee that runs has a record for reap; with no record (a read or the write failed) it exits
cat > "$t/me.sh" <<'E'
f=$1 p=$$
if [ -r "/proc/$p/stat" ]; then st=$(sed 's/.*) //' "/proc/$p/stat"); s=$(echo "$st" | cut -d' ' -f20); g=$(echo "$st" | cut -d' ' -f3)
else s=$(LC_ALL=C ps -o lstart= -p "$p"); g=$(ps -o pgid= -p "$p" | tr -d ' '); fi
# no record, no escapee: a process reap cannot find must not be left behind
[ -n "$s" ] && [ -n "$g" ] && printf '%s\n%s\n%s\n' "$p" "$s" "$g" > "$f.w" && mv "$f.w" "$f" || exit 1
exec sleep 300
E
cat > "$t/esc.sh" <<'E'
#!/bin/sh
f=$1 m=${1%/*}/me.sh
if command -v setsid >/dev/null 2>&1; then
  setsid sh -c 'sh "$1" "$0" &' "$f" "$m" &
else
  perl -MPOSIX -e 'my ($f, $m) = @ARGV; my $p = fork(); if ($p == 0) { POSIX::setsid() != -1 or die "setsid"; my $q = fork(); if ($q == 0) { exec "sh", $m, $f; } exit 0; } waitpid($p, 0);' "$f" "$m"
fi
i=0; while [ ! -s "$f" ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
E
w=$(command -v "$SD" 2>/dev/null) || w=$SD
echo "smoke: testing $w ($("$SD" --version < /dev/null 2>&1 | head -1))"
"$SD" run -- sh -c 'exit 7' >/dev/null 2>&1; rc=$?
[ $rc = 7 ] && ok "cell 1: the exit code passes through" || bad "cell 1: exit $rc, want 7"
# cell 3: the root starts the escapee and ends; sheepr must kill it, not wait for it
t0=$(date +%s)
"$SD" run --timeout 60s -- sh "$t/esc.sh" "$t/e1" >/dev/null 2>&1
el=$(($(date +%s) - t0))
if ! rec "$t/e1"; then bad "cell 3: the escapee did not start or could not record itself"
elif [ "$g" = "$me" ]; then bad "cell 3: the escapee did not leave smoke's process group ($g)"
elif alive "$t/e1"; then bad "cell 3: the escapee survived the job"
elif [ $el -ge 30 ]; then bad "cell 3: the job took ${el}s, so sheepr waited instead of killing (the escapee sleeps 300 s)"
else ok "cell 3: the escapee left smoke's process group and was killed with the job (${el}s)"; fi
# control: without sheepr it survives (reap kills it at exit)
sh "$t/esc.sh" "$t/e2"
if rec "$t/e2" && [ "$g" != "$me" ] && alive "$t/e2"; then ok "control: without sheepr the escapee survives"
else bad "control: the escapee did not start or record itself, did not escape, or did not survive without sheepr (the check proves nothing)"; fi
[ $fails = 0 ] && { echo "smoke: PASS"; exit 0; }
echo "smoke: FAIL ($fails)"; exit 1
