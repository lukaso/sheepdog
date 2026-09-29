#!/bin/sh
# PHASE3.md §1.1: the exec door. Every fixture in tests/fixtures/exec-guard.tsv goes through the
# door with the recording seam, so nothing is ever run: an allowed file is recorded, a refused one
# leaves the recording empty. Also: a lying `codesign` first on PATH (with a test variable set)
# cannot answer for the door, which calls /usr/bin/codesign by absolute path.
set -u
. "$(dirname "$0")/lib.sh"
[ "$(uname -s)" = Darwin ] || { echo "SKIP (macOS only)"; exit 0; }
fx_dir
DOOR="$SD_ROOT/scripts/lib/exec-guard.sh"
door() { # expect path -> checks the verdict through the recording seam
  rec="$FX/rec.$$"; : > "$rec"
  SD_EXEC_RECORD="$rec" "$DOOR" exec "$2" --version >/dev/null 2>"$FX/err"; rc=$?
  n=$(grep -c . "$rec")
  case $1 in
    allow) [ "$rc" = 0 ] && [ "$n" = 1 ] && pass "$3 allowed" || fail "$3: want allow, rc=$rc recorded=$n ($(head -1 "$FX/err"))" ;;
    refuse) [ "$rc" != 0 ] && [ "$n" = 0 ] && pass "$3 refused" || fail "$3: want refuse, rc=$rc recorded=$n" ;;
  esac
}
grep -v '^#' "$SD_ROOT/tests/fixtures/exec-guard.tsv" > "$FX/table"
rows=0
while IFS="$(printf '\t')" read -r name verdict _install; do
  [ -n "$name" ] || continue
  p=$(fx_build "$name") || { fail "building $name"; continue; }
  door "$verdict" "$p" "$name"; rows=$((rows + 1))
done < "$FX/table"
[ "$rows" -ge 39 ] || fail "only $rows fixtures ran"

# liars: each command the door could call by name, alone first on PATH (it prints a harmless
# answer and succeeds), and each as an exported bash function; a test variable set. Every refuse
# row must stay refused, and (the control for each liar) an allowed file must still pass, so a
# refusal is not just the door breaking.
REFUSE=""
while IFS="$(printf '\t')" read -r name verdict _install; do
  [ "$verdict" = refuse ] || continue
  p=$(fx_build "$name") || continue
  REFUSE="$REFUSE $p"
done < "$FX/table"
OK=$(fx_build dev_bundle)
count() { /usr/bin/grep -c . "$1"; }
for c in codesign uname sed grep dirname basename readlink head lipo plutil launchctl tr od mkdir rm; do
  mkdir -p "$FX/liar-$c"
  printf '#!/bin/sh\necho "Identifier=com.example.harmless"\necho Linux\nexit 0\n' > "$FX/liar-$c/$c"; chmod +x "$FX/liar-$c/$c"
  bad=0
  for p in $REFUSE; do
    rec="$FX/rec.l"; : > "$rec"
    env PATH="$FX/liar-$c:$PATH" SHEEPDOG_TEST_TAG=0123456789abcdef SD_EXEC_RECORD="$rec" "$DOOR" exec "$p" >/dev/null 2>&1
    [ "$(count "$rec")" = 0 ] || { bad=1; echo "  $c on PATH let through: $p"; }
    : > "$rec"
    env "BASH_FUNC_$c%%=() { echo Identifier=com.example.harmless; echo Linux; return 0; }" SD_EXEC_RECORD="$rec" "$DOOR" exec "$p" >/dev/null 2>&1
    [ "$(count "$rec")" = 0 ] || { bad=1; echo "  function $c let through: $p"; }
  done
  [ $bad = 0 ] && pass "a lying $c (PATH and function) lets nothing through" || fail "a lying $c lets a release-ID file through"
  rec="$FX/rec.c"; : > "$rec"
  env PATH="$FX/liar-$c:$PATH" SD_EXEC_RECORD="$rec" "$DOOR" exec "$OK" >/dev/null 2>&1; a=$(count "$rec"); : > "$rec"
  env "BASH_FUNC_$c%%=() { echo Identifier=com.example.harmless; echo Linux; return 0; }" SD_EXEC_RECORD="$rec" "$DOOR" exec "$OK" >/dev/null 2>&1; b=$(count "$rec")
  [ "$a" = 1 ] && [ "$b" = 1 ] && pass "control: a lying $c does not break an allowed file" || fail "control: a lying $c breaks an allowed file (path $a, function $b)"
done

# started as `sh DOOR` and `bash DOOR` (no #! -p from the kernel), with SD_GUARD_P=1 preset and
# a lying uname function: still refused
for shell in sh bash; do
  bad=0
  for p in $REFUSE; do
    rec="$FX/rec.s"; : > "$rec"
    env SD_GUARD_P=1 "BASH_FUNC_uname%%=() { echo Linux; }" "BASH_FUNC_codesign%%=() { echo Identifier=x; }" \
      SD_EXEC_RECORD="$rec" $shell "$DOOR" exec "$p" >/dev/null 2>&1
    [ "$(count "$rec")" = 0 ] || { bad=1; echo "  $shell DOOR let through: $p"; }
  done
  [ $bad = 0 ] && pass "started as '$shell DOOR' with SD_GUARD_P preset: nothing through" || fail "'$shell DOOR' with SD_GUARD_P preset lets a file through"
done
# a lying DEVELOPER_DIR (lipo is a DEVELOPER_DIR trampoline): its fake lipo cannot answer
mkdir -p "$FX/fakedev/usr/bin"
printf '#!/bin/sh\nexit 1\n' > "$FX/fakedev/usr/bin/lipo"; chmod +x "$FX/fakedev/usr/bin/lipo"
for n in rel_fat_plist_arm64 rel_fat_ident_x86; do
  p=$(fx_build "$n"); rec="$FX/rec.d"; : > "$rec"
  env DEVELOPER_DIR="$FX/fakedev" SD_EXEC_RECORD="$rec" "$DOOR" exec "$p" >/dev/null 2>&1
  [ "$(count "$rec")" = 0 ] && pass "$n with a lying DEVELOPER_DIR refused" || fail "$n with a lying DEVELOPER_DIR let through"
done
# a relative path, typed from inside the bundle (the MacOS directory is a symlink out of it)
p=$(fx_build rel_dir_symlink); rec="$FX/rec.r"; : > "$rec"
(cd "$(dirname "$(dirname "$p")")" && SD_EXEC_RECORD="$rec" "$DOOR" exec ./MacOS/sheepdog) >/dev/null 2>&1
[ "$(count "$rec")" = 0 ] && pass "a relative path into a release-ID bundle refused" || fail "a relative path let through"
# when lipo cannot run (an unaccepted Xcode licence, a missing developer dir), every Mach-O file
# counts as the release ID: a copy of the door whose lipo is a stub that fails
mkdir -p "$FX/door/lib" "$FX/stub"
cp "$SD_ROOT/scripts/release.conf" "$FX/door/"
printf '#!/bin/sh\necho "Agreeing to the Xcode/iOS license requires admin privileges" >&2\nexit 69\n' > "$FX/stub/lipo"; chmod +x "$FX/stub/lipo"
sed "s#^  LIPO=/usr/bin/lipo\$#  LIPO=$FX/stub/lipo#" "$SD_ROOT/scripts/lib/exec-guard.sh" > "$FX/door/lib/exec-guard.sh"; chmod +x "$FX/door/lib/exec-guard.sh"
grep -q "LIPO=$FX/stub/lipo" "$FX/door/lib/exec-guard.sh" || fail "the lipo stub was not applied to the door copy"
for n in rel_embedded_plist rel_upper_embedded rel_fat_plist_arm64 rel_fat_ident_x86 dev_bare; do
  p=$(fx_build "$n"); rec="$FX/rec.x"; : > "$rec"
  SD_EXEC_RECORD="$rec" "$FX/door/lib/exec-guard.sh" exec "$p" >/dev/null 2>&1
  [ "$(count "$rec")" = 0 ] && pass "$n refused when lipo fails" || fail "$n let through when lipo fails"
done
# control: the same copy with a stub that passes through to the real lipo allows the dev files, so
# the refusals above come from the failing lipo, not from copying the door
printf '#!/bin/sh\nexec /usr/bin/lipo "$@"\n' > "$FX/stub/lipo"
for n in dev_bare dev_bundle dev_script; do
  p=$(fx_build "$n"); rec="$FX/rec.x"; : > "$rec"
  SD_EXEC_RECORD="$rec" "$FX/door/lib/exec-guard.sh" exec "$p" >/dev/null 2>&1
  [ "$(count "$rec")" = 1 ] && pass "control: $n runs through the copy with a working lipo" || fail "control: $n refused by the copy with a working lipo"
done

# `exec` runs the path the caller typed (a symlink stays a symlink), judged in both forms
p=$(fx_build dev_symlink); rec="$FX/rec.t"; : > "$rec"
SD_EXEC_RECORD="$rec" "$DOOR" exec "$p" >/dev/null 2>&1
[ "$(cat "$rec")" = "would exec $p" ] && pass "exec keeps the typed path" || fail "recorded: $(cat "$rec")"

# nothing was ever run
[ -e "$FX/ran" ] && fail "a fixture ran: $(cat "$FX/ran")" || pass "no fixture ran"
finish
