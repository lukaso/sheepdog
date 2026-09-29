#!/bin/sh
# A smoke test of an installed sheepdog (PHASE3.md S7): needs no repo and no cargo.
#   smoke.sh [SHEEPDOG]      (default: the sheepdog on PATH)
# Cell 1: the command's exit code passes through. Cell 3: an escapee (a double fork into its own
# session) is alive before the job ends and gone after it. Control: the same escapee without
# sheepdog survives (and is then killed here by its recorded identity). Exit 0 only if all three
# hold. Processes are identified by pid and start time, never by name.
set -u
SD=${1:-sheepdog}
t=$(mktemp -d "${TMPDIR:-/tmp}/sd-smoke.XXXXXX") || exit 1
trap 'rm -rf "$t"' EXIT
fails=0
ok() { echo "ok: $*"; }
bad() { echo "FAIL: $*"; fails=$((fails + 1)); }
start() { # pid -> its start time (Linux: /proc; macOS: ps, in the C locale)
  if [ -r "/proc/$1/stat" ]; then sed 's/.*) //' "/proc/$1/stat" | cut -d' ' -f20
  else LC_ALL=C ps -o lstart= -p "$1" 2>/dev/null; fi
}
alive() { # record-file -> 0 if that pid is alive with that start time
  p=$(sed -n 1p "$1"); s=$(sed -n 2p "$1")
  [ -n "$p" ] && [ -n "$s" ] && [ "$(start "$p")" = "$s" ]
}
# the escapee: fork, a new session, fork again; the grandchild records its pid and sleeps; the
# script then records the grandchild's start time while it lives (pid on line 1, start on line 2)
cat > "$t/esc.sh" <<'E'
#!/bin/sh
f=$1
if command -v setsid >/dev/null 2>&1; then
  setsid sh -c 'sh -c "echo \$\$ > \"$0.p\"; exec sleep 300" "$0" &' "$f" &
else
  perl -MPOSIX -e 'my $f = shift; my $p = fork(); if ($p == 0) { POSIX::setsid() != -1 or die "setsid"; my $q = fork(); if ($q == 0) { open(my $h, ">", "$f.p"); print $h "$$\n"; close $h; exec "sleep", "300"; } exit 0; } waitpid($p, 0);' "$f"
fi
i=0; while [ ! -s "$f.p" ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
p=$(cat "$f.p")
if [ -r "/proc/$p/stat" ]; then s=$(sed 's/.*) //' "/proc/$p/stat" | cut -d' ' -f20); else s=$(LC_ALL=C ps -o lstart= -p "$p"); fi
printf '%s\n%s\n' "$p" "$s" > "$f"
E
"$SD" run -- sh -c 'exit 7' >/dev/null 2>&1; rc=$?
[ $rc = 7 ] && ok "cell 1: the exit code passes through" || bad "cell 1: exit $rc, want 7"
# cell 3: the root starts the escapee and ends; sheepdog must kill it
"$SD" run -- sh "$t/esc.sh" "$t/e1" >/dev/null 2>&1
if [ -s "$t/e1" ]; then
  if alive "$t/e1"; then bad "cell 3: the escapee survived the job"; kill -KILL "$(sed -n 1p "$t/e1")" 2>/dev/null
  else ok "cell 3: the escapee is gone"; fi
else bad "cell 3: the escapee did not start"; fi
# control: without sheepdog it survives
sh "$t/esc.sh" "$t/e2"
if [ -s "$t/e2" ] && alive "$t/e2"; then ok "control: without sheepdog the escapee survives"; kill -KILL "$(sed -n 1p "$t/e2")" 2>/dev/null
else bad "control: the escapee did not survive without sheepdog (the check proves nothing)"; fi
[ $fails = 0 ] && { echo "smoke: PASS"; exit 0; }
echo "smoke: FAIL ($fails)"; exit 1
