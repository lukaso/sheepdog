#!/bin/sh
# Compare a release's version with the tap's cask (release.sh publish, so the cask never moves
# back): cask-version.sh OLD NEW prints newer, older or same (NEW against OLD). The grammar is
# render-cask.sh's, X.Y.Z or X.Y.Z-rc.N; each field compares as a number (0.10.0 is after 0.9.0),
# and an rc comes before its final. Exit 2: usage, or a version off the grammar.
set -u
[ $# -eq 2 ] || { echo "usage: cask-version.sh OLD NEW" >&2; exit 2; }
# one line only: grep matches line by line, so a second line would pass unchecked
one() { printf '%s\n' "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-rc\.[1-9][0-9]*)?$' && [ "$(printf '%s' "$1" | wc -l | tr -d ' ')" = 0 ]; }
for v in "$1" "$2"; do one "$v" || { echo "cask-version: bad version '$v'" >&2; exit 2; }; done
# awk compares the fields as decimal numbers (no octal reading of a leading 0); a final is rc
# "infinity", after each of its rcs
printf '%s %s\n' "$1" "$2" | awk '
  function key(v, a, n) {
    n = split(v, a, /[.-]/)
    k[1] = a[1] + 0; k[2] = a[2] + 0; k[3] = a[3] + 0
    k[4] = (n == 5) ? substr(a[5], 1) + 0 : -1   # a[4] is "rc"; -1 marks a final
  }
  {
    key($1); o1 = k[1]; o2 = k[2]; o3 = k[3]; o4 = k[4]
    key($2); n1 = k[1]; n2 = k[2]; n3 = k[3]; n4 = k[4]
    if (o4 == -1) o4 = 1e18; if (n4 == -1) n4 = 1e18
    if (n1 != o1) { print (n1 > o1 ? "newer" : "older"); exit }
    if (n2 != o2) { print (n2 > o2 ? "newer" : "older"); exit }
    if (n3 != o3) { print (n3 > o3 ? "newer" : "older"); exit }
    if (n4 != o4) { print (n4 > o4 ? "newer" : "older"); exit }
    print "same"
  }'
