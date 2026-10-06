#!/bin/sh
# scripts/lib/warnings.sh, test-all's compiler-warning rule (issue #10): a log with rustc's warning
# fails the leg, with or without colour and with or without a code (`warning[E0133]: `), and a
# log the check cannot read fails it too (an unread log is not a clean one). The logs are rustc's
# own output: tiny libraries with an unused variable, with a coded warning, and with none,
# compiled by the pinned toolchain (run from the repo) into the fixture directory. And cargo
# builds whose config file hides a warning (colour and the progress bar, which put it in the
# middle of a line; `build.warnings = "allow"`): under cargo's output lock, which the build gets
# only by sourcing the lib (as test-all and the containers do), the warning fails the leg; without
# the lock the same build hides it (the controls that prove each config file took effect). No
# log line is printed as it is: the bundle leg's own log is checked by the same rule.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir
. "$SR_ROOT/scripts/lib/warnings.sh"
printf 'pub fn f() { let x = 1; }\n' > "$FX/w.rs"
printf 'pub fn f() {}\n' > "$FX/c.rs"
printf '#![warn(unsafe_op_in_unsafe_fn)]\nunsafe fn g() {}\npub unsafe fn f() { g(); }\n' > "$FX/k.rs"
rustc_log() { # name colour source: rustc's output in $FX/name; exit 0 only if rustc compiled it
  (cd "$SR_ROOT" && rustc --color="$2" --crate-type lib --emit=metadata -o "$FX/$1.rmeta" "$FX/$3.rs") > "$FX/$1" 2>&1
}
show() { tr '\033' '~' < "$1" | head -4 | sed 's/^/  | /'; }
rustc_log plain never w || { fail "rustc did not compile the warning source"; show "$FX/plain"; finish; }
rustc_log colour always w || { fail "rustc did not compile the warning source (colour)"; finish; }
rustc_log clean never c || { fail "rustc did not compile the clean source"; finish; }
rustc_log coded never k || { fail "rustc did not compile the coded-warning source"; finish; }
rustc_log coded_c always k || { fail "rustc did not compile the coded-warning source (colour)"; finish; }
# the expected count is the plain log's lines that start `warning`, so a colour must not change it
n=$(grep -c '^warning' "$FX/plain")
[ "$n" -ge 1 ] && grep -q 'unused variable' "$FX/plain" || { fail "harness: the plain log has no warning line"; show "$FX/plain"; }
grep -q "$WARN_ESC" "$FX/colour" || fail "harness: rustc --color=always wrote no colour code"
k=$(grep -c '^warning' "$FX/coded")
grep -q '^warning\[E0133\]: ' "$FX/coded" || { fail "harness: the coded log has no warning[E0133] line"; show "$FX/coded"; }
grep -q "$WARN_ESC" "$FX/coded_c" || fail "harness: the coloured coded log has no colour code"

[ -z "$(warn_fail "$FX/clean")" ] && [ ! -s "$FX/clean" ] && pass "control: a clean build's log passes" \
  || fail "a clean build's log fails: $(warn_fail "$FX/clean")"
[ "$(warn_count "$FX/plain")" = "$n" ] && [ -n "$(warn_fail "$FX/plain")" ] && pass "rustc's warning fails the leg ($n line(s))" \
  || fail "rustc's warning: count $(warn_count "$FX/plain") (want $n)"
[ "$(warn_count "$FX/colour")" = "$n" ] && [ -n "$(warn_fail "$FX/colour")" ] && pass "rustc's coloured warning fails the leg ($n line(s))" \
  || { fail "rustc's coloured warning: count $(warn_count "$FX/colour") (want $n)"; show "$FX/colour"; }
[ "$(warn_count "$FX/coded")" = "$k" ] && [ "$(warn_count "$FX/coded_c")" = "$k" ] && pass "rustc's coded warning fails the leg, plain and coloured ($k line(s))" \
  || { fail "rustc's coded warning: count $(warn_count "$FX/coded") plain, $(warn_count "$FX/coded_c") coloured (want $k)"; show "$FX/coded_c"; }
printf 'test a_warning_test ... ok\n  warning: indented\nnote: warning: inside a line\nwarningless output\nwarning without a colon\nwarnings: 3\nwarning[E0133] no colon\nwarning:no space\n' > "$FX/text"
[ -z "$(warn_fail "$FX/text")" ] && pass "control: lines that are not warning lines pass" \
  || fail "lines that are not warning lines fail: $(warn_fail "$FX/text")"
cp "$FX/plain" "$FX/locked" && chmod 000 "$FX/locked"
if [ -r "$FX/locked" ]; then fail "harness: a mode-000 file is still readable (root?)"
else
  why=$(warn_fail "$FX/locked")
  case $why in *"cannot be read"*) pass "a log the check cannot read fails the leg" ;; *) fail "an unreadable log: '$why' (want: cannot be read)" ;; esac
fi
chmod 600 "$FX/locked"

# cargo, on a tiny crate in the fixture directory: its own CARGO_HOME (no user config is read) and
# target dirs, the pinned toolchain; a build script that sleeps 3 s so that the bar is drawn. Every
# run starts with each variable the lock names removed (a lock in this cell's own environment
# proves nothing); a locked run then sources the lib, an unlocked one sources nothing.
mkdir -p "$FX/crate/src" "$FX/crate/.cargo" "$FX/home"
cp "$SR_ROOT/rust-toolchain.toml" "$FX/crate/"
printf '[package]\nname = "wcheck"\nversion = "0.1.0"\nedition = "2021"\n' > "$FX/crate/Cargo.toml"
printf 'fn main() { std::thread::sleep(std::time::Duration::from_secs(3)); }\n' > "$FX/crate/build.rs"
printf '[term]\ncolor = "always"\nprogress.when = "always"\nprogress.width = 80\n' > "$FX/term.toml"
printf '[build]\nwarnings = "allow"\n' > "$FX/allow.toml"
unlock=""
for kv in $WARN_CARGO_ENV; do unlock="$unlock -u ${kv%%=*}"; done
cargo_log() { # name source config lock(yes|no): cargo build -q's output in $FX/name; exit 0 only if it built
  cp "$FX/$2.rs" "$FX/crate/src/lib.rs" && cp "$FX/$3.toml" "$FX/crate/.cargo/config.toml" || return 1
  src=/dev/null; [ "$4" = yes ] && src="$SR_ROOT/scripts/lib/warnings.sh"
  # shellcheck disable=SC2086 # $unlock is a list of `-u NAME` words
  (cd "$FX/crate" && env $unlock CARGO_HOME="$FX/home" CARGO_TARGET_DIR="$FX/t-$1" \
    sh -c '. "$1" && exec cargo build -q' sh "$src") > "$FX/$1" 2>&1
}
CR=$(printf '\r')
cargo_log bar w term no || { fail "harness: cargo did not build the crate"; show "$FX/bar"; finish; }
grep -q "$CR" "$FX/bar" && grep -q 'unused variable' "$FX/bar" \
  || { fail "harness: the config file drew no progress bar, or the build has no warning"; show "$FX/bar"; }
[ "$(warn_count "$FX/bar")" = 0 ] && pass "control: without the lock, a config file's colour and progress bar hide cargo's warning" \
  || fail "harness: without the lock the match still sees the warning ($(warn_count "$FX/bar")): the lock row proves nothing"
cargo_log barlock w term yes || { fail "cargo did not build the crate under the lock"; show "$FX/barlock"; finish; }
if [ -n "$(warn_fail "$FX/barlock")" ] && ! grep -q "$CR" "$FX/barlock" && ! grep -q "$WARN_ESC" "$FX/barlock"; then
  pass "under the lock, the same build's warning fails the leg (no bar, no colour)"
else fail "under the lock: '$(warn_fail "$FX/barlock")' (want a failure, no bar and no colour)"; show "$FX/barlock"; fi
cargo_log allow w allow no || { fail "harness: cargo did not build the crate (warnings allowed)"; show "$FX/allow"; finish; }
! grep -q 'unused variable' "$FX/allow" && [ "$(warn_count "$FX/allow")" = 0 ] \
  && pass "control: without the lock, build.warnings = \"allow\" hides cargo's warning" \
  || { fail "harness: build.warnings = allow did not hide the warning: the lock row proves nothing"; show "$FX/allow"; }
cargo_log allowlock w allow yes || { fail "cargo did not build the crate under the lock (warnings allowed)"; show "$FX/allowlock"; finish; }
[ -n "$(warn_fail "$FX/allowlock")" ] && pass "under the lock, a warning allowed by the config file fails the leg" \
  || { fail "under the lock, build.warnings = allow still hides the warning"; show "$FX/allowlock"; }
cargo_log lockclean c term yes || { fail "cargo did not build the clean crate under the lock"; show "$FX/lockclean"; finish; }
[ -z "$(warn_fail "$FX/lockclean")" ] && pass "control: a clean build under the lock passes" \
  || { fail "a clean build under the lock fails: $(warn_fail "$FX/lockclean")"; show "$FX/lockclean"; }

# warn_lock, which each container build runs right after sourcing the lib: it reads the lock from
# the environment (what a child inherits), not from the lib's text
# shellcheck disable=SC2086 # $unlock is a list of `-u NAME` words
lk() { env $unlock sh -c "$1"' 2>&1; echo "rc=$?"' sh "$SR_ROOT/scripts/lib/warnings.sh"; }
o=$(lk '. "$1" && warn_lock')
case $o in *"cargo output lock: $WARN_CARGO_ENV"*rc=0) pass "control: after the source, warn_lock reads the whole lock from the environment" ;;
  *) fail "warn_lock after the source: $(printf %s "$o" | tr '\n' ' ')" ;; esac
o=$(lk '. "$1" && unset CARGO_BUILD_WARNINGS && warn_lock')
case $o in *"CARGO_BUILD_WARNINGS unset"*rc=[1-9]*) pass "warn_lock fails when a name is unset" ;;
  *) fail "warn_lock with a name unset: $(printf %s "$o" | tr '\n' ' ')" ;; esac
o=$(lk '. "$1" && unset CARGO_TERM_COLOR && CARGO_TERM_COLOR=never && warn_lock')
case $o in *"CARGO_TERM_COLOR unset"*rc=[1-9]*) pass "warn_lock fails on a name the shell holds but does not export (a child would not see it)" ;;
  *) fail "warn_lock with a name not exported: $(printf %s "$o" | tr '\n' ' ')" ;; esac
o=$(lk '. "$1" && export CARGO_TERM_COLOR=always && warn_lock')
case $o in *"CARGO_TERM_COLOR=always"*rc=[1-9]*) pass "warn_lock fails on a value that is not the lock's" ;;
  *) fail "warn_lock with another value: $(printf %s "$o" | tr '\n' ' ')" ;; esac
finish
