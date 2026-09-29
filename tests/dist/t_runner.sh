#!/bin/sh
# tests/dist/run.sh as a subject (PHASE3.md §3): green with passing cells; red with a failing
# cell, a cell that skipped on macOS, or no cells; on TERM it prints the running cell's output and
# no process of that cell is left, even one that ignores TERM (the whole group is killed).
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
R="$SD_ROOT/tests/dist/run.sh"
mk() { rm -rf "$FX/d"; mkdir -p "$FX/d"; cp "$R" "$FX/d/run.sh"; }
mk; printf '#!/bin/sh\necho a\n' > "$FX/d/t_a.sh"
sh "$FX/d/run.sh" > "$FX/o" 2>&1 && grep -q '^a$' "$FX/o" && pass "passing cells: green, output printed" || fail "passing cells"
printf '#!/bin/sh\nexit 1\n' > "$FX/d/t_b.sh"
sh "$FX/d/run.sh" >/dev/null 2>&1 && fail "a failing cell was green" || pass "a failing cell: red"
if [ "$(uname -s)" = Darwin ]; then
  mk; printf '#!/bin/sh\necho SKIP here\n' > "$FX/d/t_c.sh"
  sh "$FX/d/run.sh" >/dev/null 2>&1 && fail "a SKIP on macOS was green" || pass "a SKIP on macOS: red"
fi
mk; sh "$FX/d/run.sh" >/dev/null 2>&1 && fail "no cells was green" || pass "no cells: red"
# TERM with a cell whose child ignores TERM: both record their pids. Under sh, and under dash when
# it is there (dash's kill builtin rejects `--`, so the kill form must work in both).
shells=sh; command -v dash >/dev/null 2>&1 && shells="sh dash"
for shell in $shells; do
  mk; rm -f "$FX/cellpid" "$FX/childpid"
  cat > "$FX/d/t_hang.sh" <<H
#!/bin/sh
trap '' TERM
echo started-cell-output
echo \$\$ > "$FX/cellpid"
sh -c 'trap "" TERM; echo \$\$ > "$FX/childpid"; exec sleep 60' &
wait
H
  $shell "$FX/d/run.sh" > "$FX/o" 2>&1 & rp=$!
  i=0; while [ ! -s "$FX/childpid" ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
  kill -TERM $rp; wait $rp
  grep -q started-cell-output "$FX/o" && pass "$shell, TERM: the running cell's output printed" || fail "$shell, TERM: output lost"
  sleep 1
  left=""
  for f in cellpid childpid; do p=$(cat "$FX/$f" 2>/dev/null); [ -n "$p" ] && kill -0 "$p" 2>/dev/null && left="$left $p"; done
  if [ -z "$left" ]; then pass "$shell, TERM: no process of the cell is left"
  else fail "$shell, TERM: left alive:$left"; for p in $left; do kill -KILL "$p" 2>/dev/null; done; fi
done
finish
