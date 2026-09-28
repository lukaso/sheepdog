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
[ "$rows" -ge 14 ] || fail "only $rows fixtures ran"

# a lying codesign first on PATH, and a test variable set: the door still refuses
mkdir -p "$FX/liar"
printf '#!/bin/sh\necho "Identifier=com.example.harmless" >&2\nexit 0\n' > "$FX/liar/codesign"
chmod +x "$FX/liar/codesign"
p=$(fx_build rel_adhoc_bundle)
PATH="$FX/liar:$PATH" SHEEPDOG_TEST_TAG=0123456789abcdef door refuse "$p" "lying codesign on PATH"

# nothing was ever run
[ -e "$FX/ran" ] && fail "a fixture ran: $(cat "$FX/ran")" || pass "no fixture ran"
finish
