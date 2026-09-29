#!/bin/sh
# PHASE3.md S2 and §1.1–§1.4: scripts/lib/sign.sh, dry. Every tool is a shim that records its argv
# and environment; HOME is a temp dir (so the real key cannot be reached: the guard checks it);
# a decoy password is in the caller's environment. The end state of a dry signing run is the
# door's refusal of the bundle the shim "signed" (the real /usr/bin/codesign sees it unsigned):
# sign.sh exits 5 at the --version step and nothing runs. Checked:
#   - the order: announce, profile check, sign, requirement, submit, staple, the dialog question;
#   - notarytool only with --keychain-profile sheepdog-notary, never a password, key or Apple ID;
#   - each tool's environment is the named list, and the decoy reaches no tool, output or file;
#   - the requirement comparison passes the real Developer ID output and refuses a changed marker;
#   - the control mode (--no-notarize) makes no notarytool call and asks nothing;
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
  *"notarytool submit"*) echo "{\"id\":\"00000000-0000-0000-0000-000000000000\",\"status\":\"Accepted\"}" ;;
esac; exit 0'
shim spctl 'exit 0'
shim ditto 'for a; do last=$a; done; : > "$last"; exit 0'
shim security 'case "$*" in *find-identity*) echo "     0 valid identities found" ;; esac; exit 0'

run() { # name args... -> rc; output in $FX/out.<name>
  nm=$1; shift
  rm -f "$S/calls" "$S"/env.* "$FX/ask.rec"; printf 'yes\nyes\n' > "$FX/ask.in"
  mkdir -p "$FX/dest.$nm"
  env HOME="$FX/home" PATH="$S:$PATH" APPLE_APP_SPECIFIC_PASSWORD="$DECOY" CSC_KEY_PASSWORD="$DECOY" GH_TOKEN="$DECOY" NPM_TOKEN="$DECOY" \
    SD_ASK_SCRIPT="$FX/ask.in" SD_ASK_RECORD="$S/calls" \
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
[ "$(grep -c '^ask ' "$S/calls" 2>/dev/null)" = 1 ] && pass "the dialog question asked once" || fail "the dialog question: $(grep -c '^ask ' "$S/calls" 2>/dev/null) reads"
seq=$(grep -n -e '^announce' -e 'notarytool history' -e 'notarytool submit' -e '^ask ' -e 'stapler staple' "$S/calls" | cut -d: -f2 | cut -c1-20 | tr '\n' '|')
case $seq in 'announce|xcrun notarytool his|xcrun notarytool sub|ask sign|xcrun stapler staple|') pass "the order: announce, profile check, submit, the question, then staple" ;; *) fail "the order: $seq" ;; esac
grep notarytool "$S/calls" | grep -v -q -- '--keychain-profile sheepdog-notary' && fail "a notarytool call without the profile" || pass "every notarytool call uses the profile"
grep notarytool "$S/calls" | grep -q -e '--password' -e '--apple-id' -e '--key ' -e '--key-id' -e '--issuer' && fail "notarytool got a credential argument" || pass "notarytool got no credential argument"
grep -q -e '--keychain ' -e 'Library/Keychains' "$S/calls" && fail "a tool got a keychain path" || pass "no tool got a keychain path"
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
! grep -q -e '^ask ' -e '^announce' "$S/calls" && pass "the control mode announces and asks nothing" || fail "the control mode announced or asked"

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
env PATH="$S:$PATH" SD_ASK_SCRIPT="$FX/ask.in" SD_ASK_RECORD="$S/calls" \
  sh "$SIGN" --bin "$FX/bin" --version 0.1.0 --build 1 --tag v0.1.0-rc.1 --commit 0123456789ab --dest "$FX/dest.g1" > "$FX/out.g1" 2>&1; rc=$?
[ $rc = 3 ] && ! grep -q -e '^codesign' -e '^xcrun' "$S/calls" 2>/dev/null && pass "a real HOME: refused (3), no signing tool ran" || fail "real HOME: rc=$rc calls=$(cat "$S/calls" 2>/dev/null | tr '\n' ';')"
# (b) the real mode's parent: sign.sh must be a direct child of `release.sh build --sign`, in any of
# the ways the operator may start it, with that parent's pid and its nonce. A stand-in release.sh
# runs sign.sh as its child; past the guard, the real mode announces the dialogs (then stops at its
# question, as /dev/tty is not there: exit 1, not 3).
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
  (cd "$FX/fake" && env HOME="$FX/home" PATH="$S:$PATH" "$@") > "$FX/out.f" 2>&1; rc=$?
  [ $rc != 3 ] && grep -q 'expect keychain dialogs' "$FX/out.f" && pass "parent '$l': past the guard" || fail "parent '$l': rc=$rc $(grep refused "$FX/out.f" | head -1)"
}
form "scripts/release.sh build --sign" scripts/release.sh build --sign v0.1.0-rc.1
form "./scripts/release.sh build --sign" ./scripts/release.sh build --sign v0.1.0-rc.1
form "sh scripts/release.sh build --sign" sh scripts/release.sh build --sign v0.1.0-rc.1
form "an absolute path" "$FX/fake/scripts/release.sh" build --sign v0.1.0-rc.1
# forged: the test shell as the parent (the nonce matches, the parent pid is its own)
echo forged > "$FX/nonce"; chmod 600 "$FX/nonce"; rm -f "$S/calls"
env HOME="$FX/home" PATH="$S:$PATH" sh "$SIGN" --bin "$FX/bin" --version 0.1.0 --build 1 --tag v0.1.0-rc.1 --commit 0123456789ab \
  --dest "$FX/dest.g2" --real forged --nonce-file "$FX/nonce" --parent-pid $$ > "$FX/out.g2" 2>&1; rc=$?
[ $rc = 3 ] && [ ! -e "$S/calls" ] && pass "a forged --real (not called by release.sh build --sign): refused (3), nothing ran" || fail "forged --real: rc=$rc calls=$(cat "$S/calls" 2>/dev/null | tr '\n' ';')"
# forged: the right parent and arguments, but another pid given (1)
rm -f "$S/calls"
(cd "$FX/fake" && env HOME="$FX/home" PATH="$S:$PATH" SD_PPID=1 scripts/release.sh build --sign v0.1.0-rc.1) > "$FX/out.g3" 2>&1; rc=$?
[ $rc = 3 ] && grep -q "parent's pid" "$FX/out.g3" && [ ! -e "$S/calls" ] && pass "the right parent with another pid given: refused by the pid check (3)" || fail "wrong pid: rc=$rc $(tail -1 "$FX/out.g3")"
finish
