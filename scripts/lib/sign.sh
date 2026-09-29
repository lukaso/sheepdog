#!/bin/sh
# Sign, notarize and staple the macOS release (PHASE3.md S2). Called by `release.sh build --sign`,
# and directly by the dry cells (tests/dist/t_sign.sh). Never call it directly otherwise: it can
# sign with the Developer ID without a prompt.
#
#   sign.sh --bin BINARY --version X.Y.Z --build N --tag vTAG --commit SHORT --dest DIR
#           [--no-notarize] [--real NONCE --nonce-file FILE --parent-pid PID]
#
# The guard, before any codesign, xcrun or security call:
#   real mode (--real): only when its parent is `release.sh build --sign` (however it was started:
#     the parent's pid must be the one passed, and its arguments `...release.sh build ... --sign`),
#     which passed a nonce it wrote to a 0600 file of this user's, and with the system's codesign,
#     xcrun, spctl and ditto first on PATH; otherwise refused;
#   dry mode: only when the real key cannot be reached: the real /usr/bin/security (by absolute
#     path) finds exactly 0 signing identities and does not list the real login keychain (an empty
#     or failed answer refuses), and codesign/xcrun/spctl/ditto are not the system's; otherwise
#     refused.
# Then: (notarizing) the notary keychain, $HOME/$SD_NOTARY_KEYCHAIN, must exist; the bundle with
# the release ID, built only under /private/tmp and deleted on any failure; codesign with the
# hardened runtime; the signed requirement must equal the release requirement (csreq's canonical
# form); verify; the runtime flag. Notarizing: announce the password prompt; lock the notary
# keychain, then `security unlock-keychain` asks for its password in the terminal (a failure stops
# before any notarytool call); the profile check; submit (the profile in the notary keychain only;
# on a rejection, unlock again, then the log fetch); lock it again at once, before anything of the
# new build runs (a failed lock is loud and fails the build; any exit in that span locks it too);
# staple; spctl and stapler validate. Then --version through the exec door names the commit; the
# archive. The control mode (--no-notarize) signs and checks only: no keychain, no profile check,
# no submission, no staple.
#
# Every tool gets only HOME, PATH, TMPDIR, USER, LOGNAME (and DEVELOPER_DIR if set).
# Exit: 0 done; 1 a step failed; 2 usage; 3 refused by the guard; 5 the exec door refused the
# signed bundle (the end state of a dry run, whose codesign is a shim).
set -u
lib=$(cd "$(dirname "$0")" && pwd -P) || exit 1
usage() { echo "usage: sign.sh --bin B --version X.Y.Z --build N --tag T --commit C --dest D [--no-notarize] [--real NONCE --nonce-file F --parent-pid P]" >&2; exit 2; }
die() { echo "sign: $*" >&2; exit 1; }
guard() { echo "sign: refused: $*" >&2; exit 3; }

bin="" version="" build="" tag="" commit="" dest="" notarize=yes nonce="" nfile="" ppid=""
while [ $# -gt 0 ]; do
  case $1 in
    --bin|--version|--build|--tag|--commit|--dest|--real|--nonce-file|--parent-pid) [ $# -ge 2 ] || usage ;;
  esac
  case $1 in
    --bin) bin=$2; shift ;; --version) version=$2; shift ;; --build) build=$2; shift ;;
    --tag) tag=$2; shift ;; --commit) commit=$2; shift ;; --dest) dest=$2; shift ;;
    --real) nonce=$2; shift ;; --nonce-file) nfile=$2; shift ;; --parent-pid) ppid=$2; shift ;;
    --no-notarize) notarize=no ;;
    *) usage ;;
  esac
  shift
done
[ -n "$bin" ] && [ -n "$version" ] && [ -n "$build" ] && [ -n "$tag" ] && [ -n "$commit" ] && [ -n "$dest" ] || usage
. "$lib/../release.conf" || die "cannot read release.conf"

tool() { env -i HOME="${HOME:-}" PATH="$PATH" TMPDIR="${TMPDIR:-/tmp}" USER="${USER:-}" LOGNAME="${LOGNAME:-}" \
  ${DEVELOPER_DIR:+DEVELOPER_DIR="$DEVELOPER_DIR"} "$@"; }

# --- the guard -----------------------------------------------------------------------------------
if [ -n "$nonce" ] || [ -n "$nfile" ] || [ -n "$ppid" ]; then
  real=yes
  [ -n "$nonce" ] && [ -f "$nfile" ] && [ -n "$ppid" ] || guard "--real needs a nonce, its file and the parent's pid"
  [ "$PPID" = "$ppid" ] || guard "the parent's pid is $PPID, not $ppid (sign.sh must be release.sh's direct child)"
  [ "$(/usr/bin/stat -f %u "$nfile")" = "$(/usr/bin/id -u)" ] && [ "$(/usr/bin/stat -f %Lp "$nfile")" = 600 ] || guard "the nonce file is not this user's 0600 file"
  [ "$(cat "$nfile")" = "$nonce" ] || guard "the nonce does not match"
  pargs=$(/bin/ps -o args= -p "$PPID" 2>/dev/null)
  case $pargs in *"release.sh build"*" --sign"*) ;; *) guard "not called by release.sh build --sign (parent: $pargs)" ;; esac
  # the real tools only: a shim first on PATH must not answer for codesign or notarytool
  for t in codesign:/usr/bin/codesign xcrun:/usr/bin/xcrun spctl:/usr/sbin/spctl ditto:/usr/bin/ditto security:/usr/bin/security; do
    [ "$(command -v "${t%%:*}" 2>/dev/null)" = "${t#*:}" ] || guard "real mode with a non-system ${t%%:*} on PATH ($(command -v "${t%%:*}" 2>/dev/null))"
  done
else
  real=no
  u=$(/usr/bin/id -un)
  rh=$(/usr/bin/dscl . -read "/Users/$u" NFSHomeDirectory 2>/dev/null | /usr/bin/sed -n 's/^NFSHomeDirectory: *//p')
  [ -n "$rh" ] || guard "cannot read the real home"
  kl=$(/usr/bin/security list-keychains 2>/dev/null) || guard "cannot list the keychains"
  [ -n "$kl" ] || guard "the keychain list is empty (cannot tell whether the key is in reach)"
  case $kl in *"$rh/Library/Keychains/login.keychain"*) guard "the real login keychain is in reach (dry runs need a temp HOME)" ;; esac
  ids=$(/usr/bin/security find-identity -v -p codesigning 2>/dev/null) || guard "cannot count the signing identities"
  printf '%s\n' "$ids" | /usr/bin/grep -q '^ *0 valid identities found$' || guard "a signing identity is in reach"
  for t in codesign xcrun spctl ditto security; do
    p=$(command -v "$t" 2>/dev/null)
    case $p in ''|/usr/bin/*|/bin/*|/usr/sbin/*|/sbin/*) guard "dry mode with the system's $t on PATH ($p)" ;; esac
  done
fi

# --- the work ------------------------------------------------------------------------------------
mkdir -p "$dest" || die "cannot make $dest"
# named, trapped, then made: a signal at any point finds a trap that knows the dir
tmp=/private/tmp/sd-sign.$$.$(od -An -N4 -tx4 /dev/urandom | tr -d ' ')
# kc is set only while the notary keychain may be unlocked (from the lock before the unlock to the
# lock after the last notarytool call); any exit in that span locks it. A failed lock is loud and
# fails the build: the operator must not be told it ended well while the keychain is open.
kc=""
# kc is cleared only after the lock was tried, so a signal during the lock leaves it set and the
# exit trap tries again
relock() {
  [ -n "$kc" ] || return 0
  if tool security lock-keychain "$kc"; then kc=""; return 0; fi
  echo "sign: could not lock the notary keychain $kc; lock it now: security lock-keychain $kc" >&2
  kc=""; return 1
}
trap 'rc=$?; relock; rm -rf "$tmp"; exit $rc' EXIT
trap 'exit 1' HUP INT TERM
mkdir -m 700 "$tmp" || die "cannot make $tmp"
# `security unlock-keychain` with no -p asks for the password in the terminal (never read here)
unlock() { # what-was-sent
  tool security unlock-keychain "$kc" || die "the notary keychain was not unlocked (a wrong password, or the prompt cancelled): $1"; }

if [ $notarize = yes ]; then
  k=$HOME/$SD_NOTARY_KEYCHAIN
  [ -f "$k" ] || die "no notary keychain at $k: do PHASE3.md D2 step 3 first"
fi

app=$("$lib/../bundle.sh" "$bin" "$tmp/b" "$version" "$build" --release-id) || die "bundle.sh"
tool codesign --force --options runtime --timestamp -s "$SD_SIGN_IDENTITY" "$app" || die "codesign"
got=$(tool codesign -d -r- "$app" 2>&1 | sed -n 's/^designated => //p')
want=$(/usr/bin/csreq -r="$SD_RELEASE_REQUIREMENT" -t 2>/dev/null)
[ -n "$want" ] && [ "$got" = "$want" ] || die "the signed requirement is not the release requirement: got '$got', want '$want'"
tool codesign --verify --strict --deep "$app" || die "codesign --verify"
tool codesign -d -v "$app" 2>&1 | grep -q 'flags=.*runtime' || die "the hardened runtime flag is not set"

if [ $notarize = yes ]; then
  tool ditto -c -k --keepParent "$app" "$tmp/submit.zip" || die "ditto"
  # the notary keychain is open only from here to the lock after the last notarytool call: nothing
  # of the new build runs in that span
  echo "sign: security will now ask for the password of the sheepdog-notary keychain (its own password, not your login password), in this terminal. Type it only here. It is locked again right after the submission."
  [ $real = no ] && echo "announce" >> "${SD_ASK_RECORD:-/dev/null}"
  kc=$k
  tool security lock-keychain "$kc" || die "cannot lock the notary keychain $kc"
  unlock "nothing was sent to Apple"
  if ! tool xcrun notarytool history --keychain-profile sheepdog-notary --keychain "$kc" > "$dest/notary-profile.txt" 2>&1; then
    die "the profile check failed; see $dest/notary-profile.txt (a missing profile or no network: until the operator records each outcome, it is not told apart)"
  fi
  # the reply is kept even when notarytool fails (it may exit non-zero on a rejection)
  res=$(tool xcrun notarytool submit "$tmp/submit.zip" --keychain-profile sheepdog-notary --keychain "$kc" --wait --output-format json 2>&1)
  printf '%s\n' "$res" > "$dest/notary-submit.json"
  case $res in
    *'"status":"Accepted"'*|*'"status": "Accepted"'*) ;;
    *) id=$(printf '%s' "$res" | sed -n 's/.*"id": *"\([^"]*\)".*/\1/p')
       # the keychain may have locked itself during --wait (5 minutes idle)
       [ -n "$id" ] && unlock "the submission $id WAS sent and Apple did not accept it; its log was not fetched: xcrun notarytool log $id --keychain-profile sheepdog-notary --keychain $kc" \
         && tool xcrun notarytool log "$id" --keychain-profile sheepdog-notary --keychain "$kc" > "$dest/notary-log.json" 2>&1
       die "Apple did not accept it; see $dest/notary-log.json" ;;
  esac
  relock || exit 1
  tool xcrun stapler staple "$app" || die "stapler staple"
  tool spctl --assess --type execute "$app" || die "spctl rejects the bundle"
  tool xcrun stapler validate "$app" || die "stapler validate"
fi

mkdir -p "$tmp/rh"
v=$("$lib/release-run.sh" "$tmp/rh" "$app/Contents/MacOS/sheepdog" --version 2>"$tmp/door.err"); vr=$?
if [ $vr != 0 ]; then
  grep -q 'exec-guard: refused' "$tmp/door.err" && { cat "$tmp/door.err" >&2; echo "sign: the exec door refused the signed bundle" >&2; exit 5; }
  die "the signed binary does not run: $(cat "$tmp/door.err")"
fi
case $v in *"$commit"*) ;; *) die "the signed binary names another commit: $v" ;; esac

if [ $notarize = yes ]; then chk=--signed; else chk=--signed-unstapled; fi
"$lib/archive.sh" make "$app" "$dest/sheepdog-macos-universal.tar.gz" || die "archive"
"$lib/archive.sh" check "$dest/sheepdog-macos-universal.tar.gz" $chk || die "the archive fails its check"
echo "sign: $tag signed${notarize:+ }$( [ $notarize = yes ] && echo 'and notarized' || echo '(control: not notarized)')"
