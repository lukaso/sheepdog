#!/bin/sh
# Issue #20 (review round 3): a release `sheepr run` or `sweep` removes the registration folders of
# sheeprs that are gone where its own listener would put one: TMPDIR, or /tmp when TMPDIR is unset
# or too long. A test leg that hand-runs a release build must not do that in the operator's /tmp
# with the code under test. So every hand-run of a sheepr binary in a test leg (`env -i` with
# SHEEPR_STATE=, the convention for one) names its own TMPDIR or passes --no-sweep, on the same
# logical line (backslash continuations joined; a line that starts with a comment is skipped and
# never continued, as sh never continues one). The control: a line with neither is caught (also
# after a comment ending in a backslash), the two forms that pass pass, and a path the scan cannot
# read fails it. The real scan must read every path it names (each pattern matches a file, awk
# exits 0) and see the two hand-runs it caught before their fix (release-refusal.sh,
# t_npm_install.sh's `job`), so a scan that read nothing never passes. Bounds: a hand-run spelled
# without `env -i` and SHEEPR_STATE= is not seen; a TMPDIR too long for a socket path would still
# mean /tmp; the scan is not a shell parser: a comment INSIDE a continued command, or after it on
# its line, is read as part of it, so a `TMPDIR=` or `--no-sweep` written only in such a comment
# passes (and its `\` is taken as a continuation, where sh ends the command at the comment).
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
scan() { # file... -> "SEEN file:line" for each hand-run, "BAD file:line" for each that breaks the
  # rule; awk's exit status (not 0 when a file cannot be read)
  awk 'FNR == 1 { buf = "" }
    { if (buf == "" && $0 ~ /^[ \t]*#/) next   # a comment line: never continued, even ending in a backslash
      if (buf == "") start = FNR
      if ($0 ~ /\\$/) { buf = buf substr($0, 1, length($0) - 1) " "; next }
      buf = buf $0
      if (buf ~ /env [-]i/ && buf ~ /SHEEPR_STATE[=]/) {
        print "SEEN " FILENAME ":" start
        if (buf !~ /TMPDIR[=]/ && buf !~ /--no-sweep/) print "BAD " FILENAME ":" start }
      buf = "" }' "$@"
}
# the control (built from pieces, so no line of this file breaks the rule)
ei="env -i" st="SHEEPR_STATE"
{ printf '%s\n' "# $ei PATH=/x $st=/s \"\$b\" run -- true"
  printf '%s\n' "$ei PATH=/usr/bin:/bin HOME=\"\$h\" $st=\"\$h/s\" \"\$b\" run -- true"
  printf '%s\n' "$ei PATH=/usr/bin:/bin HOME=\"\$h\" $st=\"\$h/s\" TMPDIR=\"\$h/t\" \"\$b\" run -- true"
  printf '%s\n' "$ei PATH=/usr/bin:/bin HOME=\"\$h\" $st=\"\$h/s\" \\" "  \"\$b\" run --no-sweep -- true"
  # a comment ending in a backslash does not continue (sh runs the next line): line 7 is a hand-run
  printf '%s\n' "# a comment that ends in a backslash \\" "$ei PATH=/usr/bin:/bin $st=\"\$h/s\" \"\$b\" run -- true"; } > "$FX/control.sh"
got=$(scan "$FX/control.sh" | sed -n 's/^BAD //p' | tr '\n' ' ')
[ "$got" = "$FX/control.sh:2 $FX/control.sh:7 " ] && pass "control: a hand-run with neither TMPDIR nor --no-sweep is caught (also after a comment ending in a backslash), the others pass" || fail "control: '$got'"
scan "$FX/missing.sh" "$FX/control.sh" > /dev/null 2>&1 && fail "control: a path the scan cannot read passed" || pass "control: a path the scan cannot read fails the scan"
cd "$SR_ROOT" || { fail "cd $SR_ROOT"; finish; }
for g in 'tests/dist/*.sh' 'tests/rc/*.sh' 'scripts/*.sh' 'scripts/lib/*.sh' 'test-all'; do
  set -- $g; [ -e "$1" ] || fail "the scan's path $g matches nothing"
done
out=$(scan tests/dist/*.sh tests/rc/*.sh scripts/*.sh scripts/lib/*.sh test-all 2>&1); rc=$?
seen=$(printf '%s\n' "$out" | grep -c '^SEEN ') bad=$(printf '%s\n' "$out" | sed -n 's/^BAD //p')
[ $rc = 0 ] && printf '%s\n' "$out" | grep -q '^SEEN scripts/release-refusal\.sh:' && printf '%s\n' "$out" | grep -q '^SEEN tests/dist/t_npm_install\.sh:' \
  && pass "the scan read the test legs ($seen hand-runs, the two caught before among them)" || fail "the scan did not read the test legs: rc=$rc, $seen seen: $(printf '%s\n' "$out" | grep -v '^SEEN ' | head -3 | tr '\n' ' ')"
[ -z "$bad" ] && pass "every hand-run of a sheepr binary in a test leg names its own TMPDIR or passes --no-sweep" || fail "a hand-run that may sweep the operator's /tmp: $(echo $bad)"
finish
