#!/bin/sh
# PHASE3.md S1, "Rules for every cell that runs a release or rc binary" (PHASE2.md §0.5 carried
# over): scripts/lib/release-run.sh HOME-DIR BINARY [SUBCOMMAND ARGS...] runs BINARY through the
# exec door with a cleared environment (no SHEEPDOG_TEST_*, no SHEEPDOG_OUTER), HOME,
# XDG_STATE_HOME and SHEEPDOG_STATE inside HOME-DIR, and --no-sweep added to `run`. It allows only
# run, doctor, ps, strays (not --kill), --version and help; it refuses kill, sweep, strays --kill
# and --inherit-terminal-permissions (before `--`). The binary here is a probe script that prints
# its arguments and environment; refused calls are checked through the door's recording seam.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
H="$SD_ROOT/scripts/lib/release-run.sh"
mkdir -p "$FX/home"
cat > "$FX/probe" <<'P'
#!/bin/sh
for a in "$@"; do echo "arg $a"; done
env | sed 's/^/env /'
P
chmod +x "$FX/probe"
out() { env SHEEPDOG_TEST_TAG=0123456789abcdef0123456789abcdef SHEEPDOG_TEST_STATE=/nope SHEEPDOG_OUTER=/sock \
  SHEEPDOG_STATE=/real/state XDG_STATE_HOME=/real/xdg HOME=/real/home "$H" "$FX/home" "$FX/probe" "$@" 2>"$FX/err"; }

o=$(out run -- true); rc=$?
[ $rc = 0 ] || fail "run: rc=$rc $(head -1 "$FX/err")"
echo "$o" | grep -q '^env SHEEPDOG_TEST_' && fail "a SHEEPDOG_TEST_ variable reached the binary" || pass "no SHEEPDOG_TEST_ variable"
echo "$o" | grep -q '^env SHEEPDOG_OUTER=' && fail "SHEEPDOG_OUTER reached the binary" || pass "no SHEEPDOG_OUTER"
for v in HOME XDG_STATE_HOME SHEEPDOG_STATE; do
  val=$(echo "$o" | sed -n "s/^env $v=//p")
  case $val in "$FX/home"*) pass "$v inside the temp home" ;; *) fail "$v is '$val'" ;; esac
done
[ "$(echo "$o" | sed -n 's/^arg //p' | head -2 | tr '\n' ' ')" = "run --no-sweep " ] && pass "--no-sweep added to run" || fail "args: $(echo "$o" | grep '^arg ' | tr '\n' ' ')"
o=$(out doctor); [ $? = 0 ] && echo "$o" | grep -q '^arg doctor$' && pass "doctor allowed" || fail "doctor refused"
o=$(out strays); [ $? = 0 ] && pass "strays allowed" || fail "strays refused"
o=$(out run -- sh -c 'x --inherit-terminal-permissions'); [ $? = 0 ] && pass "a flag after -- is the job's" || fail "a flag after -- refused"

refused() { # label args...
  l=$1; shift
  rec="$FX/rec"; : > "$rec"
  SD_EXEC_RECORD="$rec" "$H" "$FX/home" "$FX/probe" "$@" >/dev/null 2>&1; rc=$?
  [ $rc != 0 ] && [ "$(grep -c . "$rec")" = 0 ] && pass "$l refused" || fail "$l: rc=$rc recorded=$(grep -c . "$rec")"
}
refused kill kill 123
refused sweep sweep
refused "strays --kill" strays --kill
refused "strays --older-than 1h --kill" strays --older-than 1h --kill
refused "run --inherit-terminal-permissions" run --inherit-terminal-permissions -- true
refused "strays --cmd -- --kill" strays --cmd -- --kill
refused "strays --cmd -- --kill --yes" strays --cmd -- --kill --yes
refused "strays --older-than 1h --cmd -- --kill" strays --older-than 1h --cmd -- --kill
o=$(out run -- sh -c 'x --kill'); [ $? = 0 ] && pass "control: --kill after run -- is the job's" || fail "run -- ... --kill refused"
# HOME-DIR rules, in dry mode (SD_RELEASE_RUN_DRY=1 stops after the checks, before any mkdir,
# so even a broken rule writes nothing), with HOME set to a temp dir. Each rule has its own exit
# code: 4 = the real home or inside it, 5 = holds the real home, 3 = not under a temp root.
realhome=$(dscl . -read "/Users/$(id -un)" NFSHomeDirectory 2>/dev/null | sed -n 's/^NFSHomeDirectory: *//p')
[ -d "$realhome" ] || realhome=$(eval echo "~$(id -un)")
up=$(printf %s "$realhome" | tr '[:lower:]' '[:upper:]')
snap() { ls -A "$realhome" "$realhome/Library" 2>/dev/null | cksum; }
before=$(snap)
dry() { # want-rc home-dir label
  rec="$FX/rec"; : > "$rec"
  HOME="$FX/home" SD_RELEASE_RUN_DRY=1 SD_EXEC_RECORD="$rec" "$H" "$2" "$FX/probe" doctor >/dev/null 2>&1; rc=$?
  [ "$rc" = "$1" ] && [ "$(grep -c . "$rec")" = 0 ] && pass "$3: refused ($rc)" || fail "$3: rc=$rc, want $1"
}
dry 4 "$realhome" "the real home"
dry 4 "$realhome/Library" "inside the real home"
[ -d "$up/Library" ] && dry 4 "$up/Library" "inside the real home, upper-case spelling"
[ -d "/System/Volumes/Data$realhome/Library" ] && dry 4 "/System/Volumes/Data$realhome/Library" "inside the real home, the data-volume spelling"
dry 5 / "the root, which holds the real home"
dry 5 "$(dirname "$realhome")" "the folder that holds the real home"
dry 3 /usr "a folder outside every temp root"
HOME="$FX/home" SD_RELEASE_RUN_DRY=1 "$H" "$FX/home" "$FX/probe" doctor >/dev/null 2>&1 && pass "control: a temp HOME-DIR passes the checks" || fail "control: a temp HOME-DIR refused"
[ "$(snap)" = "$before" ] && pass "the real home gained no entries" || fail "the real home changed"
finish
