#!/bin/sh
# Issue #20 (review round 3): a release `sheepr run` or `sweep` removes the registration folders of
# sheeprs that are gone where its own listener would put one: TMPDIR, or /tmp when TMPDIR is unset
# or too long. A test leg that hand-runs a release build must not do that in the operator's /tmp
# with the code under test. So every hand-run of a sheepr binary in a test leg (`env -i` with
# SHEEPR_STATE=, the convention for one) names its own TMPDIR or passes --no-sweep, on the same
# logical line (backslash continuations joined; comment lines skipped). The control: a line with
# neither is caught, the two forms that pass pass. Bounds: a hand-run spelled without `env -i` and
# SHEEPR_STATE= is not seen; a TMPDIR too long for a socket path would still mean /tmp.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
scan() { # file... -> each logical line that breaks the rule, as file:line
  awk 'FNR == 1 { buf = "" }
    { if (buf == "") start = FNR
      if ($0 ~ /\\$/) { buf = buf substr($0, 1, length($0) - 1) " "; next }
      buf = buf $0
      if (buf !~ /^[ \t]*#/ && buf ~ /env -i/ && buf ~ /SHEEPR_STATE=/ && buf !~ /TMPDIR=/ && buf !~ /--no-sweep/) print FILENAME ":" start
      buf = "" }' "$@"
}
# the control (built from pieces, so this file holds no line of the shape it looks for)
ei="env -i" st="SHEEPR_STATE"
{ printf '%s\n' "# $ei PATH=/x $st=/s \"\$b\" run -- true"
  printf '%s\n' "$ei PATH=/usr/bin:/bin HOME=\"\$h\" $st=\"\$h/s\" \"\$b\" run -- true"
  printf '%s\n' "$ei PATH=/usr/bin:/bin HOME=\"\$h\" $st=\"\$h/s\" TMPDIR=\"\$h/t\" \"\$b\" run -- true"
  printf '%s\n' "$ei PATH=/usr/bin:/bin HOME=\"\$h\" $st=\"\$h/s\" \\" "  \"\$b\" run --no-sweep -- true"; } > "$FX/control.sh"
got=$(scan "$FX/control.sh")
[ "$got" = "$FX/control.sh:2" ] && pass "control: a hand-run with neither TMPDIR nor --no-sweep is caught, the others pass" || fail "control: '$got'"
found=$(cd "$SR_ROOT" && scan tests/dist/*.sh tests/rc/*.sh scripts/*.sh scripts/lib/*.sh test-all)
[ -z "$found" ] && pass "every hand-run of a sheepr binary in a test leg names its own TMPDIR or passes --no-sweep" || fail "a hand-run that may sweep the operator's /tmp: $(echo $found)"
finish
