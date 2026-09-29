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
# Then: announce the keychain dialogs (notarizing only); the profile check; the bundle with the
# release ID, built only under /private/tmp and deleted on any failure; codesign with the hardened
# runtime; the signed requirement must equal the release requirement (csreq's canonical form);
# verify; the runtime flag; submit (--keychain-profile only), staple; ask whether the announced
# dialogs appeared (No stops); spctl and stapler validate; --version through the exec door names
# the commit; the archive. The control mode (--no-notarize) signs and checks only: no profile
# check, no dialogs, no submission, no staple.
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
  for t in codesign:/usr/bin/codesign xcrun:/usr/bin/xcrun spctl:/usr/sbin/spctl ditto:/usr/bin/ditto; do
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
  for t in codesign xcrun spctl ditto; do
    p=$(command -v "$t" 2>/dev/null)
    case $p in ''|/usr/bin/*|/bin/*|/usr/sbin/*|/sbin/*) guard "dry mode with the system's $t on PATH ($p)" ;; esac
  done
fi

# --- the work ------------------------------------------------------------------------------------
mkdir -p "$dest" || die "cannot make $dest"
# named, trapped, then made: a signal at any point finds a trap that knows the dir
tmp=/private/tmp/sd-sign.$$.$(od -An -N4 -tx4 /dev/urandom | tr -d ' ')
trap 'rm -rf "$tmp"' EXIT
trap 'rm -rf "$tmp"; exit 1' HUP INT TERM
mkdir -m 700 "$tmp" || die "cannot make $tmp"

ask() { # question -> 0 on "yes"
  if [ $real = yes ]; then
    printf '%s ' "$1" > /dev/tty; IFS= read -r a < /dev/tty || return 1
  else
    n=$(grep -c '^ask ' "${SD_ASK_RECORD:-/dev/null}" 2>/dev/null); n=${n:-0}
    a=$(sed -n "$((n + 1))p" "${SD_ASK_SCRIPT:-/dev/null}")
    echo "ask $1 -> $a" >> "${SD_ASK_RECORD:-/dev/null}"
  fi
  [ "$a" = yes ]
}

if [ $notarize = yes ]; then
  echo "sign: expect keychain dialogs, in this order: (1) the profile check, (2) the submission, and (3) the log fetch only if Apple rejects it. Answer each at the screen; never click Always Allow."
  [ $real = no ] && echo "announce" >> "${SD_ASK_RECORD:-/dev/null}"
  if ! tool xcrun notarytool history --keychain-profile sheepdog-notary > "$dest/notary-profile.txt" 2>&1; then
    die "the profile check failed; see $dest/notary-profile.txt (a Deny, a missing profile, or no network: until the operator records each outcome, it is not told apart)"
  fi
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
  # the reply is kept even when notarytool fails (it may exit non-zero on a rejection)
  res=$(tool xcrun notarytool submit "$tmp/submit.zip" --keychain-profile sheepdog-notary --wait --output-format json 2>&1)
  printf '%s\n' "$res" > "$dest/notary-submit.json"
  case $res in
    *'"status":"Accepted"'*|*'"status": "Accepted"'*) ;;
    *) id=$(printf '%s' "$res" | sed -n 's/.*"id": *"\([^"]*\)".*/\1/p')
       [ -n "$id" ] && tool xcrun notarytool log "$id" --keychain-profile sheepdog-notary > "$dest/notary-log.json" 2>&1
       die "Apple did not accept it; see $dest/notary-log.json" ;;
  esac
  ask "sign: did the keychain dialogs appear, as announced? (yes/no)" || die "the dialogs were not confirmed: redo PHASE3.md D2 step 3 (Confirm before allowing access)"
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
