#!/bin/sh
# PHASE3.md S2 (and §1.2): scripts/release.sh's checks before any build. `release.sh check vTAG`
# runs only the checks. Refused: a dirty tree, an untracked file, HEAD not on the tag, a missing
# or malformed tag, a tag whose X.Y.Z is not Cargo.toml's version, a build counter not above the
# previous tag's (previous = highest tag below, with -rc sorting before the final), a final tag
# whose CHANGELOG has no `## X.Y.Z (YYYY-MM-DD)` entry (still "not yet released", or none): exit 1.
# `build --sign`, `publish` and `publish-npm` refuse under the test environment (exit 3) and without a terminal
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
# (in a subshell: a variable set before a function call stays set after it in bash's POSIX mode)
(FX_STALE_LOCK=1; fx_release 0.3.0 9 v0.3.0-rc.1); chk no "a Cargo.lock that still records the old version" v0.3.0-rc.1
g tag -d v0.3.0-rc.1 >/dev/null; fx_release 0.1.0 5 v0.1.0-x; g tag -d v0.1.0-x >/dev/null
fx_release 0.1.0 1 v0.1.0-rc.2; chk no "a counter not above the previous tag's" v0.1.0-rc.2
fx_release 0.1.0 2 v0.1.0-rc.3; chk ok "a counter above the previous tag's" v0.1.0-rc.3
fx_release 0.1.0 2 v0.1.0; chk no "the final after an rc with the same counter" v0.1.0
g tag -d v0.1.0 >/dev/null
# a final tag needs its CHANGELOG entry dated (an rc does not: rc.1-rc.3 above passed undated); the
# final above dated it, so undate it again first
sed -i.bak 's/^## 0\.1\.0 (.*)$/## 0.1.0 (not yet released)/' "$REPO/CHANGELOG.md" && rm -f "$REPO/CHANGELOG.md.bak"
[ "$(grep -c '^## 0.1.0 (not yet released)$' "$REPO/CHANGELOG.md")" = 1 ] || fail "could not undate the fixture's CHANGELOG"
(FX_UNDATED=1; fx_release 0.1.0 3 v0.1.0); chk no "a final whose CHANGELOG entry says not yet released" v0.1.0
grep -q "CHANGELOG" "$FX/o" && pass "the refusal names the CHANGELOG" || fail "the refusal: $(tail -1 "$FX/o")"
g tag -d v0.1.0 >/dev/null; sed -i.bak '/^## 0.1.0 /d' "$REPO/CHANGELOG.md" && rm -f "$REPO/CHANGELOG.md.bak"
(FX_UNDATED=1; fx_release 0.1.0 3 v0.1.0); chk no "a final with no CHANGELOG entry" v0.1.0
grep -q "CHANGELOG" "$FX/o" && pass "that refusal names the CHANGELOG" || fail "no-entry refusal: $(tail -1 "$FX/o")"
g tag -d v0.1.0 >/dev/null; g checkout -q HEAD~1 -- CHANGELOG.md
# a date that is not one, and a dated entry beside a leftover undated one, are refused too
for bad in "## 0.1.0 (9999-99-99)" "## 0.1.0 (2026-13-01)" "## 0.1.0 (2026-01-32)" "## 0.1.0 (1999-01-01)" "## 0.1.0 (2026-10-04) " "## 0.1.0 (2026-10-04)
## 0.1.0 (not yet released)"; do
  sed -i.bak '/^## 0\.1\.0 /d' "$REPO/CHANGELOG.md" && rm -f "$REPO/CHANGELOG.md.bak" && printf '\n%s\n' "$bad" >> "$REPO/CHANGELOG.md"
  (FX_UNDATED=1; fx_release 0.1.0 3 v0.1.0); chk no "a final whose CHANGELOG has '$(printf '%s' "$bad" | head -1)'$( [ "$(printf '%s\n' "$bad" | wc -l | tr -d ' ')" = 2 ] && echo ' and an undated one')" v0.1.0
  want="found 1 heading(s) for 0.1.0, 0 of them dated"; [ "$(printf '%s\n' "$bad" | wc -l | tr -d ' ')" = 2 ] && want="found 2 heading(s) for 0.1.0, 1 of them dated"
  grep -q 'CHANGELOG' "$FX/o" && grep -qF "$want" "$FX/o" && pass "  that refusal names the CHANGELOG and says: $want" || fail "  the refusal: $(tail -1 "$FX/o")"
  g tag -d v0.1.0 >/dev/null
done
sed -i.bak '/^## 0\.1\.0 /d' "$REPO/CHANGELOG.md" && rm -f "$REPO/CHANGELOG.md.bak" && printf '\n## 0.1.0 (not yet released)\n' >> "$REPO/CHANGELOG.md"
fx_release 0.1.0 3 v0.1.0
chk ok "the final above the last rc's counter, its CHANGELOG entry dated" v0.1.0

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
  refused 3 "publish-npm under $v" $E "$v" sh "$RS" publish-npm v0.1.0
done
# with a lying `env` first on PATH (it lists nothing), the test environment is still seen
mkdir -p "$FX/liar"; printf '#!/bin/sh\nexit 0\n' > "$FX/liar/env"; chmod 755 "$FX/liar/env"
printf '#!/bin/sh\nexec env PATH="%s" HOME="%s" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 "$@"\n' "$FX/liar:$FX/sh:$PATH" "$FX/ghome" > "$FX/eL"; chmod +x "$FX/eL"
refused 3 "publish under SHEEPDOG_TEST_STATE with a lying env on PATH" "$FX/eL" SHEEPDOG_TEST_STATE=/x sh "$RS" publish v0.1.0
# and with a lying `grep` (it never matches): the gate's tools are the system's
mkdir -p "$FX/liarg"; printf '#!/bin/sh\nexit 1\n' > "$FX/liarg/grep"; chmod 755 "$FX/liarg/grep"
printf '#!/bin/sh\nexec env PATH="%s" HOME="%s" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 "$@"\n' "$FX/liarg:$FX/sh:$PATH" "$FX/ghome" > "$FX/eG"; chmod +x "$FX/eG"
for e in "publish" "publish-npm" "build --sign"; do
  refused 3 "$e under SHEEPDOG_TEST_STATE with a lying grep on PATH" "$FX/eG" SHEEPDOG_TEST_STATE=/x sh "$RS" $e v0.1.0
  # GREP_OPTIONS (the system grep reads it) cannot hide it either; control: alone, no terminal (4)
  refused 3 "$e under SHEEPDOG_TEST_STATE with GREP_OPTIONS set" $E SHEEPDOG_TEST_STATE=/x GREP_OPTIONS='-e ^ZZZ' sh "$RS" $e v0.1.0
done
# no terminal: a new session (no controlling tty), the answer piped in
nott() { perl -MPOSIX -e 'my $p = fork(); die unless defined $p; if ($p == 0) { POSIX::setsid() != -1 or die "setsid"; exec @ARGV or die; } waitpid($p, 0); exit($? >> 8)' "$@"; }
refused 4 "build --sign with no terminal" sh -c "echo v0.1.0 | $E sh '$RS' build --sign v0.1.0"
refused 4 "build --sign with no terminal (new session)" nott sh -c "echo v0.1.0 | $E sh '$RS' build --sign v0.1.0"
refused 4 "publish with no terminal (new session)" nott sh -c "echo v0.1.0 | $E sh '$RS' publish v0.1.0"
refused 4 "publish-npm with no terminal (new session)" nott sh -c "echo v0.1.0 | $E sh '$RS' publish-npm v0.1.0"
# control for the GREP_OPTIONS rows above: GREP_OPTIONS alone changes nothing (no terminal: 4)
for e in "publish" "publish-npm" "build --sign"; do
  refused 4 "$e with GREP_OPTIONS alone (control: no terminal)" nott sh -c "echo v0.1.0 | $E GREP_OPTIONS='-e ^ZZZ' sh '$RS' $e v0.1.0"
done
finish
