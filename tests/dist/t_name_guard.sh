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
row "the old tarball prefix is a hit" 1 "a.txt:1: ${tp%sheep}" "$(repo a.txt "${tp}r-0.1.0.tgz")"
u=lukas; u="${u}o"
row "the old owner name before - (a tarball prefix) is a hit" 1 "a.py:1: $u-" "$(repo a.py "fn = \"$u-%s-%s.tgz\"")"
row "the old owner name before [ (a regex) is a hit" 1 "a.sh:1: $u[" "$(repo a.sh "sed -En 's/.*$u[-/](x)/'")"
row "the old owner name before + (a pnpm store name) is a hit" 1 "a.sh:1: $u+" "$(repo a.sh "x/.pnpm/$u+x@1")"
row "the old owner name before % (an encoded scope) is a hit" 1 "a.sh:1: $u%" "$(repo a.sh "curl /$u%2fx")"
row "control: the owner's URLs, bundle ID, email and home pass" 0 ", 0 hits" "$(repo a.md "https://github.com/$u/x git@github.com:$u/x repos\\/$u\\/x com.$u.x $u@gmail.com /Users/${u}berhuber $u/tap")"
row "the prefix in a path name is a hit" 1 "(path) a/${p2}x.rs:1:" "$(repo "a/${p2}x.rs" "fn main() {}")"
row "the old name in a path name is a hit" 1 "(path) a/$O.md:1:" "$(repo "a/$O.md" "text")"
row "a word on the allow-list passes" 0 ", 0 hits" "$(repo a.c "$b=%d" scripts/lib/name-guard.allow "$b")"
row "the same word without the allow-list is a hit" 1 "a.c:1: $b" "$(repo a.c "$b=%d")"
row "a listed word that occurs nowhere is a hit" 1 "$b is listed but occurs nowhere" "$(repo a.c "clean" scripts/lib/name-guard.allow "$b")"
row "a word that only starts with the letters passes" 0 ", 0 hits" "$(repo a.sh "SDKROOT=/x")"
row "a clean repo passes" 0 "scanned 1 tracked paths, 0 hits" "$(repo a.txt "clean")"
# properties each row below pins (each was a surviving mutant in review)
row "the owner name before _ is a hit" 1 "a.sh:1: ${u}_" "$(repo a.sh "x ${u}_y")"
row "the owner name before * is a hit" 1 "a.sh:1: $u*" "$(repo a.sh "find -path '*$u*'")"
row "a second prefix word on a line after an allowed one is a hit" 1 "a.c:1: ${p3}x" "$(repo a.c "$b=%d ${p3}x" scripts/lib/name-guard.allow "$b")"
row "an allowed word inside a longer word is a hit (exact words only)" 1 "a.c:1: x$b" "$(repo a.c "x$b $b" scripts/lib/name-guard.allow "$b")"
row "an allowed word with a dot in it passes (a dot is part of a word)" 0 ", 0 hits" "$(repo a.c "v1.${p3}ok" scripts/lib/name-guard.allow "v1.${p3}ok")"
r=$(repo a.txt clean); ln -s "/x/$o/y" "$r/lnk" && git -C "$r" add lnk
row "a tracked symlink whose target holds the old name is a hit" 1 "lnk:1: $o" "$r"
row "a word in the allow-list's own comment lines is a hit" 1 "scripts/lib/name-guard.allow:1: ${p1}X" "$(repo a.c "$b" scripts/lib/name-guard.allow "# the old ${p1}X
$b")"
# what the guard also covers (red before it did)
row "the prefix in another case (S D -) is a hit" 1 "a.sh:1: $(printf %s "$p2" | tr a-z A-Z)x" "$(repo a.sh "$(printf %s "$p2" | tr a-z A-Z)x")"
row "the prefix in another case (S d _) is a hit" 1 "a.sh:1: S${p3#s}x" "$(repo a.sh "S${p3#s}x")"
row "the old scope URL-encoded is a hit" 1 "a.sh:1:" "$(repo a.sh "curl https://registry.npmjs.org/%40$u%2fx")"
row "the old scope as an HTML entity is a hit" 1 "a.html:1:" "$(repo a.html "<p>&#64;$u/x</p>")"
r=$(repo a.txt "the $o app"); printf 'clean\n' > "$r/a.txt"
row "an old name staged while the file on disk is clean is a hit" 1 "a.txt:1: $o" "$r"
r=$(repo a.c "$b=%d"); mkdir -p "$r/scripts/lib"; printf '%s\n' "$b" > "$r/scripts/lib/name-guard.allow"
row "an allow-list that is not tracked is exit 2" 2 "is not tracked" "$r"
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
