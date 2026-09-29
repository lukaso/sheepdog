#!/bin/sh
# PHASE3.md S2 (and §1.2): scripts/release.sh's checks before any build. `release.sh check vTAG`
# runs only the checks. Refused: a dirty tree, an untracked file, HEAD not on the tag, a missing
# or malformed tag, a tag whose X.Y.Z is not Cargo.toml's version, a build counter not above the
# previous tag's (previous = highest tag below, with -rc sorting before the final): exit 1.
# `build --sign` and `publish` refuse under the test environment (exit 3) and without a terminal
# (exit 4), before anything runs; a usage error is exit 2. The codes tell the reasons apart, so a
# script that refused everything would not pass.
set -u
. "$(dirname "$0")/lib.sh"
fx_dir; fx_repo
RS="$REPO/scripts/release.sh"
chk() { # want label tag
  (cd "$REPO" && env HOME="$FX/ghome" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 sh "$RS" check "$3") >"$FX/o" 2>&1; rc=$?
  case $1 in ok) [ $rc = 0 ] && pass "$2" || fail "$2: rc=$rc $(tail -1 "$FX/o")" ;;
             no) [ $rc = 1 ] && pass "$2: refused" || fail "$2: rc=$rc, want 1 ($(tail -1 "$FX/o"))" ;; esac
}
fx_release 0.1.0 1 v0.1.0-rc.1
chk ok "a clean tree on its tag" v0.1.0-rc.1
echo x >> "$REPO/README.x" ; chk no "an untracked file" v0.1.0-rc.1; rm -f "$REPO/README.x"
echo '# x' >> "$REPO/build.rs"; chk no "a changed tracked file" v0.1.0-rc.1; g checkout -q build.rs
chk no "a missing tag" v0.1.0-rc.9
for t in v0.1 0.1.0 v0.1.0-beta v0.1.0-rc v0.1.0-rc.01 'v0.1.0 ' ; do chk no "a malformed tag '$t'" "$t"; done
g commit -q --allow-empty -m later; chk no "HEAD not on the tag" v0.1.0-rc.1
# (counter 5, so only the version check can refuse it)
fx_release 0.1.0 5 v0.2.0-rc.1; chk no "a tag whose version is not Cargo.toml's" v0.2.0-rc.1
g tag -d v0.2.0-rc.1 >/dev/null
fx_release 0.1.0 1 v0.1.0-rc.2; chk no "a counter not above the previous tag's" v0.1.0-rc.2
fx_release 0.1.0 2 v0.1.0-rc.3; chk ok "a counter above the previous tag's" v0.1.0-rc.3
fx_release 0.1.0 2 v0.1.0; chk no "the final after an rc with the same counter" v0.1.0
g tag -d v0.1.0 >/dev/null; fx_release 0.1.0 3 v0.1.0
chk ok "the final above the last rc's counter" v0.1.0

# the signing and publishing entries refuse before anything runs
fx_shims "$FX/sh" codesign xcrun security gh docker cargo npm ditto lipo spctl
refused() { # want-rc label args...
  w=$1 l=$2; shift 2; rm -f "$FX/sh/calls"
  "$@" >/dev/null 2>&1; rc=$?
  [ $rc = "$w" ] && [ ! -e "$FX/sh/calls" ] && pass "$l: refused ($rc), nothing ran" || fail "$l: rc=$rc (want $w) calls=$(cat "$FX/sh/calls" 2>/dev/null | head -1)"
}
# a wrapper, not a string: PATH may hold spaces
printf '#!/bin/sh\nexec env PATH="%s" HOME="%s" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 "$@"\n' "$FX/sh:$PATH" "$FX/ghome" > "$FX/e"
chmod +x "$FX/e"; E=$FX/e
refused 2 "build --no-notarize without --sign" $E sh "$RS" build --no-notarize v0.1.0
for v in SHEEPDOG_TEST_TAG=0123456789abcdef SHEEPDOG_TEST_STATE=/x; do
  refused 3 "build --sign under $v" $E "$v" sh "$RS" build --sign v0.1.0
  refused 3 "build --sign --no-notarize under $v" $E "$v" sh "$RS" build --sign --no-notarize v0.1.0
  refused 3 "build --no-notarize --sign under $v" $E "$v" sh "$RS" build --no-notarize --sign v0.1.0
  refused 3 "publish under $v" $E "$v" sh "$RS" publish v0.1.0
done
# no terminal: a new session (no controlling tty), the answer piped in
nott() { perl -MPOSIX -e 'my $p = fork(); die unless defined $p; if ($p == 0) { POSIX::setsid() != -1 or die "setsid"; exec @ARGV or die; } waitpid($p, 0); exit($? >> 8)' "$@"; }
refused 4 "build --sign with no terminal" sh -c "echo v0.1.0 | $E sh '$RS' build --sign v0.1.0"
refused 4 "build --sign with no terminal (new session)" nott sh -c "echo v0.1.0 | $E sh '$RS' build --sign v0.1.0"
refused 4 "publish with no terminal (new session)" nott sh -c "echo v0.1.0 | $E sh '$RS' publish v0.1.0"
finish
