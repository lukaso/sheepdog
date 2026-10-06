#!/bin/sh
# scripts/lib/warnings.sh, test-all's compiler-warning rule (issue #10): a log with rustc's warning
# fails the leg, with or without colour, and a log the check cannot read fails it too (an
# unread log is not a clean one). The logs are rustc's own output: a one-line library with an
# unused variable and one without, compiled by the pinned toolchain (run from the repo) into the
# fixture directory. No log line is printed as it is: the bundle leg's own log is checked by the
# same rule.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
. "$SR_ROOT/scripts/lib/warnings.sh"
printf 'pub fn f() { let x = 1; }\n' > "$FX/w.rs"
printf 'pub fn f() {}\n' > "$FX/c.rs"
rustc_log() { # name colour source: rustc's output in $FX/name; exit 0 only if rustc compiled it
  (cd "$SR_ROOT" && rustc --color="$2" --crate-type lib --emit=metadata -o "$FX/$1.rmeta" "$FX/$3.rs") > "$FX/$1" 2>&1
}
show() { tr '\033' '~' < "$1" | head -4 | sed 's/^/  | /'; }
rustc_log plain never w || { fail "rustc did not compile the warning source"; show "$FX/plain"; finish; }
rustc_log colour always w || { fail "rustc did not compile the warning source (colour)"; finish; }
rustc_log clean never c || { fail "rustc did not compile the clean source"; finish; }
# the expected count is the plain log's lines that start `warning`, so a colour must not change it
n=$(grep -c '^warning' "$FX/plain")
[ "$n" -ge 1 ] && grep -q 'unused variable' "$FX/plain" || { fail "harness: the plain log has no warning line"; show "$FX/plain"; }
grep -q "$WARN_ESC" "$FX/colour" || fail "harness: rustc --color=always wrote no colour code"

[ -z "$(warn_fail "$FX/clean")" ] && [ ! -s "$FX/clean" ] && pass "control: a clean build's log passes" \
  || fail "a clean build's log fails: $(warn_fail "$FX/clean")"
[ "$(warn_count "$FX/plain")" = "$n" ] && [ -n "$(warn_fail "$FX/plain")" ] && pass "rustc's warning fails the leg ($n line(s))" \
  || fail "rustc's warning: count $(warn_count "$FX/plain") (want $n)"
[ "$(warn_count "$FX/colour")" = "$n" ] && [ -n "$(warn_fail "$FX/colour")" ] && pass "rustc's coloured warning fails the leg ($n line(s))" \
  || { fail "rustc's coloured warning: count $(warn_count "$FX/colour") (want $n)"; show "$FX/colour"; }
printf 'test a_warning_test ... ok\n  warning: indented\nnote: warning: inside a line\n' > "$FX/text"
[ -z "$(warn_fail "$FX/text")" ] && pass "control: 'warning' not at the start of a line passes" \
  || fail "'warning' inside a line fails: $(warn_fail "$FX/text")"
cp "$FX/plain" "$FX/locked" && chmod 000 "$FX/locked"
if [ -r "$FX/locked" ]; then fail "harness: a mode-000 file is still readable (root?)"
else
  why=$(warn_fail "$FX/locked")
  case $why in *"cannot be read"*) pass "a log the check cannot read fails the leg" ;; *) fail "an unreadable log: '$why' (want: cannot be read)" ;; esac
fi
chmod 600 "$FX/locked"
finish
