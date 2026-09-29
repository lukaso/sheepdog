#!/bin/sh
# PHASE3.md S2 and §1.1–§1.4: scripts/lib/sign.sh, dry. Every tool is a shim that records its argv
# and environment; HOME is a temp dir (so the real key cannot be reached: the guard checks it);
# a decoy password is in the caller's environment. The end state of a dry signing run is the
# door's refusal of the bundle the shim "signed" (the real /usr/bin/codesign sees it unsigned):
# sign.sh exits 5 at the --version step and nothing runs. Checked:
#   - the order: sign, requirement; then announce, lock, unlock (the notary keychain's password),
#     profile check, submit, and the notary keychain locked again right after the last notarytool
#     call, before staple, spctl, the door and the archive (the new binary never runs while it is
#     unlocked); a failed lock is loud and fails the build;
#   - notarytool only with --keychain-profile sheepdog-notary and --keychain <the notary keychain
#     file under HOME>, never a password, key or Apple ID; no other tool gets a keychain path;
#   - a failed unlock stops the build before any notarytool call, and the keychain is locked; a
#     missing notary keychain stops it before any keychain call; a rejection unlocks again before
#     the log fetch;
#   - each tool's environment is the named list, and the decoy reaches no tool, output or file;
#   - the requirement comparison passes the real Developer ID output and refuses a changed marker;
#   - the control mode (--no-notarize) makes no notarytool call and no keychain lock or unlock;
#   - the guard: a direct call with a `security` that reports an identity, or with a forged
#     --real nonce, is refused (3) and no signing tool runs;
#   - the release-ID bundle lives only under /private/tmp/sd-sign.* and is gone afterwards.
set -u
. "$(dirname "$0")/lib.sh"
[ "$(uname -s)" = Darwin ] || { echo "SKIP (macOS only)"; exit 0; }
fx_dir
SIGN="$SD_ROOT/scripts/lib/sign.sh"
DECOY=decoy-$(od -An -N6 -tx1 /dev/urandom | tr -d ' \n')
printf 'int main(){return 0;}\n' > "$FX/m.c"; cc -arch arm64 -arch x86_64 -o "$FX/bin" "$FX/m.c" || exit 3
mkdir -p "$FX/home"

# the shims: each appends "name argv" to calls, and its environment to env.<name>.<n>
S=$FX/shims; mkdir -p "$S"
shim() { # name body
  cat > "$S/$1" <<EOF
#!/bin/sh
n=\$(ls "$S" | grep -c "^env\\.$1\\.")
env > "$S/env.$1.\$n"
echo "$1 \$*" >> "$S/calls"
$2
EOF
  chmod +x "$S/$1"
}
shim codesign 'case "$*" in
  *"-d -r-"*) grep -v "^#" "'"$SD_ROOT"'/tests/fixtures/codesign-dr-devid.txt" ;;
  *"-d -v"*) echo "CodeDirectory v=20500 flags=0x10000(runtime)" >&2 ;;
esac; exit 0'
shim xcrun 'case "$*" in
  *"notarytool history"*) echo "Successfully received submission history." ;;
  *"notarytool submit"*) if [ -e "'"$FX"'/reject" ]; then echo "{\"id\":\"11111111-0000-0000-0000-000000000000\",\"status\":\"Invalid\"}"; exit 1; fi
    echo "{\"id\":\"00000000-0000-0000-0000-000000000000\",\"status\":\"Accepted\"}" ;;
esac; exit 0'
shim spctl 'exit 0'
shim ditto 'for a; do last=$a; done; : > "$last"; exit 0'
shim security 'case "$*" in *find-identity*) echo "     0 valid identities found" ;;
  unlock-keychain*) { [ -e "'"$FX"'/unlock.fail" ] || { [ -e "'"$FX"'/unlock.fail2" ] && [ "$(grep -c "^security unlock-keychain" "'"$S"'/calls")" -ge 2 ]; }; } && { echo "security: SecKeychainUnlock: The user name or passphrase you entered is not correct." >&2; exit 51; } ;;
  lock-keychain*) [ -e "'"$FX"'/lock.fail" ] && [ "$(grep -c "^security lock-keychain" "'"$S"'/calls")" -ge 2 ] && { echo "security: lock failed" >&2; exit 50; } ;;
esac; exit 0'
KC=$FX/home/Library/Keychains/sheepdog-notary.keychain-db
mkdir -p "${KC%/*}"; : > "$KC"

run() { # name args... -> rc; output in $FX/out.<name>
  nm=$1; shift
  rm -f "$S/calls" "$S"/env.*
  mkdir -p "$FX/dest.$nm"
  env HOME="$FX/home" PATH="$S:$PATH" APPLE_APP_SPECIFIC_PASSWORD="$DECOY" CSC_KEY_PASSWORD="$DECOY" GH_TOKEN="$DECOY" NPM_TOKEN="$DECOY" \
    SD_ASK_RECORD="$S/calls" \
    sh "$SIGN" --bin "$FX/bin" --version 0.1.0 --build 1 --tag v0.1.0-rc.1 --commit 0123456789ab --dest "$FX/dest.$nm" "$@" \
    > "$FX/out.$nm" 2>&1
}
order() { sed 's/ .*//' "$S/calls" | tr '\n' ' '; }

# 1. a notarizing run
run n; rc=$?
[ $rc = 5 ] && pass "notarizing run ends at the door's refusal (5)" || fail "notarizing run: rc=$rc $(tail -2 "$FX/out.n" | tr '\n' ' ')"
grep -q 'notarytool history' "$S/calls" && grep -q 'notarytool submit' "$S/calls" && grep -q 'stapler staple' "$S/calls" \
  && pass "profile check, submit, staple called" || fail "calls: $(cat "$S/calls" 2>/dev/null | tr '\n' ';')"
seq=$(grep -n -e 'notarytool history' -e '--force' -e 'notarytool submit' -e 'stapler staple' "$S/calls" | cut -d: -f1 | tr '\n' ' ')
[ "$seq" = "$(printf '%s\n' $seq | sort -n | tr '\n' ' ')" ] && [ "$(echo $seq | wc -w | tr -d ' ')" = 4 ] && pass "the order: profile, sign, submit, staple" || fail "the order: $seq"
kcl() { grep -n -e '^announce' -e '^security lock-keychain' -e '^security unlock-keychain' -e 'notarytool' "$S/calls" | cut -d: -f2- | sed -e 's/^security \([a-z-]*\) .*/\1/' -e 's/^xcrun notarytool \([a-z]*\) .*/\1/' | tr '\n' '|'; }
seq=$(kcl)
[ "$seq" = "announce|lock-keychain|unlock-keychain|history|submit|lock-keychain|" ] && pass "the order: announce, lock, unlock, profile check, submit, lock at the end" || fail "the keychain order: $seq"
ln=$(grep -n notarytool "$S/calls" | tail -1 | cut -d: -f1)
[ "$(sed -n "$((ln + 1))p" "$S/calls")" = "security lock-keychain $KC" ] && pass "the notary keychain is locked right after the last notarytool call" || fail "after the last notarytool call: $(sed -n "$((ln + 1))p" "$S/calls")"
lk=$(grep -n '^security lock-keychain' "$S/calls" | tail -1 | cut -d: -f1); st=$(grep -n 'stapler staple' "$S/calls" | head -1 | cut -d: -f1)
[ -n "$lk" ] && [ -n "$st" ] && [ "$lk" -lt "$st" ] && pass "locked before stapling (nothing runs the new binary while it is unlocked)" || fail "lock at $lk, staple at $st"
ul=$(grep -n '^security unlock-keychain' "$S/calls" | head -1 | cut -d: -f1); cs=$(grep -n '^codesign --force' "$S/calls" | head -1 | cut -d: -f1)
[ -n "$ul" ] && [ -n "$cs" ] && [ "$ul" -gt "$cs" ] && pass "unlocked only after codesign" || fail "unlock at $ul, codesign at $cs"
[ "$(grep -c '^security lock-keychain' "$S/calls")" = 2 ] && pass "two locks: before the unlock and after the last notarytool call" || fail "lock calls: $(grep -c '^security lock-keychain' "$S/calls")"
grep notarytool "$S/calls" | grep -v -q -- "--keychain-profile sheepdog-notary --keychain $KC\( \|\$\)" && fail "a notarytool call without the profile and the notary keychain" || pass "every notarytool call uses the profile in the notary keychain"
grep notarytool "$S/calls" | grep -q -e '--password' -e '--apple-id' -e '--key ' -e '--key-id' -e '--issuer' && fail "notarytool got a credential argument" || pass "notarytool got no credential argument"
other=$(grep -e '--keychain ' -e 'Library/Keychains' "$S/calls" | grep -v -e "^xcrun notarytool .* --keychain $KC" -e "^security lock-keychain $KC\$" -e "^security unlock-keychain $KC\$")
[ -z "$other" ] && pass "no other call got a keychain path" || fail "other keychain paths: $other"
bad=""
for f in "$S"/env.*; do
  [ -f "$f" ] || continue
  for k in $(sed 's/=.*//' "$f"); do
    case $k in HOME|PATH|TMPDIR|USER|LOGNAME|DEVELOPER_DIR|PWD|SHLVL|_|OLDPWD) ;; *) bad="$bad $(basename "$f"):$k" ;; esac
  done
done
[ -z "$bad" ] && pass "every tool got only the named environment" || fail "extra environment:$bad"
if grep -r -l "$DECOY" "$S" "$FX/out.n" "$FX/dest.n" >/dev/null 2>&1; then fail "the decoy reached: $(grep -r -l "$DECOY" "$S" "$FX/out.n" "$FX/dest.n" | tr '\n' ' ')"
else pass "the decoy reached no tool, output or file"; fi
grep -q 'refused' "$FX/out.n" && pass "the refusal is the door's" || fail "no door refusal in the output"
ls -d /private/tmp/sd-sign.* >/dev/null 2>&1 && fail "a signing temp dir is left" || pass "no signing temp dir left"
ls "$FX/dest.n" | grep -q Sheepdog.app && fail "a bundle was left in the output" || pass "no bundle left in the output"

# 2. the control mode: no notarytool, no question
run c --no-notarize; rc=$?
[ $rc = 5 ] && pass "control run ends at the door's refusal (5)" || fail "control run: rc=$rc $(tail -2 "$FX/out.c" | tr '\n' ' ')"
grep -q notarytool "$S/calls" && fail "the control mode called notarytool" || pass "the control mode calls no notarytool"
! grep -q -e '^announce' -e '^security lock-keychain' -e '^security unlock-keychain' "$S/calls" && pass "the control mode announces nothing and touches no keychain" || fail "the control mode announced or touched a keychain: $(grep -e announce -e keychain "$S/calls" | tr '\n' ';')"

# 2b. a failed unlock (a wrong password): stops before any notarytool call; the keychain is locked
: > "$FX/unlock.fail"; run u; rc=$?; rm -f "$FX/unlock.fail"
[ $rc = 1 ] && ! grep -q notarytool "$S/calls" && [ "$(tail -1 "$S/calls")" = "security lock-keychain $KC" ] && grep -q 'was not unlocked' "$FX/out.u" \
  && pass "a failed unlock: stopped (1) before notarytool, the keychain locked, the unlock named" || fail "failed unlock: rc=$rc calls=$(tr '\n' ';' < "$S/calls")"
# 2c. no notary keychain: stops before any keychain call, names D2
mv "$KC" "$KC.away"; run m; rc=$?; mv "$KC.away" "$KC"
[ $rc = 1 ] && ! grep -q -e 'keychain' -e notarytool -e '^codesign' "$S/calls" 2>/dev/null && grep -q 'D2' "$FX/out.m" \
  && pass "no notary keychain: stopped (1) before any keychain or signing call, D2 named" || fail "missing keychain: rc=$rc $(tail -1 "$FX/out.m") calls=$(tr '\n' ';' < "$S/calls" 2>/dev/null)"
# 2e. the lock after the last notarytool call fails: loud (names the file), the build fails
: > "$FX/lock.fail"; run l; rc=$?; rm -f "$FX/lock.fail"
[ $rc = 1 ] && grep -q "could not lock the notary keychain $KC" "$FX/out.l" && ! grep -q 'stapler' "$S/calls" \
  && pass "a failed lock: the build fails (1), the file named, nothing after it runs" || fail "failed lock: rc=$rc $(tail -2 "$FX/out.l" | tr '\n' ' ')"
# 2f. a failed unlock whose lock at exit also fails: still loud
: > "$FX/unlock.fail"; : > "$FX/lock.fail"; run lu; rc=$?; rm -f "$FX/unlock.fail" "$FX/lock.fail"
[ $rc != 0 ] && grep -q "could not lock the notary keychain $KC" "$FX/out.lu" \
  && pass "a failed unlock and a failed lock at exit: loud, the file named" || fail "failed unlock+lock: rc=$rc $(tail -2 "$FX/out.lu" | tr '\n' ' ')"
# 2g. Apple rejects it and the second unlock fails: the message says it WAS sent, names the id
: > "$FX/reject"; : > "$FX/unlock.fail2"; run ju; rc=$?; rm -f "$FX/reject" "$FX/unlock.fail2"
[ $rc = 1 ] && ! grep -q 'nothing was sent' "$FX/out.ju" && grep -q '11111111-0000-0000-0000-000000000000' "$FX/out.ju" && [ "$(tail -1 "$S/calls")" = "security lock-keychain $KC" ] \
  && pass "a rejection whose log unlock fails: says the submission was sent, names its id, locks" || fail "rejection + failed unlock: rc=$rc $(tail -2 "$FX/out.ju" | tr '\n' ' ')"
# 2d. Apple rejects it: unlocked again before the log fetch, locked at the end
: > "$FX/reject"; run j; rc=$?; rm -f "$FX/reject"
seq=$(kcl)
[ $rc = 1 ] && [ "$seq" = "announce|lock-keychain|unlock-keychain|history|submit|unlock-keychain|log|lock-keychain|" ] && ! grep -q stapler "$S/calls" \
  && pass "a rejection: unlocked again before the log fetch, locked at the end" || fail "rejection: rc=$rc $seq"

# 3. the requirement comparison refuses a changed marker (a copy of sign.sh's library dir with a
#    release.conf whose requirement lacks one Developer ID marker)
mkdir -p "$FX/alt/lib"; cp "$SD_ROOT/scripts/lib/"*.sh "$FX/alt/lib/"; cp "$SD_ROOT/scripts/bundle.sh" "$FX/alt/"
sed 's/ and certificate 1\[field.1.2.840.113635.100.6.2.6\]//' "$SD_ROOT/scripts/release.conf" > "$FX/alt/release.conf"
grep -q '100.6.2.6' "$FX/alt/release.conf" && fail "the changed requirement was not made"
SIGN_SAVE=$SIGN; SIGN="$FX/alt/lib/sign.sh"
run r; rc=$?
SIGN=$SIGN_SAVE
[ $rc = 1 ] && grep -q -i 'requirement' "$FX/out.r" && pass "a requirement without a Developer ID marker is refused (1)" || fail "changed requirement: rc=$rc $(tail -1 "$FX/out.r")"

# 4. the guard. (a) A real HOME (the key and the real login keychain in reach), everything else
# the same: refused (3) before any signing tool, whatever the security shim says.
rm -f "$S/calls" "$S"/env.*
env PATH="$S:$PATH" SD_ASK_RECORD="$S/calls" \
  sh "$SIGN" --bin "$FX/bin" --version 0.1.0 --build 1 --tag v0.1.0-rc.1 --commit 0123456789ab --dest "$FX/dest.g1" > "$FX/out.g1" 2>&1; rc=$?
[ $rc = 3 ] && ! grep -q -e '^codesign' -e '^xcrun' "$S/calls" 2>/dev/null && pass "a real HOME: refused (3), no signing tool ran" || fail "real HOME: rc=$rc calls=$(cat "$S/calls" 2>/dev/null | tr '\n' ';')"
# (b) the real mode's parent: sign.sh must be a direct child of `release.sh build --sign`, in any of
# the ways the operator may start it, with that parent's pid and its nonce. A stand-in release.sh
# runs sign.sh as its child, with no controlling terminal and the shims first on PATH: past the
# parent check, real mode refuses the shims (the system's tools only), so nothing runs.
nott() { perl -MPOSIX -e 'my $p = fork(); die unless defined $p; if ($p == 0) { POSIX::setsid() != -1 or die "setsid"; exec @ARGV or die; } waitpid($p, 0); exit($? >> 8)' "$@"; }
mkdir -p "$FX/fake/scripts"
cat > "$FX/fake/scripts/release.sh" <<F
#!/bin/sh
umask 077; echo n0nce > "$FX/nonce.f"
exec 3>&-
sh "$SIGN" --bin "$FX/bin" --version 0.1.0 --build 1 --tag v0.1.0-rc.1 --commit 0123456789ab --dest "$FX/dest.f" --real n0nce --nonce-file "$FX/nonce.f" --parent-pid \${SD_PPID:-\$\$}
F
chmod 755 "$FX/fake/scripts/release.sh"
form() { # label cmd... (run from $FX/fake)
  l=$1; shift; rm -f "$S/calls"
  (cd "$FX/fake" && nott env HOME="$FX/home" PATH="$S:$PATH" "$@") > "$FX/out.f" 2>&1 < /dev/null; rc=$?
  [ $rc = 3 ] && grep -q 'non-system' "$FX/out.f" && [ ! -e "$S/calls" ] && pass "parent '$l': past the parent check, then the shims refused, nothing ran" || fail "parent '$l': rc=$rc $(grep refused "$FX/out.f" | head -1)"
}
form "scripts/release.sh build --sign" scripts/release.sh build --sign v0.1.0-rc.1
form "./scripts/release.sh build --sign" ./scripts/release.sh build --sign v0.1.0-rc.1
form "sh scripts/release.sh build --sign" sh scripts/release.sh build --sign v0.1.0-rc.1
form "an absolute path" "$FX/fake/scripts/release.sh" build --sign v0.1.0-rc.1
# forged: the test shell as the parent (the nonce matches, the parent pid is its own)
echo forged > "$FX/nonce"; chmod 600 "$FX/nonce"; rm -f "$S/calls"
env HOME="$FX/home" PATH="$S:$PATH" sh "$SIGN" --bin "$FX/bin" --version 0.1.0 --build 1 --tag v0.1.0-rc.1 --commit 0123456789ab \
  --dest "$FX/dest.g2" --real forged --nonce-file "$FX/nonce" --parent-pid $$ > "$FX/out.g2" 2>&1; rc=$?
[ $rc = 3 ] && grep -q 'not called by release.sh build --sign' "$FX/out.g2" && [ ! -e "$S/calls" ] && pass "a forged --real (not called by release.sh build --sign): refused by the parent check (3), nothing ran" || fail "forged --real: rc=$rc calls=$(cat "$S/calls" 2>/dev/null | tr '\n' ';')"
# forged: the right parent and arguments, but another pid given (1)
rm -f "$S/calls"
(cd "$FX/fake" && env HOME="$FX/home" PATH="$S:$PATH" SD_PPID=1 scripts/release.sh build --sign v0.1.0-rc.1) > "$FX/out.g3" 2>&1; rc=$?
[ $rc = 3 ] && grep -q "parent's pid" "$FX/out.g3" && [ ! -e "$S/calls" ] && pass "the right parent with another pid given: refused by the pid check (3)" || fail "wrong pid: rc=$rc $(tail -1 "$FX/out.g3")"
finish
