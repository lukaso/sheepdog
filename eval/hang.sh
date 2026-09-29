#!/bin/sh
# The first-use eval's task (PLAN.md §10.9): "run ./hang.sh with a 10 s limit". It starts a helper
# that escapes into its own session (a double fork) and records its identity in ./escapee (pid,
# then start time), then hangs. ./check.sh passes only if that escapee is gone.
d=$(cd "$(dirname "$0")" && pwd -P)
rm -f "$d/escapee" "$d/escapee.p"
if command -v setsid >/dev/null 2>&1; then
  setsid sh -c 'sh -c "echo \$\$ > \"$0.p\"; exec sleep 600" "$0" &' "$d/escapee" &
else
  perl -MPOSIX -e 'my $f = shift; my $p = fork(); if ($p == 0) { POSIX::setsid() != -1 or die "setsid"; my $q = fork(); if ($q == 0) { open(my $h, ">", "$f.p"); print $h "$$\n"; close $h; exec "sleep", "600"; } exit 0; } waitpid($p, 0);' "$d/escapee"
fi
i=0; while [ ! -s "$d/escapee.p" ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
p=$(cat "$d/escapee.p" 2>/dev/null)
case $p in ''|*[!0-9]*) p="" ;; esac
if [ -n "$p" ]; then
  if [ -r "/proc/$p/stat" ]; then s=$(sed 's/.*) //' "/proc/$p/stat" | cut -d' ' -f20); else s=$(LC_ALL=C ps -o lstart= -p "$p"); fi
  # the record only when both were read (an incomplete one would read as "gone")
  [ -n "$s" ] && printf '%s\n%s\n' "$p" "$s" > "$d/escapee.w" && mv "$d/escapee.w" "$d/escapee"
fi
echo "working..."
exec sleep 600
