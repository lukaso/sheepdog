#!/bin/sh
# The tool's old names stay out of the tracked tree (scripts/lib/name-guard.py). The real tree
# must be clean, and the guard must have scanned every tracked path, its own file included.
# Controls, each in a fixture repo of its own (the fixture words are built from pieces, so this
# file stays clean and a rename pass cannot rewrite them): a hit in a file's content or in a path
# name is exit 1 and is named in the output; the allow-list makes its exact word pass (and the
# same word without it is a hit); a listed word that occurs nowhere is exit 1; a word that only
# starts with the letters (SDKROOT) passes; outside a repo, and a tracked file that cannot be
# read, are exit 2.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE
GIT_CEILING_DIRECTORIES=$FX; export GIT_CEILING_DIRECTORIES
G="$SR_ROOT/scripts/lib/name-guard.py"
o=sheep; o="${o}dog"; O=$(printf %s "$o" | tr a-z A-Z); Oc="S${o#s}"
p1=S; p1="${p1}D_"; p2=s; p2="${p2}d-"; p3=s; p3="${p3}d_"
b=bs; b="${b}d_r"; sc=@; sc="${sc}lukaso"; tp=lukaso; tp="${tp}-sheep"
# repo FILE CONTENT [FILE CONTENT ...]: a fresh repo with those files in its index; prints its path
# (a directory of its own each time: a counter would not survive the $( ) subshell)
repo() {
  r=$(mktemp -d "$FX/r.XXXXXX") && git -C "$r" init -q || { echo "cannot make a fixture repo" >&2; exit 3; }
  while [ $# -ge 2 ]; do mkdir -p "$r/$(dirname "$1")" && printf '%s\n' "$2" > "$r/$1"; shift 2; done
  git -C "$r" add -A && echo "$r"
}
# row NAME WANT-EXIT WANT-TEXT REPO: the guard's exit is WANT-EXIT and its output holds WANT-TEXT
row() {
  out=$(python3 "$G" "$4" 2>&1); rc=$?
  if [ "$rc" = "$2" ] && printf '%s\n' "$out" | grep -F -- "$3" >/dev/null; then pass "$1"
  else fail "$1: exit $rc (want $2), want '$3', got: $(printf '%s' "$out" | tail -3 | tr '\n' '|')"; fi
}

row "the prefix with _ in a file is a hit" 1 "a.sh:1: ${p1}FOO" "$(repo a.sh "${p1}FOO=1")"
row "the prefix with - after a space is a hit" 1 "a.sh:1: ${p2}foo" "$(repo a.sh "x ${p2}foo")"
row "the prefix with _ after a \$ is a hit" 1 "a.sh:1: ${p3}foo" "$(repo a.sh "\$${p3}foo")"
row "the prefix after a backslash-n is a hit" 1 "a.sh:1: n${p1}FOO" "$(repo a.sh "'\\n${p1}FOO'")"
row "the prefix inside a longer word is a hit" 1 "a.rs:1: CARGO_BIN_EXE_${p2}foo" "$(repo a.rs "CARGO_BIN_EXE_${p2}foo")"
row "the old name, capitalised, is a hit" 1 "a.md:1: $o" "$(repo a.md "the $Oc app")"
row "the old name, upper case, is a hit" 1 "a.sh:1: $o" "$(repo a.sh "${O}_STATE=1")"
row "the old scope is a hit" 1 "a.json:1: $sc" "$(repo a.json "\"${sc}/x\": \"1\"")"
row "the old tarball prefix is a hit" 1 "a.txt:1: $tp" "$(repo a.txt "${tp}r-0.1.0.tgz")"
row "the prefix in a path name is a hit" 1 "(path) a/${p2}x.rs:1:" "$(repo "a/${p2}x.rs" "fn main() {}")"
row "the old name in a path name is a hit" 1 "(path) a/$O.md:1:" "$(repo "a/$O.md" "text")"
row "a word on the allow-list passes" 0 ", 0 hits" "$(repo a.c "$b=%d" scripts/lib/name-guard.allow "$b")"
row "the same word without the allow-list is a hit" 1 "a.c:1: $b" "$(repo a.c "$b=%d")"
row "a listed word that occurs nowhere is a hit" 1 "$b is listed but occurs nowhere" "$(repo a.c "clean" scripts/lib/name-guard.allow "$b")"
row "a word that only starts with the letters passes" 0 ", 0 hits" "$(repo a.sh "SDKROOT=/x")"
row "a clean repo passes" 0 "scanned 1 tracked paths, 0 hits" "$(repo a.txt "clean")"
mkdir -p "$FX/plain" && echo x > "$FX/plain/a"
row "outside a repo is exit 2" 2 "git ls-files failed" "$FX/plain"
if [ "$(id -u)" != 0 ]; then
  r=$(repo a.txt clean b.txt clean); chmod 000 "$r/b.txt"
  row "a tracked file that cannot be read is exit 2" 2 "cannot read the tracked file b.txt" "$r"
  chmod 600 "$r/b.txt"
fi

# the real tree
out=$(python3 "$G" "$SR_ROOT" 2>&1); rc=$?
k=$(git -C "$SR_ROOT" ls-files | wc -l | tr -d ' ')
if git -C "$SR_ROOT" ls-files --error-unmatch scripts/lib/name-guard.py >/dev/null 2>&1 \
   && printf '%s\n' "$out" | tail -1 | grep -F "scanned $k tracked paths" >/dev/null; then
  pass "the guard scanned every tracked path ($k), its own file included"
else fail "the guard did not scan every tracked path (want $k, its own file tracked): $(printf '%s' "$out" | tail -1)"; fi
[ "$rc" = 0 ] && pass "the tracked tree holds none of the old names" \
  || fail "the tracked tree holds old names: $(printf '%s' "$out" | tail -1); first: $(printf '%s\n' "$out" | sed -n '1,3p' | tr '\n' '|')"
finish
