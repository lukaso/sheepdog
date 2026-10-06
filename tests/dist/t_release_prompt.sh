#!/bin/sh
# scripts/release.sh's typed-tag confirmation, at a real terminal (a pty from `script`): `publish`
# and `publish-npm` make a release public and ask for the tag (their only human confirmation);
# `build --sign` does not ask (the notary keychain's own password is its gate). Every run names a
# tag that does not exist, with no input: a run that asks stops at the empty answer (exit 4), one
# that does not goes on to the checks and stops there (no tag: exit 1), before any tool runs.
# No test-environment variable is set (the entries refuse those first: exit 3).
set -u
. "$(dirname "$0")/lib.sh"
[ "$(uname -s)" = Darwin ] || { echo "SKIP (macOS script syntax)"; exit 0; }
fx_dir; fx_repo
t() { # label want-rc want-prompt(yes|no) want-text args...: the output holds want-text and no
  # `release: building` (the no-tag refusal is the build's only stop before its tools run)
  l=$1 want=$2 asks=$3 txt=$4; shift 4
  (cd "$REPO" && env -i PATH=/usr/bin:/bin HOME="$FX/ghome" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 \
    script -q /dev/null sh scripts/release.sh "$@" < /dev/null) > "$FX/o" 2>&1; rc=$?
  # script returns its command's status; the prompt is written to the pty, which script copies out
  p=no; grep -q 'Type the tag' "$FX/o" && p=yes
  if [ "$rc" = "$want" ] && [ "$p" = "$asks" ] && grep -qF "$txt" "$FX/o" && ! grep -q 'release: building' "$FX/o"; then pass "$l"
  else fail "$l: rc=$rc (want $want), prompt=$p (want $asks): $(tr '\r\n' '  ' < "$FX/o" | cut -c1-200)"; fi
}
t "build --sign at a terminal: no typed tag, on to the checks (no tag v9.9.9)" 1 no "release: no tag v9.9.9" build --sign v9.9.9
t "build --sign --no-notarize at a terminal: no typed tag either" 1 no "release: no tag v9.9.9" build --sign --no-notarize v9.9.9
t "control: publish at a terminal asks for the tag (no answer: not confirmed)" 4 yes "Type the tag" publish v9.9.9
t "control: publish-npm at a terminal asks for the tag" 4 yes "Type the tag" publish-npm v9.9.9
finish
