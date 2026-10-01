#!/bin/sh
# tests/lib/tree-same.py, the bundle comparison of the rc and npm cells: two copies of a tree are
# the same (control), and each of these makes them differ: the root's own mode, a child's mode, a
# file's content, a link's target, a link where a file was, an extra empty directory, a missing
# file; a root that is a link or does not exist is refused. Nothing is executed.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
T="$SD_ROOT/tests/lib/tree-same.py"
mk() { # dir: a small tree with a file, an executable, a link, an empty dir
  mkdir -p "$1/Contents/MacOS" "$1/Contents/empty" && printf 'a\n' > "$1/Contents/f" && printf 'x\n' > "$1/Contents/MacOS/x" \
    && chmod 755 "$1/Contents/MacOS/x" && ln -s MacOS/x "$1/Contents/l" && chmod 755 "$1"
}
row() { # what expect(same|differs) a b
  python3 "$T" "$3" "$4" > "$FX/o" 2>&1; r=$?
  case $2 in
    same) [ $r = 0 ] && pass "$1: the same" || fail "$1: rc=$r $(tr '\n' ' ' < "$FX/o")" ;;
    differs) [ $r = 1 ] && pass "$1: differs ($(tr '\n' ' ' < "$FX/o"))" || fail "$1: rc=$r, not a difference" ;;
  esac
}
n=0
pair() { n=$((n + 1)); A=$FX/$n/a B=$FX/$n/b; mk "$A"; mk "$B"; }
pair; row "control: two copies" same "$A" "$B"
pair; chmod 777 "$B"; row "the root's own mode" differs "$A" "$B"
pair; chmod 775 "$B/Contents/MacOS/x"; row "a child's mode" differs "$A" "$B"
pair; printf 'b\n' > "$B/Contents/f"; row "a file's content" differs "$A" "$B"
pair; rm "$B/Contents/l" && ln -s MacOS/../MacOS/x "$B/Contents/l"; row "a link's target" differs "$A" "$B"
pair; rm "$B/Contents/f" && ln -s MacOS/x "$B/Contents/f"; row "a link where a file was" differs "$A" "$B"
pair; mkdir "$B/Contents/extra"; row "an extra empty directory" differs "$A" "$B"
pair; rm "$B/Contents/f"; row "a missing file" differs "$A" "$B"
pair; ln -s "$B" "$FX/$n/blink"; row "a root that is a link" differs "$A" "$FX/$n/blink"
pair; row "a root that does not exist" differs "$A" "$FX/$n/none"
finish
