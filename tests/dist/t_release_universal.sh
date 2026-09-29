#!/bin/sh
# PHASE3.md S1: the universal release build under the hardened runtime. Built with the pinned
# toolchain, --release --locked, for aarch64 and x86_64 with MACOSX_DEPLOYMENT_TARGET=12.0, joined
# by lipo; both slices report minos 12.0 (D8). Bundled with the .dev ID and signed ad hoc with the
# hardened runtime, it runs through scripts/lib/release-run.sh by a PATH-style symlink: --version,
# cell 1 (the exit code passes), the tracking on --status-fd is "responsibility" with no degraded
# mechanism, and cell 3 (a setsid escapee, alive before the end by its recorded identity, is gone
# after). Control: the same escapee without sheepdog survives (and is killed here by identity).
# The x86_64 slice runs only where an x86_64 binary can run (Rosetta); otherwise it is checked
# statically, and the cell says so.
set -u
. "$(dirname "$0")/lib.sh"
[ "$(uname -s)" = Darwin ] || { echo "SKIP (macOS only)"; exit 0; }
fx_dir
T=$FX/target
for t in aarch64-apple-darwin x86_64-apple-darwin; do
  (cd "$SD_ROOT" && env CARGO_TARGET_DIR="$T" MACOSX_DEPLOYMENT_TARGET=12.0 timeout 1200 cargo build -q --release --locked --bin sheepdog --target $t) \
    || { fail "release build for $t"; finish; }
done
lipo -create -output "$FX/sheepdog" "$T/aarch64-apple-darwin/release/sheepdog" "$T/x86_64-apple-darwin/release/sheepdog" || { fail lipo; finish; }
for a in arm64 x86_64; do
  m=$(vtool -arch $a -show-build "$FX/sheepdog" 2>/dev/null | awk '$1=="minos"{print $2; exit}')
  [ "$m" = 12.0 ] && pass "$a minos 12.0" || fail "$a minos '$m'"
done
app=$("$SD_ROOT/scripts/bundle.sh" "$FX/sheepdog" "$FX/b" 0.1.0 1) || { fail bundle; finish; }
/usr/bin/codesign -s - -f -o runtime "$app" 2>/dev/null
case $(/usr/bin/codesign -d -v "$app" 2>&1) in *'(adhoc,runtime)'*) pass "signed ad hoc with the hardened runtime" ;; *) fail "not hardened" ;; esac
mkdir -p "$FX/bin" "$FX/home"; ln -s "$app/Contents/MacOS/sheepdog" "$FX/bin/sheepdog"
R() { "$SD_ROOT/scripts/lib/release-run.sh" "$FX/home" "$FX/bin/sheepdog" "$@"; }

R --version >/dev/null 2>&1 && pass "--version" || fail "--version"
R run -- sh -c 'exit 7' 2>/dev/null; rc=$?
[ $rc = 7 ] && pass "cell 1: the exit code passes" || fail "cell 1: rc=$rc"
R run --status-fd 3 -- sh -c 'exit 0' 3>"$FX/status" 2>/dev/null
grep -q '"tracking":"responsibility"' "$FX/status" && grep -q '"degraded":null' "$FX/status" \
  && pass "tracking by responsibility, nothing degraded" || fail "status: $(tail -1 "$FX/status")"

# the escapee: fork, setsid (checked), write pid and start time, sleep. The root waits for the
# record, checks it is alive, then exits; sheepdog's end must kill it.
cat > "$FX/escape.pl" <<'P'
use POSIX;
my $f = shift;
my $p = fork(); die "fork" unless defined $p;
if ($p == 0) {
  POSIX::setsid() != -1 or die "setsid";
  my $q = fork(); die "fork2" unless defined $q;
  if ($q == 0) { my $s = `LC_ALL=C ps -o lstart= -p $$`; chomp $s; open(my $h, '>', "$f.tmp") or die; print $h "$$ $s\n"; close $h; rename "$f.tmp", $f; sleep 300; exit 0; }
  exit 0;
}
waitpid($p, 0);
for (1..100) { last if -e $f; select(undef, undef, undef, 0.05); }
exit(-e $f ? 0 : 3);
P
alive() { # recordfile -> 0 if the recorded process is alive with the same start time
  p=$(cut -d' ' -f1 "$1"); s=$(cut -d' ' -f2- "$1")
  [ "$(LC_ALL=C ps -o lstart= -p "$p" 2>/dev/null)" = "$s" ]
}
R run -- perl "$FX/escape.pl" "$FX/esc1" 2>/dev/null; rc=$?
if [ $rc = 0 ] && [ -s "$FX/esc1" ]; then
  if alive "$FX/esc1"; then fail "cell 3: the escapee survived"; kill -9 "$(cut -d' ' -f1 "$FX/esc1")"; else pass "cell 3: the escapee is gone"; fi
else fail "cell 3: the escapee did not start (rc=$rc)"; fi
# control: without sheepdog it survives
perl "$FX/escape.pl" "$FX/esc2"
if [ -s "$FX/esc2" ] && alive "$FX/esc2"; then pass "control: without sheepdog the escapee survives"; kill -9 "$(cut -d' ' -f1 "$FX/esc2")"
else fail "control: the escapee did not survive without sheepdog"; fi

# the x86_64 slice: the door judges the file first (a refusal fails the cell); it runs only where
# an x86_64 binary can (Rosetta), else the cell notes that it was checked statically
if ! "$SD_ROOT/scripts/lib/exec-guard.sh" check "$FX/sheepdog"; then fail "the door refused the universal binary"
elif printf 'int main(){return 0;}\n' > "$FX/x.c" && cc -arch x86_64 -o "$FX/x" "$FX/x.c" \
    && "$SD_ROOT/scripts/lib/exec-guard.sh" check "$FX/x" && arch -x86_64 "$FX/x" 2>/dev/null; then
  arch -x86_64 "$FX/sheepdog" --version >/dev/null 2>&1 && pass "x86_64 slice runs (Rosetta)" || fail "x86_64 slice does not run under Rosetta"
else
  echo "note: no x86_64 runtime here (Rosetta absent); the x86_64 slice is checked statically only"
fi
finish
